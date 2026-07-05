//! WebDAV 挂载(macOS 原生 mount_webdav)。
//!
//! 设计:docs/rust-migration.md §4.2
//!   mount_webdav -S -o url=https://127.0.0.1:8443,username=<u>,password=<p> ~/Mount/Quark
//!
//! 状态查询走 /sbin/mount 输出 + umount 走 /sbin/umount。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use tokio::process::Command;
use tracing::{info, warn};

/// 默认 webdav 凭据文件。
pub fn passwd_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME not set"))?;
    let dir = PathBuf::from(home).join("Library/Application Support/QuarkDrive");
    std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    Ok(dir.join("webdav.passwd"))
}

pub fn write_passwd(passwd: &str) -> Result<PathBuf> {
    use std::io::Write;
    let path = passwd_path()?;
    let mut f = std::fs::File::create(&path)
        .with_context(|| format!("create {}", path.display()))?;
    f.write_all(passwd.as_bytes())?;
    // 0o600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = f.metadata()?.permissions();
        perm.set_mode(0o600);
        f.set_permissions(perm)?;
    }
    Ok(path)
}

#[derive(Debug, Clone)]
pub struct MountConfig {
    pub mount_point: PathBuf,
    pub webdav_url: String,
    pub user: String,
    pub pass: String,
}

/// 调 `mount_webdav` 挂载。已挂载则直接返回 Ok。
pub async fn mount(cfg: &MountConfig) -> Result<()> {
    if is_mounted(&cfg.mount_point).await {
        info!(point = %cfg.mount_point.display(), "already mounted");
        return Ok(());
    }
    std::fs::create_dir_all(&cfg.mount_point)
        .with_context(|| format!("mkdir {}", cfg.mount_point.display()))?;

    // 解析 host 和 port 用于 Keychain 存储
    let url_str = cfg.webdav_url.strip_prefix("https://").unwrap_or(&cfg.webdav_url);
    let (host, port) = url_str.split_once(':').unwrap_or((url_str, "443"));

    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME not set"))?;
    let keychain_path = PathBuf::from(&home).join("Library/Keychains/login.keychain-db");

    // 1. 将密码写入 macOS Keychain，webdavfs_agent 会自动在此处匹配
    info!(host, port, "writing credentials to keychain");
    let status = Command::new("/usr/bin/security")
        .arg("add-internet-password")
        .arg("-a")
        .arg(&cfg.user)
        .arg("-s")
        .arg(host)
        .arg("-P")
        .arg(port)
        .arg("-r")
        .arg("htps")
        .arg("-w")
        .arg(&cfg.pass)
        .arg("-U") // update if exists
        .arg(&keychain_path)
        .status()
        .await
        .with_context(|| "spawn /usr/bin/security to add-internet-password")?;

    if !status.success() {
        warn!("security add-internet-password exited with {:?}", status.code());
    }

    // 2. 将自签证书信任加入 Keychain，解决 macOS 26.6+ webdavfs_agent 握手失败
    let cert_path = PathBuf::from(&home).join("Library/Application Support/QuarkDrive/cert.pem");
    if cert_path.exists() {
        info!(cert = %cert_path.display(), "trusting self-signed certificate");
        let trust_future = Command::new("/usr/bin/security")
            .arg("add-trusted-cert")
            .arg("-p")
            .arg("ssl")
            .arg("-r")
            .arg("trustRoot")
            .arg("-k")
            .arg(&keychain_path)
            .arg(&cert_path)
            .status();
        match tokio::time::timeout(Duration::from_secs(3), trust_future).await {
            Ok(Ok(status)) => {
                if !status.success() {
                    warn!("security add-trusted-cert exited with {:?}", status.code());
                }
            }
            Ok(Err(e)) => {
                warn!("security add-trusted-cert failed to spawn: {}", e);
            }
            Err(_) => {
                warn!("security add-trusted-cert timed out (probably waiting for user password dialog)");
            }
        }
    }

    // 3. 执行 mount_webdav，使用 -s 选项（强制 HTTPS 挂载安全校验），不再传入 credentials 选项
    info!(point = %cfg.mount_point.display(), url = %cfg.webdav_url, "mount_webdav");
    let status = Command::new("/sbin/mount_webdav")
        .arg("-s")
        .arg(&cfg.webdav_url)
        .arg(&cfg.mount_point)
        .status()
        .await
        .with_context(|| "spawn /sbin/mount_webdav")?;

    if !status.success() {
        bail!("mount_webdav exited with {:?}", status.code());
    }
    Ok(())
}

/// /sbin/mount 输出里是否包含挂载点。
pub async fn is_mounted(point: &Path) -> bool {
    let out = match Command::new("/sbin/mount").output().await {
        Ok(o) => o,
        Err(e) => {
            warn!(error = %e, "/sbin/mount failed");
            return false;
        }
    };
    let s = String::from_utf8_lossy(&out.stdout);
    s.contains(point.to_string_lossy().as_ref())
}

pub async fn unmount(point: &Path) -> Result<()> {
    if !is_mounted(point).await {
        return Ok(());
    }
    let status = Command::new("/sbin/umount")
        .arg(point)
        .status()
        .await
        .with_context(|| "spawn /sbin/umount")?;
    if !status.success() {
        bail!("umount exited with {:?}", status.code());
    }
    Ok(())
}

/// 防止 user/pass 里有 `,` `=` 破坏 mount_webdav 的 -o 解析。
/// 这里采用 POSIX 单引号转义,避开逗号/等号。
fn shell_escape(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@')) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_safe_passthrough() {
        assert_eq!(shell_escape("user_name-1.2@x"), "user_name-1.2@x");
    }

    #[test]
    fn escape_quotes() {
        assert_eq!(shell_escape("a'b"), "'a'\\''b'");
    }
}
