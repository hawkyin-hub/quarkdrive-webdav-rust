//! 健康检查 + 自动修复(60s tick)。
//!
//! 设计:docs/rust-migration.md §4.3
//!   指标: sl-session 存在 / OPTIONS 200 / /sbin/mount 包含挂载点
//!   故障:
//!     cookies 缺失  → CookieStore::from_chromium
//!     webdav 不可达 → 杀进程 + 重启 webdav task
//!     挂载掉了     → mount::mount

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use reqwest::Client;
use tokio::sync::Notify;
use tokio::time::interval;
use tracing::{info, warn};

use crate::cookie::{BrowserKind, CookieStore, DEFAULT_BROWSERS};
use crate::mount::{self, MountConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthLevel {
    Healthy,
    CookieMissing,
    WebDavUnreachable,
    MountMissing,
}

#[derive(Debug, Clone)]
pub struct HealthReport {
    pub level: HealthLevel,
    pub cookies_ok: bool,
    pub webdav_ok: bool,
    pub mounted_ok: bool,
}

impl HealthReport {
    pub fn is_healthy(&self) -> bool {
        self.level == HealthLevel::Healthy
    }
}

pub struct HealthChecker {
    pub cookies: CookieStore,
    pub webdav_url: String,
    pub mount_point: PathBuf,
    /// webdav 进程重启触发器(由 main.rs 设置)
    pub restart_signal: Arc<Notify>,
    client: Client,
}

impl HealthChecker {
    pub fn new(cookies: CookieStore, webdav_url: String, mount_point: PathBuf) -> Self {
        Self {
            cookies,
            webdav_url,
            mount_point,
            restart_signal: Arc::new(Notify::new()),
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .danger_accept_invalid_certs(true)
                .build()
                .expect("reqwest client"),
        }
    }

    pub async fn check(&self) -> HealthReport {
        let cookies_ok = self.cookies.get("sl-session").is_some();
        let webdav_ok = self.probe_webdav().await;
        let mounted_ok = mount::is_mounted(&self.mount_point).await;

        let level = if !cookies_ok {
            HealthLevel::CookieMissing
        } else if !webdav_ok {
            HealthLevel::WebDavUnreachable
        } else if !mounted_ok {
            HealthLevel::MountMissing
        } else {
            HealthLevel::Healthy
        };

        HealthReport { level, cookies_ok, webdav_ok, mounted_ok }
    }

    async fn probe_webdav(&self) -> bool {
        match self.client.request(reqwest::Method::OPTIONS, &self.webdav_url).send().await {
            Ok(r) => r.status().is_success(),
            Err(e) => {
                warn!(error = %e, "webdav OPTIONS failed");
                false
            }
        }
    }

    /// 根据故障类型路由到对应修复路径。
    pub async fn repair(&self, report: &HealthReport, mount_cfg: &MountConfig) -> Result<()> {
        match report.level {
            HealthLevel::Healthy => Ok(()),
            HealthLevel::CookieMissing => {
                info!("repair: refreshing cookies");
                match CookieStore::from_chromium(DEFAULT_BROWSERS).await {
                    Ok(new_store) => {
                        self.cookies.replace(new_store.snapshot());
                        Ok(())
                    }
                    Err(e) => {
                        warn!(error = %e, "cookie refresh failed");
                        Err(e)
                    }
                }
            }
            HealthLevel::WebDavUnreachable => {
                warn!("repair: restarting webdav");
                self.restart_signal.notify_waiters();
                Ok(())
            }
            HealthLevel::MountMissing => {
                info!("repair: remount");
                mount::mount(mount_cfg).await
            }
        }
    }
}

/// 在后台跑 60s tick 循环。返回的 task 由调用方持有以便 shutdown。
pub fn spawn_loop(
    checker: Arc<HealthChecker>,
    mount_cfg: MountConfig,
    period: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(period);
        loop {
            ticker.tick().await;
            let report = checker.check().await;
            if !report.is_healthy() {
                if let Err(e) = checker.repair(&report, &mount_cfg).await {
                    warn!(error = %e, level = ?report.level, "repair failed");
                }
            }
        }
    })
}

#[allow(dead_code)]
fn _unused_priority_marker(_b: BrowserKind) {} // keep import stable
