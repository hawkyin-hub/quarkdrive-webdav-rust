#!/usr/bin/env bash
# Stage 12b.2: reverse of run-localquark.sh. Unmounts, kills the
# quarkdrive-webdav process, removes the pid file. Idempotent: safe
# to run when nothing is mounted / no process is running.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCALQUARK_REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-common.sh
source "$SCRIPT_DIR/lib-common.sh"

if [ "$(uname -s)" = "Darwin" ]; then
    osascript -e "tell application \"Finder\" to close (every window whose name is \"LocalQuark\" or POSIX path of (target of it as alias) starts with \"$LOCALQUARK_MOUNT_POINT\")" 2>/dev/null || true
fi

if [ "$(uname -s)" = "Darwin" ] && mount | grep -q " on $LOCALQUARK_MOUNT_POINT ("; then
    if helper_available; then
        log "helper detected; unmounting via helper-client"
        if "$SCRIPT_DIR/helper-client.sh" unmount "$LOCALQUARK_MOUNT_POINT" \
                2>>"$LOCALQUARK_LOG_FILE"; then
            log "unmount ok (via helper)"
        else
            RC=$?
            log "WARN: helper unmount failed (rc=$RC); falling back to umount/diskutil"
            if ! umount "$LOCALQUARK_MOUNT_POINT" 2>>"$LOCALQUARK_LOG_FILE"; then
                log "WARN: umount failed; trying diskutil"
                /usr/sbin/diskutil unmount "$LOCALQUARK_MOUNT_POINT" 2>>"$LOCALQUARK_LOG_FILE" || true
            fi
        fi
    else
        log "unmounting $LOCALQUARK_MOUNT_POINT (no helper)"
        if ! umount "$LOCALQUARK_MOUNT_POINT" 2>>"$LOCALQUARK_LOG_FILE"; then
            log "WARN: umount failed; trying diskutil"
            /usr/sbin/diskutil unmount "$LOCALQUARK_MOUNT_POINT" 2>>"$LOCALQUARK_LOG_FILE" || true
        fi
    fi
else
    log "nothing mounted at $LOCALQUARK_MOUNT_POINT, skipping unmount"
fi

if [ -f "$LOCALQUARK_PID_FILE" ]; then
    PID="$(cat "$LOCALQUARK_PID_FILE")"
    if kill -0 "$PID" 2>/dev/null; then
        # Write a teardown marker so the run-localquark.sh launcher
        # (running under launchd) can exit 0 instead of mirroring
        # webdav's SIGTERM-induced 143 exit code. KeepAlive:
        # SuccessfulExit: false in the plist means launchd only
        # restarts on non-zero exit, so 0 keeps the agent quiet.
        touch "$LOCALQUARK_APP_DIR/.launchd-teardown-marker"
        log "killing pid=$PID"
        kill "$PID" 2>/dev/null || true
        for i in 1 2 3 4 5; do
            kill -0 "$PID" 2>/dev/null || break
            sleep 0.5
        done
        if kill -0 "$PID" 2>/dev/null; then
            log "WARN: pid=$PID did not exit gracefully; SIGKILL"
            kill -9 "$PID" 2>/dev/null || true
        fi
    else
        log "pid=$PID from $LOCALQUARK_PID_FILE is already gone"
    fi
    rm -f "$LOCALQUARK_PID_FILE"
else
    log "no pid file at $LOCALQUARK_PID_FILE, nothing to kill"
fi

log "torn down"
