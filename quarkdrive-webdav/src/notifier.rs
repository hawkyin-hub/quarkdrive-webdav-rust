//! macOS 用户通知:包 osascript,失败静默。

use std::process::Command;
use tracing::warn;

pub fn notify(title: &str, body: &str) {
    let script = format!(
        r#"display notification "{}" with title "{}""#,
        body.replace('"', "'"),
        title.replace('"', "'"),
    );
    let res = Command::new("/usr/bin/osascript").arg("-e").arg(&script).status();
    if let Err(e) = res {
        warn!(error = %e, "osascript spawn failed");
    }
}
