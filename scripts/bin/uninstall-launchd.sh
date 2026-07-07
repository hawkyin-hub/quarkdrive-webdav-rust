#!/usr/bin/env bash
# Stage 13a.1: reverse of install-launchd.sh. Boots out the launchd
# agent, removes the rendered plist, and defensively kills any
# leftover quarkdrive-webdav process that launchd did not clean up
# (e.g. when the previous shutdown was a SIGKILL race).
#
# Idempotent: safe to run when the agent is not loaded or when no
# process is running. Always exits 0 unless a hard failure occurs.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCALQUARK_REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-common.sh
source "$SCRIPT_DIR/lib-common.sh"

LAUNCH_AGENTS_DIR="$(resolve_launch_agents_dir)"
UID_GUI="$(resolve_uid_gui)"
PLIST_FILE="$LAUNCH_AGENTS_DIR/com.localquark.webdav.plist"

# 1. Bootout the agent. launchctl bootout is idempotent on a label
# that is not currently loaded: it returns non-zero but we do not
# treat that as a hard failure because the audit goal is "plist
# and process gone", not "bootout returned 0".
if launchctl print "$UID_GUI/com.localquark.webdav" >/dev/null 2>&1; then
    log "bootout $UID_GUI/com.localquark.webdav"
    launchctl bootout "$UID_GUI/com.localquark.webdav" 2>/dev/null \
        || log "WARN: bootout rc=$?"
else
    log "agent not loaded, nothing to bootout"
fi

# 2. Remove the rendered plist.
if [ -f "$PLIST_FILE" ]; then
    log "rm $PLIST_FILE"
    rm -f "$PLIST_FILE"
else
    log "plist not present at $PLIST_FILE, nothing to remove"
fi

# 3. Defensive kill of the 12b.2 pid-file path (preserved for
# symmetry with teardown-localquark.sh).
if [ -f "$LOCALQUARK_PID_FILE" ]; then
    PID="$(cat "$LOCALQUARK_PID_FILE" 2>/dev/null || true)"
    if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
        log "killing leftover webdav pid=$PID (from pid file)"
        kill "$PID" 2>/dev/null || true
        for i in 1 2 3 4 5; do
            kill -0 "$PID" 2>/dev/null || break
            sleep 0.5
        done
        kill -0 "$PID" 2>/dev/null && kill -9 "$PID" 2>/dev/null || true
    fi
    rm -f "$LOCALQUARK_PID_FILE"
fi

# 4. pgrep fallback: kill any leftover webdav process that launchd
# or nohup may have spawned without writing a pid file (e.g. when
# RunAtLoad was triggered before this script wrote the pid file).
if pgrep -f quarkdrive-webdav >/dev/null 2>&1; then
    log "WARN: pgrep sees quarkdrive-webdav running; SIGTERM"
    pkill -TERM -f quarkdrive-webdav 2>/dev/null || true
    for i in 1 2 3 4 5; do
        pgrep -f quarkdrive-webdav >/dev/null 2>&1 || break
        sleep 0.5
    done
    pgrep -f quarkdrive-webdav >/dev/null 2>&1 \
        && pkill -KILL -f quarkdrive-webdav 2>/dev/null || true
fi

log "torn down (agent plist + leftover processes)"
