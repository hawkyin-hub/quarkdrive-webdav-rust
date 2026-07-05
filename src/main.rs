//! QuarkDrive-WebDAV 启动入口(终结代理 + 后端双进程模型)。
//!
//! 复刻原 Python 版 `LocalQuark` 启动流程:
//!   1. cookie::CookieStore::from_chromium
//!   2. 生成随机 webdav password + 落盘
//!   3. 启动 HTTP WebDAV 后端 (127.0.0.1:8080, dav-server)
//!   4. 启动 HTTPS 终结代理 (127.0.0.1:8443, TLS 1.2 + ALPN http/1.1)
//!   5. 挂载前根 PROPFIND 预热 (https_proxy.py warm_root_sync)
//!   6. 挂载点:由 helper / mount_webdav 接手
//!   7. 12h cookie refresh + 60s health check
//!   8. tray::run 阻塞
//!
//! 与原版 Python 行为一一对应:
//!   quarkdrive-webdav (HTTP 8080) = 原 quarkdrive-webdav 二进制
//!   proxy (HTTPS 8443)            = 原 https_proxy.py
//!   mount_webdav                  = 原 mount_core.mount_webdav

use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use dashmap::DashMap;
use rand::RngCore;
use tokio::sync::Notify;
use tokio::time::interval;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use quarkdrive_webdav::cookie::{CookieStore, DEFAULT_BROWSERS};
use quarkdrive_webdav::drive::{DriveConfig, QuarkDrive};
use quarkdrive_webdav::health::{self, HealthChecker};
use quarkdrive_webdav::mount::{self, MountConfig};
use quarkdrive_webdav::notifier;
use quarkdrive_webdav::proxy::{self, ProxyConfig};
use quarkdrive_webdav::tray;
use quarkdrive_webdav::vfs::QuarkDriveFileSystem;
use quarkdrive_webdav::webdav::WebDavServer;

#[cfg(unix)]
use {signal_hook::consts::signal::*, signal_hook_tokio::Signals};
#[cfg(unix)]
use futures_util::stream::StreamExt;

#[derive(Parser, Debug)]
#[command(name = "quarkdrive-webdav", about, version, author)]
struct Opt {
    /// HTTPS 终结代理监听地址(对外,即 mount_webdav 连接的目标)
    #[arg(long, env = "HOST", default_value = "127.0.0.1")]
    host: String,
    #[arg(long, env = "PORT", default_value = "8443")]
    port: u16,

    /// HTTP WebDAV 后端监听地址(仅本地)
    #[arg(long, env = "BACKEND_HOST", default_value = "127.0.0.1")]
    backend_host: String,
    #[arg(long, env = "BACKEND_PORT", default_value = "8080")]
    backend_port: u16,

    /// 显式传入 cookie(分号串);不传则从浏览器抓
    #[arg(long, env = "QUARK_COOKIE")]
    quark_cookie: Option<String>,

    #[arg(short = 'U', long, env = "WEBDAV_AUTH_USER", default_value = "quasar")]
    auth_user: String,
    #[arg(short = 'W', long, env = "WEBDAV_AUTH_PASSWORD")]
    auth_password: Option<String>,

    #[arg(long, default_value = "~/Mount/Quark")]
    mount_point: String,

    /// TLS 证书/私钥(强制要求;终结代理需要)
    #[arg(long, env = "TLS_CERT")]
    tls_cert: Option<PathBuf>,
    #[arg(long, env = "TLS_KEY")]
    tls_key: Option<PathBuf>,

    #[arg(long, default_value = "43200")]
    cookie_refresh_secs: u64,
    #[arg(long, default_value = "60")]
    health_check_secs: u64,

    #[arg(long)]
    debug: bool,

    /// 仅 server 模式(不挂载、不菜单栏、不健康检查)
    #[arg(long)]
    serve_only: bool,

    /// 挂载前是否做根 PROPFIND 预热(默认开,失败不阻塞)
    #[arg(long, default_value = "true")]
    warm_root: bool,

