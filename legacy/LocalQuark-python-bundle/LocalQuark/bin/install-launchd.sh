#!/usr/bin/env bash
# Stage 13a.1: install / dry-run / restart / uninstall launcher for
# the com.localquark.webdav launchd agent. Replaces the 11B-4-era
# com.localquark.refresher plist which is retired inline.
#
# Modes:
#   install-launchd.sh                # default: install if absent (idempotent)
#   install-launchd.sh --dry-run      # log intended actions, no side effects
#   install-launchd.sh --restart      # bootstrap + kickstart -k (force restart)
#   install-launchd.sh --remove-legacy # bootout + rm legacy plist only
#   install-launchd.sh --uninstall    # delegate to uninstall-launchd.sh
#
# Idempotent: re-running default mode on an already-loaded agent logs
# "already loaded, no-op" and exits 0 (C-11 contract from
# docs/13a-design.md).

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCALQUARK_REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-common.sh
source "$SCRIPT_DIR/lib-common.sh"

MODE="install"
DRY_RUN=0
while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run)        DRY_RUN=1 ;;
        --restart)        MODE="restart" ;;
        --uninstall)      exec "$SCRIPT_DIR/uninstall-launchd.sh" ;;
        --remove-legacy)  MODE="remove-legacy" ;;
        -h|--help)
            sed -n '2,20p' "$0"
            exit 0 ;;
        *)
            log "unknown arg: $1"
            exit 2 ;;
    esac
    shift
done

LAUNCH_AGENTS_DIR="$(resolve_launch_agents_dir)"
LOG_DIR="$(resolve_log_dir)"
UID_GUI="$(resolve_uid_gui)"
PLIST_FILE="$LAUNCH_AGENTS_DIR/com.localquark.webdav.plist"
PLIST_TMPL="$SCRIPT_DIR/com.localquark.webdav.plist.tmpl"
LEGACY_PLIST="$(detect_legacy_refresher_plist)"

# Pre-flight: the plist template ships with the repo and must exist.
if [ ! -f "$PLIST_TMPL" ]; then
    log "ERROR: plist template not found at $PLIST_TMPL"
    exit 1
fi

# action() runs a shell command, or logs it under --dry-run.
action() {
    if [ "$DRY_RUN" -eq 1 ]; then
        log "[dry-run] $*"
    else
        eval "$@"
    fi
}

# Print 0 if the agent is currently loaded into the user's launchd
# gui domain, 1 otherwise. We probe via `launchctl print` because the
# legacy `launchctl list | grep <label>` test misses Background
# agents whose label is hidden in macOS 13+.
agent_loaded() {
    if launchctl print "$UID_GUI/com.localquark.webdav" >/dev/null 2>&1; then
        return 0
    fi
    return 1
}

# Render the plist template into ~/Library/LaunchAgents/. Idempotent
# under sed: re-running produces identical output. We use a tmp-file +
# rename to avoid leaving a half-written plist on disk if the
# launcher is interrupted mid-write.
render_plist() {
    if [ "$DRY_RUN" -eq 1 ]; then
        log "[dry-run] would render $PLIST_TMPL -> $PLIST_FILE"
        return 0
    fi
    mkdir -p "$LAUNCH_AGENTS_DIR" "$LOG_DIR"
    # Stage the (potentially multi-line) optional env block in a
    # tmpfile so awk can pull it in with getline. BSD awk's `-v`
    # does not accept embedded newlines, hence this indirection.
    local env_file plist_tmp
    env_file="$(mktemp -t localquark-env.XXXXXX)"
    plist_tmp="$PLIST_FILE.tmp"
    inject_optional_env > "$env_file"
    sed -e "s|__REPO_ROOT__|$LOCALQUARK_REPO_ROOT|g" \
        -e "s|__HOME__|$HOME|g" \
        -e "s|__PORT__|$LOCALQUARK_PORT|g" \
        "$PLIST_TMPL" \
        | awk -v ENV_FILE="$env_file" '
            /<!-- __INJECTED_ENV_BLOCK__ -->/ {
                while ((getline line < ENV_FILE) > 0) print line
                close(ENV_FILE)
                next
            }
            { print }
        ' > "$plist_tmp"
    rm -f "$env_file"
    mv "$plist_tmp" "$PLIST_FILE"
    log "rendered $PLIST_FILE"
}

