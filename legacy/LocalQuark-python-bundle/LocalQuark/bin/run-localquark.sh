#!/usr/bin/env bash
# Stage 12b.2: end-to-end launcher. Stage 13a.1: also the launchd
# ProgramArguments entry point -- when the launchd agent invokes this
# script, we MUST stay alive for the lifetime of the webdav child so
# launchd sees a meaningful exit code at teardown / crash time.
#
#   1. setup-tls.sh (cert + keychain trust)
#   2. start quarkdrive-webdav in the background with --tls-cert-dir
#   3. wait for the listener to come up
#   4. (macOS) mount_webdav to $LOCALQUARK_MOUNT_POINT (best-effort)
#   5. block on the child until it exits, then mirror the lifecycle
#      decision to launchd via exit code + teardown marker
#
# Mount failure is non-fatal: webdavfs_agent depends on GUI session,
# keychain prompt state, and caller-side macOS permissions that we
# cannot control from a CLI launcher. The HTTPS server stays up.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCALQUARK_REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-common.sh
source "$SCRIPT_DIR/lib-common.sh"

# 1. cert
"$SCRIPT_DIR/setup-tls.sh"

# 2. start
ensure_app_dir
BIN="$(resolve_webdav_bin)"

# If QUARK_COOKIE is not set via environment, try to load from cookies.json
if [ -z "${LOCALQUARK_QUARK_COOKIE:-}" ]; then
    COOKIE_FILE="$LOCALQUARK_REPO_ROOT/quark_cookie/cookies.json"
    if [ -f "$COOKIE_FILE" ]; then
        LOCALQUARK_QUARK_COOKIE=$(python3 -c "
import json
with open('$COOKIE_FILE') as f:
    data = json.load(f)
    print('; '.join([f'{k}={v}' for k, v in data.items()]))
        ")
        export LOCALQUARK_QUARK_COOKIE
    fi
fi

# Auth + cookie args: only pass the ones the operator set. The
# `${ARR[@]+"${ARR[@]}"}` form is required under `set -u` to expand
# to nothing when the array is empty (bash 3.2 treats an unset array
# as a hard error; the `+` indirection turns "unset-or-empty" into
# "skip this expansion").
AUTH_ARGS=()
[ -n "${LOCALQUARK_AUTH_USER:-}" ] && AUTH_ARGS+=(--auth-user "$LOCALQUARK_AUTH_USER")
[ -n "${LOCALQUARK_AUTH_PASSWORD:-}" ] && AUTH_ARGS+=(--auth-password "$LOCALQUARK_AUTH_PASSWORD")
[ -n "${LOCALQUARK_QUARK_COOKIE:-}" ] && AUTH_ARGS+=(--quark-cookie "$LOCALQUARK_QUARK_COOKIE")

log "starting $BIN on $LOCALQUARK_HOST:$LOCALQUARK_PORT (cert-dir $LOCALQUARK_CERT_DIR)"

# --serve-only: the binary has its own built-in mount logic that calls
# `mount_webdav -o url=...` (mount.rs:80). On macOS 27 that flag
# combination is rejected by webdavfs_agent with
# `webdavfs_agent: -o : option not supported` (the `-o url=`
# form was changed in a recent webdavfs release). We do the mount
# ourselves below via the helper (which uses the unprefixed URL +
# keychain-auth pattern that works), so the binary should stay out
# of the mount business. --serve-only keeps the binary to TLS
# serving + webdav protocol only.
#
# nohup + redirect to detach from this shell; disown so SIGTERM to
# the launcher does not propagate to the webdav child.
nohup "$BIN" \
    --serve-only \
    --host "$LOCALQUARK_HOST" \
    --port "$LOCALQUARK_PORT" \
    --tls-cert "$LOCALQUARK_CERT_DIR/cert.pem" \
    --tls-key "$LOCALQUARK_CERT_DIR/key.pem" \
    ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} \
    > "$LOCALQUARK_LOG_FILE" 2>&1 &
WD_PID=$!
disown $WD_PID 2>/dev/null || true
echo "$WD_PID" > "$LOCALQUARK_PID_FILE"
log "started pid=$WD_PID (log=$LOCALQUARK_LOG_FILE)"

# 3. wait for listener
LISTEN_OK=0
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if ! kill -0 "$WD_PID" 2>/dev/null; then
        log "ERROR: webdav process exited before binding; tail of $LOCALQUARK_LOG_FILE:"
        tail -30 "$LOCALQUARK_LOG_FILE" >&2 || true
        exit 1
    fi
    if nc -z "$LOCALQUARK_HOST" "$LOCALQUARK_PORT" 2>/dev/null; then
        LISTEN_OK=1
        break
    fi
    sleep 0.5