    #[command(subcommand)]
    subcommands: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 显式启动 server(同 --serve-only)
    Serve,
    /// 健康检查(打印当前状态后退出)
    Health,
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

fn generate_token() -> String {
    let mut buf = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    eprintln!("[main] entering main");

    // DEBUG: 捕获 panic 到 stderr + 文件,看为什么进程突然死
    std::panic::set_hook(Box::new(|info| {
        eprintln!("!!! PANIC: {}", info);
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("!!! BACKTRACE:\n{}", bt);
        let mut f = std::fs::OpenOptions::new()
            .create(true).append(true).open("/tmp/wd_panic.log").ok();
        if let Some(f) = f.as_mut() {
            use std::io::Write;
            let _ = writeln!(f, "PANIC: {}\n{}", info, bt);
        }
    }));

    let opt = Opt::parse();

    if env::var("RUST_LOG").is_err() {
        if opt.debug {
            unsafe { env::set_var("RUST_LOG", "quarkdrive_webdav=debug,reqwest=debug,proxy=debug"); }
        } else {
            unsafe { env::set_var("RUST_LOG", "quarkdrive_webdav=info,reqwest=warn"); }
        }
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_timer(tracing_subscriber::fmt::time::time())
        .init();

    if matches!(opt.subcommands, Some(Commands::Health)) {
        let cookies = CookieStore::from_chromium(DEFAULT_BROWSERS).await?;
        let mount_point = expand_home(&opt.mount_point);
        let url = format!("https://{}:{}/", opt.host, opt.port);
        let checker = HealthChecker::new(cookies, url, mount_point);
        let r = checker.check().await;
        println!("{r:?}");
        return Ok(());
    }

    let serve_only = opt.serve_only || matches!(opt.subcommands, Some(Commands::Serve));

    // 1. cookie
    let cookie_store = if let Some(s) = opt.quark_cookie.clone() {
        let map: std::collections::HashMap<String, String> = s
            .split(';')
            .filter_map(|p| p.trim().split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .collect();
        let store = CookieStore::default();
        store.replace(map);
        store
    } else {
        CookieStore::from_chromium(DEFAULT_BROWSERS).await
            .context("从浏览器抓 cookie 失败;请打开 Chrome 一次以解锁 keychain")?
    };

    // 2. webdav password
    let webdav_password = match opt.auth_password.clone() {
        Some(p) => p,
        None => {
            let p = generate_token();
            mount::write_passwd(&p)?;
            eprintln!("[main] webdav password saved"); info!("webdav password saved to disk");
            p
        }
    };

    // 3. drive + fs
    let cookie_map = cookie_store.snapshot();
    let drive_cookie: Arc<DashMap<String, String>> = Arc::new(DashMap::new());
    for (k, v) in &cookie_map {
        drive_cookie.insert(k.clone(), v.clone());
    }
    let drive_config = DriveConfig {
        api_base_url: "https://drive.quark.cn".to_string(),
        cookie: drive_cookie,
    };
    let drive = QuarkDrive::new(drive_config)?;
    let mut fs = QuarkDriveFileSystem::new(drive, "/".to_string(), 1000u64, 600u64)?;
    fs.set_read_only(false).set_no_trash(false);
    let fs_for_webdav = fs.clone();

    // 4. TLS
    let (tls_cert, tls_key) = ensure_tls(&opt)?;

    eprintln!("[main] step 5: spawning backend server task"); // 5. spawn 后端 WebDAV server
    use dav_server::memls::MemLs;
    use dav_server::DavHandler;
    let dav_handler = DavHandler::builder()
        .filesystem(Box::new(fs))
        .locksystem(MemLs::new())
        .read_buf_size(10 * 1024 * 1024)
        .autoindex(false)
        .redirect(false)
        .build_handler();
    let backend_server = WebDavServer {
        host: opt.backend_host.clone(),
        port: opt.backend_port,
        auth_user: Some(opt.auth_user.clone()),
        auth_password: Some(webdav_password.clone()),
        tls_config: None, // 后端明文 HTTP,只对本地
        handler: dav_handler,
        fs: fs_for_webdav,
        strip_prefix: None,
    };

    eprintln!("[main] step 6: spawning proxy task"); eprintln!("[main] before proxy::run task"); // 6. spawn 终结代理
    let proxy_cfg = ProxyConfig {
        https_host: opt.host.clone(),
        https_port: opt.port,
        backend_host: opt.backend_host.clone(),
        backend_port: opt.backend_port,
        cert_path: tls_cert.clone(),
        key_path: tls_key.clone(),
        auth_user: opt.auth_user.clone(),
        auth_password: webdav_password.clone(),
        cookies: cookie_store.clone(),
    };

    // 在 serve_only 模式下,只跑两个 server 然后阻塞
    eprintln!("[main] serve_only path"); if serve_only {
        // 先 spawn 两个 server,再 wait_for_port(注意顺序!)
        // 后端 task
        let backend_handle = tokio::spawn(async move {
            if let Err(e) = backend_server.serve().await {
                warn!(error = %e, "backend serve ended");
            }
        });
        // 终结代理 task
        let proxy_handle = tokio::spawn(async move {
            if let Err(e) = proxy::run(proxy_cfg).await {
                warn!(error = %e, "proxy serve ended");
            }
        });

        // 等后端监听
        wait_for_port(&opt.backend_host, opt.backend_port, 5).await?;
        eprintln!("[main] backend ready"); info!(port = opt.backend_port, "backend ready");

        // 等待代理就绪
        wait_for_port(&opt.host, opt.port, 5).await?;
        eprintln!("[main] proxy ready"); info!(port = opt.port, "https proxy ready");

        // 任一退出 -> 整体退出
        tokio::select! {
            _ = backend_handle => { warn!("backend exited"); }
            _ = proxy_handle => { warn!("proxy exited"); }
        }
        return Ok(());
    }

    // === 全功能模式(挂载 + 后台 + 12h cookie refresh + 健康检查 + tray) ===
    // 后端 task
    let backend_handle = tokio::spawn(async move {
        if let Err(e) = backend_server.serve().await {
            warn!(error = %e, "backend serve ended");
        }
    });
    wait_for_port(&opt.backend_host, opt.backend_port, 5).await?;
    eprintln!("[main] backend ready"); info!(port = opt.backend_port, "backend ready");

    // 终结代理 task
    let proxy_cfg_for_task = proxy_cfg.clone();
    let proxy_handle = tokio::spawn(async move {
        if let Err(e) = proxy::run(proxy_cfg_for_task).await {
            warn!(error = %e, "proxy serve ended");
        }
    });
    wait_for_port(&opt.host, opt.port, 5).await?;
    eprintln!("[main] proxy ready"); info!(port = opt.port, "https proxy ready");

    // 7. 根 PROPFIND 预热(防止 webdavfs 第一次看到空文件夹)
    if opt.warm_root {
        match proxy::warm_root(&proxy_cfg).await {
            Ok(()) => info!("warm_root: ok"),
            Err(e) => warn!(error = %e, "warm_root failed; mount may show empty dir briefly"),
        }
    }

    // 8. mount(走 mount.rs;实际由 helper 挂载,这里尝试一下,失败不阻塞)
    let mount_point = expand_home(&opt.mount_point);
    let mount_cfg = MountConfig {
        mount_point: mount_point.clone(),
        webdav_url: format!("https://{}:{}", opt.host, opt.port),
        user: opt.auth_user.clone(),
        pass: webdav_password.clone(),
    };
    if let Err(e) = mount::mount(&mount_cfg).await {
        warn!(error = %e, "mount failed; https proxy still running");
        notifier::notify("QuarkDrive", &format!("挂载失败: {e}"));
    }

    // 9. health checker
    let checker = Arc::new(HealthChecker::new(
        cookie_store.clone(),
        format!("https://{}:{}", opt.host, opt.port),
        mount_point.clone(),
    ));
    let _health_task = health::spawn_loop(
        checker.clone(),
        mount_cfg.clone(),
        Duration::from_secs(opt.health_check_secs),
    );

    // 10. 12h cookie refresh
    let refresh_store = cookie_store.clone();
    let _refresh_task = tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(opt.cookie_refresh_secs));
        loop {
            ticker.tick().await;
            match CookieStore::from_chromium(DEFAULT_BROWSERS).await {
                Ok(s) => {
                    refresh_store.replace(s.snapshot());
                    info!("cookies auto-refreshed");
                }
                Err(e) => warn!(error = %e, "scheduled cookie refresh failed"),
            }
        }
    });

