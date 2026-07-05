#!/usr/bin/env bash
# Build, install, restart, and wait for mount — idempotent.
# Use after any rust src/ change to validate an end-to-end deploy.
#
# Requires sudo for installing to /Applications and for killing the
# previously-installed daemon if it was started as root.
set -euo pipefail

ROOT="/Users/HawkSept/myproject/myapp/localquark-rust"
CRATE="$ROOT/quarkdrive-webdav"
APP_SRC="$ROOT/dist/LocalQuark-rust.app"
APP_DST="/Applications/LocalQuark-rust.app"
LOG_WEBDAV="$HOME/Library/Logs/LocalQuark-rust-webdav.log"
LOG_LAUNCH="$HOME/Library/Logs/LocalQuark-rust-launcher.log"

step() { printf "\n\033[1;34m[%s/%s]\033[0m %s\n" "$1" "$TOTAL_STEPS" "$2"; }
TOTAL_STEPS=6

step 1 "Compile release binary (cargo build --release)"
cd "$CRATE"
rtk cargo build --release 2>&1 | tail -20

step 2 "Build .app bundle"
cd "$ROOT"
./scripts/build-app.sh 2>&1 | tail -10

step 3 "Kill old server + unmount"
killall -9 quarkdrive-webdav run-localquark.sh 2>/dev/null || true
sleep 2
osascript -e 'tell application "Finder" to close (every window whose name is "LocalQuark" or POSIX path of (target of it as alias) starts with "/Volumes/LocalQuark")' 2>/dev/null || true
diskutil unmount force /Volumes/LocalQuark 2>/dev/null || true
diskutil unmount force /Volumes/127.0.0.1 2>/dev/null || true
diskutil unmount force /Volumes/Quark 2>/dev/null || true
sleep 1

step 4 "Install to /Applications (sudo)"
sudo rm -rf "$APP_DST"
sudo cp -R "$APP_SRC" "$APP_DST"
sudo chown -R "$(whoami):staff" "$APP_DST"

step 5 "Start app"
open -a LocalQuark-rust

step 6 "Wait for mount"
mounted=0
for i in $(seq 1 30); do
    if mount | grep -E -q "LocalQuark|Quark"; then
        mounted=1
        echo "  ✓ mounted after ${i}s"
        break
    fi
    sleep 1
done

if [ "$mounted" -ne 1 ]; then
    echo "  ✗ mount failed after 30s; tail logs:"
    echo "  --- launcher ---"
    tail -30 "$LOG_LAUNCH" 2>/dev/null || echo "    (no launcher log)"
    echo "  --- webdav ---"
    tail -30 "$LOG_WEBDAV" 2>/dev/null || echo "    (no webdav log)"
    exit 1
fi

echo
echo "✓ Ready. /Volumes/LocalQuark mounted."
echo "  binary mtime : $(stat -f '%Sm  %N' "$APP_DST/Contents/Resources/bin/quarkdrive-webdav")"
echo "  logs         : $LOG_WEBDAV"