done

if [ "$LISTEN_OK" -ne 1 ]; then
    log "ERROR: webdav did not bind $LOCALQUARK_HOST:$LOCALQUARK_PORT within 10s"
    tail -30 "$LOCALQUARK_LOG_FILE" >&2 || true
    exit 1
fi
log "listening on https://$LOCALQUARK_HOST:$LOCALQUARK_PORT"

# 4. mount (macOS only, best-effort)
if [ "$(uname -s)" = "Darwin" ]; then
    # WebDAV URL. We deliberately do NOT embed user:pass here.
    # macOS 27's webdavfs_agent rejects a URL with both userinfo AND
    # resource specifier (CFURLComponentUserInfo / ResourceSpecifier
    # both come back kCFNotFound on the embedded form, throwing
    # `IllegalURLComponent` at webdav_network.c:788). Instead, we
    # write the credentials to login keychain as an internet password
    # (server=$LOCALQUARK_HOST, port=$LOCALQUARK_PORT, prot=htps,
    # account=$LOCALQUARK_AUTH_USER) and pass a bare URL to
    # mount_webdav; webdavfs_agent then looks up the password in
    # keychain. This is the same pattern the python setup used on
    # 2026-06-26 (per e2e_runner.log: "密码走 Keychain").
    URL="https://$LOCALQUARK_HOST:$LOCALQUARK_PORT"

    if [ -n "${LOCALQUARK_AUTH_USER:-}" ] && [ -n "${LOCALQUARK_AUTH_PASSWORD:-}" ]; then
        # Best-effort keychain write. If it fails (keychain locked,
        # user not present), the mount will still proceed; webdavfs_agent
        # will then fall back to anonymous auth, which our server
        # rejects with 401, surfacing as a clear failure mode rather
        # than the silent IllegalURLComponent we got when user:pass was
        # in the URL.
        /usr/bin/security add-internet-password \
            -a "$LOCALQUARK_AUTH_USER" \
            -s "$LOCALQUARK_HOST" \
            -P "$LOCALQUARK_PORT" \
            -r htps \
            -w "$LOCALQUARK_AUTH_PASSWORD" \
            -A \
            ~/Library/Keychains/login.keychain-db \
            2>>"$LOCALQUARK_LOG_FILE" || true
    fi

    if mount | grep -q " on $LOCALQUARK_MOUNT_POINT ("; then
        log "already mounted at $LOCALQUARK_MOUNT_POINT"
    elif helper_available; then
        # 13b.2 path: helper runs as root and can mkdir /Volumes/* +
        # drive webdavfs_agent from outside the sandbox. We still
        # treat any helper failure as non-fatal: the HTTPS server is
        # the actual product, the mount is a convenience.

        # Stage 14.2: macOS's webdavfs_agent refuses to mount an https
        # URL whose cert is not in System keychain. The cert generated
        # by setup-tls.sh is fresh on every cold start, so the user
        # has to trust it once per cert. The helper (>= 0.3.0) can
        # add the cert to System keychain on our behalf because it
        # runs as root in the LaunchDaemon domain; the LaunchAgent
        # context cannot perform this write because adding to System
        # keychain requires sudo. Best-effort: if the helper is
        # older than 0.3.0 we skip the trust step and the user can
        # run install-helper.sh manually later.
        if helper_has_trust_cert; then
            CERT_FILE="$LOCALQUARK_CERT_DIR/cert.pem"
            if [ -f "$CERT_FILE" ]; then
                if /usr/bin/security verify-cert -c "$CERT_FILE" >/dev/null 2>&1; then
                    log "cert already trusted, skipping helper trust-cert"
                else
                    log "helper trust-cert $CERT_FILE"
                    if "$SCRIPT_DIR/helper-client.sh" trust-cert "$CERT_FILE" \
                            2>>"$LOCALQUARK_LOG_FILE"; then
                        log "trust-cert ok"
                    else
                        RC=$?
                        log "WARN: helper trust-cert failed (rc=$RC); mount may fail with cert error"
                    fi
                fi
            else
                log "WARN: $CERT_FILE not found; skipping trust-cert"
            fi
        else
            log "helper < 0.3.0 (no trust-cert); install new helper to auto-trust self-signed cert"
        fi

        log "helper detected; preparing mount point and mounting as user"
        if "$SCRIPT_DIR/helper-client.sh" mkdir "$LOCALQUARK_MOUNT_POINT" \
                2>>"$LOCALQUARK_LOG_FILE"; then
            UID_VAL="$(id -u)"
            GID_VAL="$(id -g)"
            if "$SCRIPT_DIR/helper-client.sh" chmod "$LOCALQUARK_MOUNT_POINT" \
                    "$UID_VAL" "$GID_VAL" \
                    2>>"$LOCALQUARK_LOG_FILE"; then
                log "mkdir and chmod ok; mounting via user mount_webdav"
                if mount_webdav -s "$URL" "$LOCALQUARK_MOUNT_POINT" \
                        2>>"$LOCALQUARK_LOG_FILE"; then
                    log "mount ok (via user mount_webdav)"
                else
                    RC=$?
                    log "WARN: user mount_webdav failed (rc=$RC); HTTPS server is still up at https://$LOCALQUARK_HOST:$LOCALQUARK_PORT"
                fi
            else
                RC=$?
                log "WARN: helper chmod failed (rc=$RC); skipping mount"
            fi
        else
            RC=$?
            log "WARN: helper mkdir failed (rc=$RC); HTTPS server is still up at https://$LOCALQUARK_HOST:$LOCALQUARK_PORT"
        fi
    else
        # 13a best-effort path: unchanged. mkdir may fail under
        # sandbox / non-admin launchd context (Permission denied on
        # /Volumes/<name>); if mkdir succeeded, mount_webdav runs from
        # the user agent and may also fail. Either failure is
        # non-fatal.
        log "helper not installed; mounting via direct mount_webdav (best-effort)"
        if mkdir -p "$LOCALQUARK_MOUNT_POINT" 2>/dev/null; then
            if mount_webdav -s "$URL" "$LOCALQUARK_MOUNT_POINT" \
                    2>>"$LOCALQUARK_LOG_FILE"; then
                log "mount ok"
            else
                RC=$?
                log "WARN: mount_webdav failed (rc=$RC); HTTPS server is still up at https://$LOCALQUARK_HOST:$LOCALQUARK_PORT"
            fi
        else
            log "WARN: cannot create $LOCALQUARK_MOUNT_POINT (sandbox/non-admin); skipping mount. HTTPS server still up at https://$LOCALQUARK_HOST:$LOCALQUARK_PORT"
        fi
    fi
