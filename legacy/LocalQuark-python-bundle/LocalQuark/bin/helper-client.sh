#!/usr/bin/env bash
# Stage 13b.1: thin client wrapper for run-localquark /
# teardown-localquark. Locates the helper binary (installed by
# install-helper.sh under /Library/PrivilegedHelperTools, falling
# back to the repo build) and exec's it in client mode.
#
# Usage:
#   bin/helper-client.sh mount <url> <mountPoint>
#   bin/helper-client.sh unmount <mountPoint>
#   bin/helper-client.sh mkdir <path>
#   bin/helper-client.sh chmod <path> <uid> <gid>
#   bin/helper-client.sh trust-cert <certPath>   # requires helper >= 0.3.0
#   bin/helper-client.sh version
set -euo pipefail

LABEL="com.localquark.webdav-helper"
INSTALLED="/Library/PrivilegedHelperTools/$LABEL"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_BUILT="$SCRIPT_DIR/../helper/.build/release/LocalQuarkHelper"
# Swift Package builds the helper as LocalQuarkHelper; install-helper.sh
# (sudo) renames it to the launchd label when copying to
# /Library/PrivilegedHelperTools. build-app.sh also uses the launchd
# label when copying into the .app's Resources/helper/ so the bundled
# path matches what the user has installed on disk. helper-client.sh
# tries INSTALLED first (the freshly-installed version), then the repo
# build (developer workflow), then the bundled copy (pre-install).
APP_BUNDLE="$SCRIPT_DIR/../helper/com.localquark.webdav-helper"

if [ -x "$INSTALLED" ]; then
    HELPER_BIN="$INSTALLED"
elif [ -x "$REPO_BUILT" ]; then
    HELPER_BIN="$REPO_BUILT"
elif [ -x "$APP_BUNDLE" ]; then
    HELPER_BIN="$APP_BUNDLE"
else
    echo "helper-client: helper binary not found; run bin/install-helper.sh or helper/build.sh" >&2
    exit 127
fi

exec "$HELPER_BIN" client "$@" # 13b.2 fix: insert "client" subcommand so main.swift dispatches to runClient. See doc-comment above for full context.
