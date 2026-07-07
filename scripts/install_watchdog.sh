#!/usr/bin/env bash
# install_watchdog.sh
# 把 watchdog_mount.sh 注册为 LaunchAgent，开机自启，登录后跑。
# 用法：bash scripts/install_watchdog.sh [--uninstall]
set -eu
LABEL="com.localquark.mount-watchdog"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
WATCHDOG="$(cd "$(dirname "$0")" && pwd)/watchdog_mount.sh"

if [[ "${1:-}" == "--uninstall" ]]; then
    launchctl bootout "gui/$(id -u)/${LABEL}" 2>/dev/null || true
    rm -f "$PLIST"
    echo "uninstalled: ${LABEL}"
    exit 0
fi

mkdir -p "${HOME}/Library/LaunchAgents"
cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>${LABEL}</string>
<key>ProgramArguments</key>
<array>
<string>/bin/bash</string><string>${WATCHDOG}</string>
</array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>ProcessType</key><string>Background</string>
<key>StandardOutPath</key><string>${HOME}/Library/Logs/LocalQuark-rust-watchdog.log</string>
<key>StandardErrorPath</key><string>${HOME}/Library/Logs/LocalQuark-rust-watchdog.log</string>
</dict></plist>
PLIST

launchctl bootout "gui/$(id -u)/${LABEL}" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
launchctl enable  "${LABEL}"
launchctl kickstart -k "gui/$(id -u)/${LABEL}" 2>/dev/null || true
echo "installed: ${PLIST}"
echo "uninstall: bash scripts/install_watchdog.sh --uninstall"
