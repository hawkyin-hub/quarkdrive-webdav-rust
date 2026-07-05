#!/usr/bin/env bash
# Restart LocalQuark-rust with the freshest known cookies and wait for
# the WebDAV mount to come back. Idempotent.
#
# What it does, in order:
#   1. Stop any running quarkdrive-webdav / run-localquark.sh so the
#      8443 listener frees up.
#   2. Force-unmount /Volumes/LocalQuark (webdavfs_agent holds the
#      vnode; killing the server without unmounting leaves Finder
#      wedged for ~30s).
#   3. Sync the freshest cookies.json (from the python bundle's
#      quark_cookie/ dir, refreshed by the user's last login) into
#      ~/Library/Application Support/LocalQuark/cookies.json so the
#      next launch picks up the new tokens.
#   4. Open the installed .app (double-click entry point in
#      /Applications). It will run run-localquark.sh with the fresh
#      cookies, bind :8443, then mount via the helper.
#   5. Poll the WebDAV PROPFIND for up to 30s and print whether the
#      root listing came back non-empty.
#
# Use when:
#   - 夸克 cookie 过期（启动时硬编码过期的 cookie）
#   - 服务进程卡住 / 挂载点不响应
#   - 想在不重新 cargo build 的情况下让网盘立刻可用
#
# Does NOT recompile. For code changes, run scripts/build_deploy_test.sh
# first, then this script.

set -euo pipefail

ROOT="/Users/HawkSept/myproject/myapp/localquark-rust"
COOKIE_SRC="$ROOT/legacy/LocalQuark-python-bundle/LocalQuark/quark_cookie/cookies.json"
COOKIE_DST="$HOME/Library/Application Support/LocalQuark/cookies.json"
APP="/Applications/LocalQuark-rust.app"
LOG_WEBDAV="$HOME/Library/Logs/LocalQuark-rust-webdav.log"
LOG_LAUNCH="$HOME/Library/Logs/LocalQuark-rust-launcher.log"
MOUNT_POINT="/Volumes/LocalQuark"

step() { printf "\n\033[1;34m[%s/%s]\033[0m %s\n" "$1" "$TOTAL_STEPS" "$2"; }
TOTAL_STEPS=5

# preflight: cookie source must exist
if [ ! -f "$COOKIE_SRC" ]; then
    echo "ERROR: fresh cookie source not found: $COOKIE_SRC" >&2
    echo "       Log in to Quark in the browser and let the python helper refresh it." >&2
    exit 1
fi

# 1. kill old server
step 1 "Kill old quarkdrive-webdav / run-localquark.sh"
killall -9 quarkdrive-webdav run-localquark.sh 2>/dev/null || true
sleep 2

# 2. unmount stale webdavfs_agent vnode
step 2 "Force-unmount $MOUNT_POINT (kills webdavfs_agent)"
diskutil unmount force "$MOUNT_POINT" 2>/dev/null || true
killall -9 webdavfs_agent 2>/dev/null || true
sleep 1

# 3. sync fresh cookies
step 3 "Sync fresh cookies.json -> $COOKIE_DST"
mkdir -p "$(dirname "$COOKIE_DST")"
cp "$COOKIE_SRC" "$COOKIE_DST"
chmod 0600 "$COOKIE_DST"
echo "  cookie mtime : $(stat -f '%Sm' "$COOKIE_DST")"

# 4. start .app
step 4 "Launch $APP"
if [ ! -d "$APP" ]; then
    echo "ERROR: $APP not installed. Run scripts/build_deploy_test.sh first." >&2
    exit 1
fi
open -a "$APP"

# 5. wait for mount + verify PROPFIND returns non-empty listing
step 5 "Wait for WebDAV listener + mount (up to 30s)"
mounted=0
for i in $(seq 1 30); do
    if mount | grep -q " on $MOUNT_POINT ("; then
        mounted=1
        echo "  mounted after ${i}s"
        break
    fi
    sleep 1
done

if [ "$mounted" -ne 1 ]; then
    echo "  mount did not appear in 30s. tail logs:"
    echo "  --- launcher ---"
    tail -30 "$LOG_LAUNCH" 2>/dev/null || echo "    (no launcher log)"
    echo "  --- webdav ---"
    tail -30 "$LOG_WEBDAV" 2>/dev/null || echo "    (no webdav log)"
    exit 1
fi

LISTING=""
for i in $(seq 1 15); do
    LISTING="$(curl -k -s -m 10 -X PROPFIND "https://127.0.0.1:8443/" \
        -H "Depth: 1" \
        -u "ujRx4Js1D:DVzv3ELQ3icn" 2>/dev/null || true)"
    if echo "$LISTING" | grep -q "<D:response>"; then
        n=$(echo "$LISTING" | grep -c "<D:response>")
        echo "  PROPFIND ok : $n entries at root"
        echo
        echo "Ready. $MOUNT_POINT mounted, root listing non-empty."
        echo "  binary      : $APP/Contents/Resources/bin/quarkdrive-webdav"
        echo "  webdav log  : $LOG_WEBDAV"
        exit 0
    fi
    sleep 1
done

echo "  PROPFIND did not return a multistatus within 15s. tail webdav log:"
tail -40 "$LOG_WEBDAV" 2>/dev/null || echo "    (no webdav log)"
exit 1
