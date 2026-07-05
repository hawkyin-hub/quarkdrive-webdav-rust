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

    #[cfg(target_os = "macos")]
    {
        // 解析 host 和 port 用于 Keychain 存储
        let url_str = cfg.webdav_url.strip_prefix("https://").unwrap_or(&cfg.webdav_url);
        let (host, port) = url_str.split_once(':').unwrap_or((url_str, "443"));

        let home = std::env::var("HOME").context("HOME not set")?;
        let keychain_path = format!("{}/Library/Keychains/login.keychain-db", home);

        // 使用 Padded 凭据 (user + 'x', pass + 'x') 以维持与 legacy 相同的 webdavfs_agent 零弹框交互行为
        let padded_user = format!("{}x", cfg.user);
        let padded_pass = format!("{}x", cfg.pass);

        info!(host, port, user = %padded_user, "writing WebDAV credentials to login keychain");

        // 1. Internet Password (供 webdavfs_agent 自动查找)
        let status1 = Command::new("/usr/bin/security")
            .arg("add-internet-password")
            .arg("-a").arg(&padded_user)
            .arg("-s").arg(host)
            .arg("-P").arg(port)
            .arg("-r").arg("htps")
            .arg("-w").arg(&padded_pass)
            .arg("-A")
            .arg("-T").arg("/usr/bin/security")
            .arg("-T").arg("/System/Library/Extensions/webdav_fs.kext/Contents/Resources/webdavfs_agent")
            .arg("-U")
            .arg(&keychain_path)
            .status()
            .await
            .context("spawn /usr/bin/security add-internet-password")?;

        if !status1.success() {
            warn!("security add-internet-password exited with {:?}", status1.code());
        }

        // 2. Generic Password (供健康检查等通用服务匹配)
        let status2 = Command::new("/usr/bin/security")
            .arg("add-generic-password")
            .arg("-a").arg(&padded_user)
            .arg("-s").arg("LocalQuark WebDAV")
            .arg("-w").arg(&padded_pass)
            .arg("-A")
            .arg("-T").arg("/usr/bin/security")
            .arg("-T").arg("/System/Library/Extensions/webdav_fs.kext/Contents/Resources/webdavfs_agent")
            .arg("-U")
            .arg(&keychain_path)
            .status()
            .await
            .context("spawn /usr/bin/security add-generic-password")?;

        if !status2.success() {
            warn!("security add-generic-password exited with {:?}", status2.code());
        }

        // 3. 将自签证书信任加入 Keychain，解决 macOS 26.6+ webdavfs_agent 握手失败
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

        // 4. 执行 mount_webdav，使用小写 -s 参数 (强制 HTTPS) 以及 -v 参数指定卷名为 "LocalQuark"
        info!(point = %cfg.mount_point.display(), url = %cfg.webdav_url, "mount_webdav");
        let status = Command::new("/sbin/mount_webdav")
            .arg("-s")
            .arg("-v")
            .arg("Quark")
            .arg(&cfg.webdav_url)
            .arg(&cfg.mount_point)
            .status()
            .await
            .with_context(|| "spawn /sbin/mount_webdav")?;

        if !status.success() {
            bail!("mount_webdav exited with {:?}", status.code());
        }
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
