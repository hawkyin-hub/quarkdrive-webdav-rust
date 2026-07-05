#!/usr/bin/env bash
# 卸载 LocalQuark

set -euo pipefail

PLIST="$HOME/Library/LaunchAgents/com.localquark.refresher.plist"
if [[ -f "$PLIST" ]]; then
    launchctl unload "$PLIST" 2>/dev/null || true
    rm -f "$PLIST"
    echo "✅ LaunchAgent 已卸载"
fi

# 卸载 FUSE
MOUNT_POINT="${QUARK_MOUNT_POINT:-$HOME/Mount/Quark}"
if mount | grep -q " on $MOUNT_POINT "; then
    umount "$MOUNT_POINT" 2>/dev/null || diskutil unmount "$MOUNT_POINT" 2>/dev/null || true
    echo "✅ 已卸载 $MOUNT_POINT"
fi

echo "如需彻底删除: rm -rf '$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)'"