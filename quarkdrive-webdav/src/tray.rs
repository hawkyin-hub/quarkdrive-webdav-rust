//! 菜单栏托盘(macOS)。
//!
//! 设计:docs/rust-migration.md §4.5
//!
//! muda 0.17 API:
//!   - MenuItem::new(text, enabled, accelerator)
//!   - MenuItem::with_id(id, text, enabled, accelerator)
//!   - MenuItem::id() -> &MenuId, set_text(&self, text)
//!   - MenuEvent::set_event_handler(f) 全局回调
//!   - MenuId(pub String),可读 .0

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{mpsc, Notify};
use tracing::{info, warn};

use crate::cookie::{CookieStore, DEFAULT_BROWSERS};
use crate::health::{HealthChecker, HealthLevel};
use crate::mount;
use crate::notifier;

pub struct TrayConfig {
    pub mount_point: PathBuf,
    pub cookies: CookieStore,
    pub health: Arc<HealthChecker>,
    pub shutdown: Arc<Notify>,
}

/// 阻塞,直到用户点退出或 shutdown 触发。
pub async fn run(cfg: TrayConfig) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        run_macos(cfg).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cfg;
        // 非 macOS 不创建托盘;只等 shutdown
        tokio::time::sleep(Duration::from_secs(u64::MAX / 2)).await;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
async fn run_macos(cfg: TrayConfig) -> Result<()> {
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tray_icon::TrayIconBuilder;

    const ID_REFRESH: &str = "refresh";
    const ID_REVEAL: &str = "reveal";
    const ID_QUIT: &str = "quit";

    let menu = Menu::new();

    let status_item = MenuItem::new("状态:启动中...", false, None);
    let refresh_item = MenuItem::with_id(ID_REFRESH, "立即刷新 cookie", true, None);
    let reveal_item = MenuItem::with_id(ID_REVEAL, "在 Finder 中显示", true, None);
    let quit_item = MenuItem::with_id(ID_QUIT, "退出", true, None);

    menu.append_items(&[
        &status_item,
        &PredefinedMenuItem::separator(),
        &refresh_item,
        &reveal_item,
        &PredefinedMenuItem::separator(),
        &quit_item,
    ])
    .map_err(|e| anyhow::anyhow!("tray menu: {e}"))?;

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("QuarkDrive WebDAV")
        .build()
        .map_err(|e| anyhow::anyhow!("tray build: {e}"))?;

    // muda 全局事件回调(非 async)
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<TrayCmd>();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |ev: tray_icon::menu::MenuEvent| {
        let id = ev.id().0.as_str();
        let cmd = match id {
            ID_REFRESH => Some(TrayCmd::Refresh),
            ID_REVEAL => Some(TrayCmd::Reveal),
            ID_QUIT => Some(TrayCmd::Quit),
            _ => None,
        };
        if let Some(c) = cmd {
            let _ = cmd_tx.send(c);
        }
    }));

    let cookies = cfg.cookies.clone();
    let health = cfg.health.clone();
    let mount_point = cfg.mount_point.clone();
    let shutdown = cfg.shutdown.clone();

    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let r = health.check().await;
                let label = match r.level {
                    HealthLevel::Healthy => "状态:在线",
                    HealthLevel::CookieMissing => "状态:Cookie 失效",
                    HealthLevel::WebDavUnreachable => "状态:WebDAV 离线",
                    HealthLevel::MountMissing => "状态:未挂载",
                };
                status_item.set_text(label);
            }
            Some(cmd) = cmd_rx.recv() => match cmd {
                TrayCmd::Refresh => {
                    info!("tray: refresh cookies");
                    match CookieStore::from_chromium(DEFAULT_BROWSERS).await {
                        Ok(s) => {
                            cookies.replace(s.snapshot());
                            notifier::notify("QuarkDrive", "Cookie 已刷新");
                        }
                        Err(e) => {
                            warn!(error = %e, "manual refresh failed");
                            notifier::notify("QuarkDrive", &format!("刷新失败: {e}"));
                        }
                    }
                }
                TrayCmd::Reveal => {
                    let _ = std::process::Command::new("/usr/bin/open")
                        .arg(&mount_point).status();
                }
                TrayCmd::Quit => {
                    info!("tray: quit");
                    let _ = mount::unmount(&mount_point).await;
                    shutdown.notify_waiters();
                    return Ok(());
                }
            },
            else => break,
        }
    }
    Ok(())
}

#[derive(Debug)]
enum TrayCmd {
    Refresh,
    Reveal,
    Quit,
}