    // 11. tray(阻塞)
    let shutdown = Arc::new(Notify::new());
    let tray_cfg = tray::TrayConfig {
        mount_point: mount_point.clone(),
        cookies: cookie_store.clone(),
        health: checker.clone(),
        shutdown: shutdown.clone(),
    };

    #[cfg(unix)]
    {
        let signals = Signals::new([SIGTERM, SIGINT])?;
        let handle = signals.handle();
        let sd = shutdown.clone();
        let mp = mount_point.clone();
        tokio::spawn(async move {
            let mut s = signals;
            while let Some(_sig) = s.next().await {
                info!("signal received, shutting down");
                let _ = mount::unmount(&mp).await;
                sd.notify_waiters();
                break;
            }
        });
        tray::run(tray_cfg).await?;
        handle.close();
    }
    #[cfg(not(unix))]
    {
        tray::run(tray_cfg).await?;
    }

    backend_handle.abort();
    proxy_handle.abort();
    let r = Ok(());
    eprintln!("[main] returning {:?} from main", r);
    r
}

async fn wait_for_port(host: &str, port: u16, timeout_secs: u64) -> Result<()> {
    use std::net::TcpStream;
    use std::time::Duration;
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    while std::time::Instant::now() < deadline {
        if TcpStream::connect((host, port)).is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("port {}:{} not listening within {}s", host, port, timeout_secs)
}

/// TLS 证书:有就用,没有就生成自签。
fn ensure_tls(opt: &Opt) -> Result<(PathBuf, PathBuf)> {
    if let (Some(c), Some(k)) = (&opt.tls_cert, &opt.tls_key) {
        return Ok((c.clone(), k.clone()));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME not set"))?;
    let dir = PathBuf::from(home).join("Library/Application Support/QuarkDrive");
    std::fs::create_dir_all(&dir)?;
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    if cert.exists() && key.exists() {
        return Ok((cert, key));
    }
    generate_self_signed(&cert, &key)?;
    info!(cert = %cert.display(), key = %key.display(), "self-signed cert generated");
    Ok((cert, key))
}

fn generate_self_signed(cert: &PathBuf, key: &PathBuf) -> Result<()> {
    use std::process::Command;
    let subj = "/CN=127.0.0.1";
    let status = Command::new("/usr/bin/openssl")
        .args([
            "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout",
        ])
        .arg(key)
        .args(["-out"])
        .arg(cert)
        .args(["-days", "3650", "-subj", subj, "-addext", "subjectAltName=IP:127.0.0.1"])
        .status()
        .context("spawn openssl")?;
    if !status.success() {
        bail!("openssl exited with {:?}", status.code());
    }
    Ok(())
}
