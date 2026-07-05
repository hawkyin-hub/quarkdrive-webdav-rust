#!/usr/bin/env bash
# Stage 12b.2: shared helpers for setup-tls.sh, run-localquark.sh,
# teardown-localquark.sh. Source-only -- do not execute directly.
#
# Resolves APP_SUPPORT paths, finds the quarkdrive-webdav binary, and
# exposes log()/ensure_app_dir() helpers. All callers source this file
# and then use $LOCALQUARK_APP_DIR, $LOCALQUARK_CERT_DIR, etc.

set -euo pipefail

# Caller must set LOCALQUARK_REPO_ROOT before sourcing. Each entry
# script sets it to the parent of bin/ via BASH_SOURCE.
: "${LOCALQUARK_REPO_ROOT:?must export LOCALQUARK_REPO_ROOT before sourcing lib-common.sh}"

# APP_SUPPORT path. Override with LOCALQUARK_APP_DIR.
LOCALQUARK_APP_DIR="${LOCALQUARK_APP_DIR:-$HOME/Library/Application Support/LocalQuark}"
LOCALQUARK_CERT_DIR="${LOCALQUARK_CERT_DIR:-$LOCALQUARK_APP_DIR/certs}"
LOCALQUARK_PID_FILE="${LOCALQUARK_PID_FILE:-$LOCALQUARK_APP_DIR/quarkdrive-webdav.pid}"
LOCALQUARK_LOG_FILE="${LOCALQUARK_LOG_FILE:-$LOCALQUARK_APP_DIR/quarkdrive-webdav.log}"
LOCALQUARK_MOUNT_POINT="${LOCALQUARK_MOUNT_POINT:-/Volumes/LocalQuark}"
LOCALQUARK_PORT="${LOCALQUARK_PORT:-8443}"
LOCALQUARK_HOST="${LOCALQUARK_HOST:-127.0.0.1}"

# quarkdrive-webdav binary path. Override with QUARKDRIVE_WEBDAV_BIN.
# Search order: env, then release, then debug (sibling quarkdrive-webdav
# subdir), then $PATH.
QUARKDRIVE_WEBDAV_BIN="${QUARKDRIVE_WEBDAV_BIN:-}"

resolve_webdav_bin() {
    if [ -n "$QUARKDRIVE_WEBDAV_BIN" ]; then
        echo "$QUARKDRIVE_WEBDAV_BIN"
        return 0
    fi
    local cand
    for cand in \
        "$LOCALQUARK_REPO_ROOT/bin/quarkdrive-webdav" \
        "$LOCALQUARK_REPO_ROOT/../../quarkdrive-webdav/target/release/quarkdrive-webdav" \
        "$LOCALQUARK_REPO_ROOT/../../quarkdrive-webdav/target/debug/quarkdrive-webdav" \
        "$LOCALQUARK_REPO_ROOT/../quarkdrive-webdav/target/release/quarkdrive-webdav" \
        "$LOCALQUARK_REPO_ROOT/../quarkdrive-webdav/target/debug/quarkdrive-webdav"
    do
        if [ -x "$cand" ]; then
            echo "$cand"
            return 0
        fi
    done
    if command -v quarkdrive-webdav >/dev/null 2>&1; then
        command -v quarkdrive-webdav
        return 0
    fi
    echo "ERROR: quarkdrive-webdav binary not found" >&2
    echo "       set QUARKDRIVE_WEBDAV_BIN or build it under ../../quarkdrive-webdav/target/" >&2
    return 1
}

ensure_app_dir() {
    mkdir -p "$LOCALQUARK_APP_DIR" "$LOCALQUARK_CERT_DIR"
}

# log() writes a timestamped line to $LOCALQUARK_LOG_FILE AND stderr.
log() {
    local msg="[$(date +%FT%T)] $*"
    if [ -n "${LOCALQUARK_LOG_FILE:-}" ]; then
        echo "$msg" >> "$LOCALQUARK_LOG_FILE"
    fi
    echo "$msg" >&2
}

# Stage 13a.1: launchd helpers. Mac-only; non-Darwin callers must
# not invoke these (they assume ${HOME}/Library/LaunchAgents layout).

# Returns the user-level LaunchAgents directory for the current
# $HOME. Hardcoded to the macOS layout (LaunchDaemons root-owned
# would require sudo; out of scope).
resolve_launch_agents_dir() {
    echo "${HOME}/Library/LaunchAgents"
}

# Returns the Log directory for LocalQuark under ${HOME}/Library/Logs.
# install-launchd.sh creates this on demand before writing the plist
# because launchd refuses to start an agent whose StandardOutPath /
# StandardErrorPath parent is missing.
resolve_log_dir() {
    echo "${HOME}/Library/Logs/LocalQuark"
}

# launchctl gui domain identifier for the current uid, exactly in the
# form `gui/<uid>` expected by `launchctl bootstrap gui/<uid> ...`
# and `launchctl print gui/<uid>/<label>`. Single source of truth so
# install/uninstall stay in lockstep.
resolve_uid_gui() {
    echo "gui/$(id -u)"
}

# Detect the legacy 11B-4 LaunchAgent plist (if it survives from the
# pre-stub era). Prints the absolute path on stdout when present,
# empty otherwise. Read-only -- the caller decides whether to
# bootout / rm. install-launchd.sh invokes the cleanup conditionally
# so the script stays idempotent on a fresh install.
detect_legacy_refresher_plist() {
    local p="${HOME}/Library/LaunchAgents/com.localquark.refresher.plist"
    if [ -f "$p" ]; then
        echo "$p"
    fi
}

# Stage 13b.2: probe whether the privileged helper is reachable via
# the Mach XPC service registered by /Library/LaunchDaemons/
# com.localquark.webdav-helper.plist. Used by run-localquark /
# teardown-localquark to decide between the helper path (root context)
# and the 13a best-effort fallback.
#
# Returns 0 if helper is installed AND the Mach service is accepting
# connections (i.e. `bin/helper-client.sh version` exits 0). Returns
# 1 otherwise -- caller should fall back to 13a direct mount_webdav /
# umount / diskutil.
#
# Note: this calls `helper-client.sh version` which does a real XPC
# round-trip to the helper. Latency is <100ms on a healthy system.
# If the helper has crashed and is being restarted by launchd, the
# XPC lookup will fail and we fall back. That is acceptable: 13a
# behavior is preserved.
helper_available() {
    local client="$SCRIPT_DIR/helper-client.sh"
    [ -x "$client" ] || return 1
    "$client" version >/dev/null 2>&1
}

# helper_has_trust_cert returns 0 iff the installed helper exposes the
# `trust-cert` XPC method (Stage 14.2, helper >= 0.3.0). Used by
# run-localquark.sh to decide whether to invoke the helper for cert
# trust before mount. We probe by inspecting the helper's `client`
# usage string (which lists supported ops on stderr).
helper_has_trust_cert() {
    local client="$SCRIPT_DIR/helper-client.sh"
    [ -x "$client" ] || return 1
    local usage
    usage="$("$client" 2>&1 || true)"
    [[ "$usage" == *"trust-cert"* ]] && return 0
    return 1
}
