#!/usr/bin/env bash
# Stage 13a.2: snapshot the launchd agent + listener + on-disk files
# for com.localquark.webdav. Read-only -- never mutates state.
#
#   bin/status-localquark.sh            # human-readable
#   bin/status-localquark.sh --json     # machine-readable, one dict
#
# Always exits 0. Any "anomaly" (agent not running, marker left over,
# listener down, etc.) is expressed in the JSON fields, not via exit
# code, so CI / monitoring scripts can grep the dict instead of trap.
#
# Fields:
#   agent.label / loaded / state / runs / pid / last_exit_code
#   listener.reachable / http_code
#   fs.plist_present / pidfile_present / pidfile_pid /
#      teardown_marker_present / legacy_refresher_present
#   webdav.binary_alive

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCALQUARK_REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-common.sh
source "$SCRIPT_DIR/lib-common.sh"

MODE="human"
for arg in "$@"; do
    case "$arg" in
        --json) MODE="json" ;;
        -h|--help)
            sed -n '2,15p' "$0"
            exit 0 ;;
        *) log "unknown arg: $arg"; exit 2 ;;
    esac
done

LABEL="com.localquark.webdav"
UID_GUI="$(resolve_uid_gui)"
PLIST_FILE="$(resolve_launch_agents_dir)/$LABEL.plist"
LEGACY_PLIST="$(detect_legacy_refresher_plist)"
TEARDOWN_MARKER="$LOCALQUARK_APP_DIR/.launchd-teardown-marker"
PIDFILE="$LOCALQUARK_PID_FILE"

# 1. agent (launchctl)
agent_loaded=0
agent_state="unknown"
agent_runs=null
agent_pid=null
agent_last_exit=null
agent_dump=""
if launchctl print "$UID_GUI/$LABEL" >/dev/null 2>&1; then
    agent_loaded=1
    agent_dump="$(launchctl print "$UID_GUI/$LABEL" 2>&1 || true)"
    # parse top-level scalar fields. We don't try to parse the
    # environment block -- that would need a real plist reader.
    agent_state="$(printf '%s\n' "$agent_dump" | awk -F'= ' '/^	state = /{gsub(/^[[:space:]]+/,"",$1); print $2; exit}')"
    [ -z "$agent_state" ] && agent_state="unknown"
    _runs="$(printf '%s\n' "$agent_dump" | awk -F'= ' '/^	runs = /{print $2; exit}')"
    [ -n "$_runs" ] && agent_runs="$_runs"
    _pid="$(printf '%s\n' "$agent_dump" | awk -F'= ' '/^	pid = /{print $2; exit}')"
    [ -n "$_pid" ] && agent_pid="$_pid"
    _le="$(printf '%s\n' "$agent_dump" | awk -F'= ' '/^	last exit code = /{print $2; exit}')"
    # launchd prints "(never exited)" as a placeholder when the launcher
    # has never run to completion. JSON requires a literal number or
    # null, so coerce non-numeric placeholders to null. The raw value
    # is kept for the human-mode output so on-call can still see the
    # launchd wording.
    case "$_le" in
        ''|*[!0-9]*) ;;     # empty / "(never exited)" / future placeholders -> null
        *) agent_last_exit="$_le" ;;
    esac
    agent_last_exit_human="${_le:-null}"
fi

# 2. listener (curl probe)
listener_reachable=0
listener_code="000"
# curl writes the %{http_code} placeholder to stdout even when the
# connection refused / TLS handshake failed, so we never need a
# `|| echo 000` fallback (that would double-write "000").
_code="$(curl -k -s --max-time 2 -o /dev/null -w '%{http_code}' \
    "https://$LOCALQUARK_HOST:$LOCALQUARK_PORT/" 2>/dev/null || true)"
# strip trailing newline; "" or "000" means unreachable.
_code="${_code%$'\n'}"
[ -n "$_code" ] && listener_code="$_code"
case "$listener_code" in
    000|"") listener_reachable=0 ;;
    *)      listener_reachable=1 ;;
esac

# 3. filesystem
plist_present=0
[ -f "$PLIST_FILE" ] && plist_present=1
pidfile_present=0
pidfile_pid=null
if [ -f "$PIDFILE" ]; then
    pidfile_present=1
    _pid="$(cat "$PIDFILE" 2>/dev/null || true)"
    [ -n "$_pid" ] && pidfile_pid="$_pid"
fi
teardown_marker_present=0
[ -f "$TEARDOWN_MARKER" ] && teardown_marker_present=1
legacy_refresher_present=0
[ -n "$LEGACY_PLIST" ] && legacy_refresher_present=1

# 4. webdav binary liveness
webdav_binary_alive=0
if [ "$pidfile_present" -eq 1 ] && [ "$pidfile_pid" != "null" ]; then
    if kill -0 "$pidfile_pid" 2>/dev/null; then
        webdav_binary_alive=1
    fi
fi

# 5. emit
if [ "$MODE" = "json" ]; then
    # null is real JSON null; bash arithmetic values map directly.
    cat <<JSON_EOF
{
  "agent": {
    "label": "$LABEL",
    "loaded": $agent_loaded,
    "state": "$agent_state",
    "runs": ${agent_runs:-null},
    "pid": ${agent_pid:-null},
    "last_exit_code": ${agent_last_exit:-null}
  },
  "listener": {
    "host": "$LOCALQUARK_HOST",
    "port": $LOCALQUARK_PORT,
    "reachable": $listener_reachable,
    "http_code": "$listener_code"
  },
  "fs": {
    "plist_present": $plist_present,
    "plist_path": "$(resolve_launch_agents_dir)/$LABEL.plist",
    "pidfile_present": $pidfile_present,
    "pidfile_pid": ${pidfile_pid:-null},
    "pidfile_path": "$PIDFILE",
    "teardown_marker_present": $teardown_marker_present,
    "teardown_marker_path": "$TEARDOWN_MARKER",
    "legacy_refresher_present": $legacy_refresher_present,
    "legacy_refresher_path": "${LEGACY_PLIST:-}"
  },
  "webdav": {
    "binary_alive": $webdav_binary_alive
  }
}
JSON_EOF
    exit 0
fi

# human mode
cat <<HUMAN_EOF
=== LocalQuark launchd status ($LABEL) ===
agent:
  loaded              = $agent_loaded
  state               = $agent_state
  runs                = ${agent_runs:-null}
  pid                 = ${agent_pid:-null}
  last exit code      = $agent_last_exit_human
listener:
  url                 = https://$LOCALQUARK_HOST:$LOCALQUARK_PORT/
  reachable           = $listener_reachable
  http_code           = $listener_code
fs:
  plist               = $PLIST_FILE (present=$plist_present)
  pidfile             = $PIDFILE (present=$pidfile_present, pid=${pidfile_pid:-null})
  teardown marker     = $TEARDOWN_MARKER (present=$teardown_marker_present)
  legacy refresher    = ${LEGACY_PLIST:-<absent>} (present=$legacy_refresher_present)
webdav:
  binary alive        = $webdav_binary_alive
HUMAN_EOF