# Build the optional EnvironmentVariables block from the caller's
# shell. Any LOCALQUARK_LAUNCHD_ENV_<NAME>=<VALUE> sh var becomes a
# plist <key>NAME</key><string>VALUE</string> entry. Use case: dev
# 13a.1 e2e test injects `LOCALQUARK_LAUNCHD_ENV_QUARK_COOKIE=fake`
# so the launchd agent context has a stub credential that satisfies
# the 11B-3 binary's pre-bind check. Production launches should
# keep this empty and rely on cookies.json instead -- a real cookie
# in a plist would be world-readable. Not full XML escape: safe only
# for ASCII token/dev placeholders; refusing values with `<`, `>`,
# or `&`.
# Stage 13a.2 fast-path: returns 0 if re-rendering the plist
# would produce a byte-equal file to the one on disk. Used by install
# mode to skip writing LaunchAgents/com.localquark.webdav.plist when
# nothing has changed (template, env, repo root, home, port). md5 is
# content-based, so mtime / owner / LaunchAgents dir churn do not
# trigger a re-write.
plist_would_be_unchanged() {
    if [ ! -f "$PLIST_FILE" ]; then
        return 1
    fi
    local env_file plist_tmp
    env_file="$(mktemp -t localquark-env.XXXXXX)"
    plist_tmp="$(mktemp -t localquark-plist.XXXXXX)"
    inject_optional_env > "$env_file"
    sed -e "s|__REPO_ROOT__|$LOCALQUARK_REPO_ROOT|g" \
        -e "s|__HOME__|$HOME|g" \
        -e "s|__PORT__|$LOCALQUARK_PORT|g" \
        "$PLIST_TMPL" \
        | awk -v ENV_FILE="$env_file" \
            '/<!-- __INJECTED_ENV_BLOCK__ -->/ {
                while ((getline line < ENV_FILE) > 0) print line
                close(ENV_FILE)
                next
            }
            { print }' \
        > "$plist_tmp"
    rm -f "$env_file"
    local on_disk rendered
    on_disk="$(md5 -q "$PLIST_FILE" 2>/dev/null || true)"
    rendered="$(md5 -q "$plist_tmp" 2>/dev/null || true)"
    rm -f "$plist_tmp"
    [ -n "$on_disk" ] && [ -n "$rendered" ] && [ "$on_disk" = "$rendered" ]
}

inject_optional_env() {
    env | awk '
        /^LOCALQUARK_LAUNCHD_ENV_/ {
            line = $0
            eq = index(line, "=")
            key = substr(line, 1, eq - 1)
            val = substr(line, eq + 1)
            if (key ~ /[^A-Za-z0-9_]/ || val ~ /[<>&\047\042]/) {
                print "# skipped unsafe env key=" key > "/dev/stderr"
                next
            }
            name = substr(key, length("LOCALQUARK_LAUNCHD_ENV_") + 1)
            print "        <key>" name "</key>"
            print "        <string>" val "</string>"
        }
    '
}

# 1. Retire the legacy 11B-4 stub plist if it survived the port. This
# runs in install / restart / remove-legacy modes.
if [ -n "$LEGACY_PLIST" ]; then
    if [ "$MODE" = "install" ] || [ "$MODE" = "restart" ] || [ "$MODE" = "remove-legacy" ]; then
        log "retiring legacy stub agent at $LEGACY_PLIST"
        if [ "$DRY_RUN" -eq 0 ]; then
            launchctl bootout "$UID_GUI" "$LEGACY_PLIST" 2>/dev/null || true
            rm -f "$LEGACY_PLIST"
            log "legacy stub plist removed"
        else
            log "[dry-run] would bootout $UID_GUI/$LEGACY_PLIST and rm it"
        fi
    fi
fi

# 2. Render the plist on disk. install mode renders inline (with
# fast-path) so we can skip the write when nothing has changed.
case "$MODE" in
    restart|remove-legacy)
        render_plist ;;
esac

# 3. Agent action.
case "$MODE" in
    install)
        if agent_loaded; then
            if plist_would_be_unchanged; then
                log "agent loaded + plist hash unchanged; no-op (C-11 fast-path)"
                exit 0
            fi
            log "agent loaded; no-op (C-11 contract; use --restart to pick up plist changes)"
            exit 0
        fi
        render_plist
        action launchctl bootstrap "$UID_GUI" "$PLIST_FILE"
        log "bootstrapped $UID_GUI/com.localquark.webdav"
        log "logs will appear in $LOG_DIR/webdav.{out,err}.log"
        ;;
    restart)
        if agent_loaded; then
            action launchctl kickstart -k "$UID_GUI/com.localquark.webdav"
            log "kickstart -k issued; agent will be restarted by launchd"
        else
            action launchctl bootstrap "$UID_GUI" "$PLIST_FILE"
            log "bootstrapped fresh (was not loaded)"
        fi
        ;;
    remove-legacy)
        log "remove-legacy mode; current plist rendered but not bootstrapped"
        exit 0
        ;;
esac

log "done"
