#!/usr/bin/env bash
# Stage 13b.1: reverse of install-helper.sh. Boots out the LaunchDaemon,
# removes the installed binary + plist, clears the version file. Idempotent.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCALQUARK_REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-common.sh
source "$SCRIPT_DIR/lib-common.sh"

LABEL="com.localquark.webdav-helper"
DEST_HELPER="/Library/PrivilegedHelperTools/$LABEL"
DEST_PLIST="/Library/LaunchDaemons/$LABEL.plist"
VERSION_FILE="$LOCALQUARK_APP_DIR/.helper-version"

if launchctl print "system/$LABEL" >/dev/null 2>&1; then
    log "helper: sudo launchctl bootout system/$LABEL"
    sudo launchctl bootout "system/$LABEL" 2>/dev/null || \
        log "WARN: bootout rc=$?"
else
    log "helper: not loaded, nothing to bootout"
fi

if [ -f "$DEST_PLIST" ]; then
    log "helper: sudo rm $DEST_PLIST"
    sudo rm -f "$DEST_PLIST"
fi
if [ -f "$DEST_HELPER" ]; then
    log "helper: sudo rm $DEST_HELPER"
    sudo rm -f "$DEST_HELPER"
fi

if [ -f "$VERSION_FILE" ]; then
    rm -f "$VERSION_FILE"
fi

log "helper: uninstalled"