else
    log "non-Darwin host, skipping mount_webdav"
fi

log "ready. log=$LOCALQUARK_LOG_FILE pid=$LOCALQUARK_PID_FILE mount=$LOCALQUARK_MOUNT_POINT"

# Stage 13c.1: detach mode for .app launcher. If LOCALQUARK_DETACH=1,
# exit here so the launcher process can return to macOS immediately
# while the webdav child keeps running in the background. The launchd
# / launchctl path stays untouched (it needs the blocking wait below
# so launchd sees the webdav lifetime).
if [ "${LOCALQUARK_DETACH:-0}" = "1" ]; then
    log "detach=1; webdav stays detached (pid=$WD_PID); exiting launcher"
    exit 0
fi

# 5. Stay alive until the webdav child exits. launchd watches this
# launcher script (per plist ProgramArguments), and KeepAlive:
# SuccessfulExit: false means launchd only restarts us on a non-zero
# exit. With the disown above, macOS would otherwise reap webdav when
# the launcher exits; blocking here keeps the two lifecycles aligned
# so that a crash here -> exit non-zero -> launchd restart, and a
# clean teardown -> marker present -> exit 0 -> launchd stays quiet.
#
# Stage 13a.1 bug G: bash 3.2 on macOS does NOT honor \`wait $PID\` for
# a disowned + nohup'd child -- wait returns rc=0 immediately while
# the child is still running (we observed ps -ef showing the webdav
# pid alive *and* curl probing 200s while launcher logged "webdav
# exited (rc=0)"). The fix is a polling loop on \`kill -0\` instead.
# We do not need the real exit status: the teardown-marker file is
# the only signal we act on.
while kill -0 "$WD_PID" 2>/dev/null; do
    sleep 1
done
# Stage 13a.1 bug H: bash \`while COND; do BODY; done\` returns the
# exit status of the last BODY command (here \`sleep 1\` -> 0), not the
# exit status of the failing COND. Re-probe \`kill -0\` to distinguish
# "webdav died on its own" (rc=1, launchd should restart) from any
# future path that exits the loop with webdav still alive (rc=0).
if kill -0 "$WD_PID" 2>/dev/null; then
    WD_RC=0
else
    WD_RC=1
fi
if [ -f "$LOCALQUARK_APP_DIR/.launchd-teardown-marker" ]; then
    log "teardown marker present; exiting clean (rc=0)"
    rm -f "$LOCALQUARK_APP_DIR/.launchd-teardown-marker"
    exit 0
fi
log "webdav exited (rc=$WD_RC); mirroring to launchd for restart decision"
exit "$WD_RC"
