#!/usr/bin/env bash
# watchdog_mount.sh
# 常驻监控 /Volumes/LocalQuark：丢失则自动重挂，确保始终在线。
# 用法：
#   ./scripts/watchdog_mount.sh                # 前台（前台时 Ctrl+C 停止）
#   nohup ./scripts/watchdog_mount.sh &        # 后台
#   ./scripts/install_watchdog.sh              # 一键安装为 LaunchAgent（开机自启）
set -u
LOG="${HOME}/Library/Logs/LocalQuark-rust-watchdog.log"
INTERVAL=3
APP="LocalQuark-rust"
MP="/Volumes/LocalQuark"

ts()   { date -Iseconds; }
log()  { printf '%s %s\n' "$(ts)" "$*" | tee -a "$LOG"; }
mounted() { mount | grep -q " on ${MP} "; }

recover() {
    log "MOUNT MISSING — relaunching ${APP}"
    osascript -e "tell application \"${APP}\" to activate" >/dev/null 2>&1 || true
    sleep 8
    if mounted; then log "RECOVERED via activate"; return 0; fi

    log "STILL DOWN — nuking stuck procs and relaunching"
    pkill -9 -f "quarkdrive-webdav.*--serve-only" 2>/dev/null || true
    pkill -9 -f "mount_webdav"                  2>/dev/null || true
    pkill -9 -f "webdavfs_agent"                2>/dev/null || true
    sleep 2
    diskutil unmount force "${MP}"              2>/dev/null || true
    sleep 1
    open -a "${APP}"
    sleep 12
    if mounted; then
        log "RECOVERED via full restart"
        return 0
    fi
    log "STILL DOWN — backend may have cookie/connectivity issue"
    return 1
}

log "start, polling ${MP} every ${INTERVAL}s (PID $$)"
trap 'log "stop"; exit 0' INT TERM
while true; do
    if mounted; then :; else recover || true; fi
    sleep "${INTERVAL}"
done
