#!/usr/bin/env bash
# Build dist/LocalQuark-rust.app from the in-tree sources.
# Idempotent -- safe to re-run after edits.
#
# Layout it produces:
#   dist/LocalQuark-rust.app/Contents/
#     Info.plist
#     PkgInfo
#     MacOS/LocalQuark                # bash launcher, double-click entry
#     Resources/
#       bin/
#         run-localquark.sh           # bootstrap + block (used by launchd)
#         teardown-localquark.sh
#         lib-common.sh               # .app-aware paths
#         setup-tls.sh
#         helper-client.sh
#         quarkdrive-webdav           # the actual Rust server
#       certs/                        # populated on first launch
#       helper/
#         com.localquark.webdav-helper
#         Resources/com.localquark.webdav-helper.plist
#
# Source paths (override via env if needed):
#   WEBDAV_BIN      = quarkdrive-webdav release binary
#   HELPER_BIN      = LocalQuarkHelper v0.2.0
#   HELPER_PLIST_SRC
#   SCRIPTS_SRC     = LocalQuark/LocalQuark/bin
#   OUT_APP         = dist/LocalQuark-rust.app

set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd .. && pwd)"

WEBDAV_BIN="${WEBDAV_BIN:-$REPO_ROOT/quarkdrive-webdav/target/release/quarkdrive-webdav}"
if [ -z "${HELPER_BIN:-}" ]; then
    if [ -f "$REPO_ROOT/helper/.build/arm64-apple-macosx/release/LocalQuarkHelper" ]; then
        HELPER_BIN="$REPO_ROOT/helper/.build/arm64-apple-macosx/release/LocalQuarkHelper"
    else
        HELPER_BIN="$REPO_ROOT/helper/.build/release/LocalQuarkHelper"
    fi
fi
HELPER_PLIST_SRC="${HELPER_PLIST_SRC:-$REPO_ROOT/helper/Resources/com.localquark.webdav-helper.plist}"
SCRIPTS_SRC="${SCRIPTS_SRC:-$REPO_ROOT/scripts/bin}"
OUT_APP="${OUT_APP:-$REPO_ROOT/dist/LocalQuark-rust.app}"

# pre-flight
for f in "$WEBDAV_BIN" "$HELPER_BIN" "$HELPER_PLIST_SRC" "$SCRIPTS_SRC/lib-common.sh" "$SCRIPTS_SRC/run-localquark.sh" "$SCRIPTS_SRC/helper-client.sh" "$SCRIPTS_SRC/setup-tls.sh"; do
    [ -e "$f" ] || { echo "ERROR: missing $f" >&2; exit 1; }
done

APP_DIR="$OUT_APP"
CONTENTS_DIR="$APP_DIR/Contents"
RES_DIR="$CONTENTS_DIR/Resources"
BIN_DIR="$RES_DIR/bin"
CERTS_DIR="$RES_DIR/certs"
HELPER_DIR="$RES_DIR/helper"

echo "==> clean $APP_DIR"
rm -rf "$APP_DIR"
mkdir -p "$CONTENTS_DIR/MacOS" "$BIN_DIR" "$CERTS_DIR" "$HELPER_DIR/Resources"

# 1) Info.plist
cat > "$CONTENTS_DIR/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDisplayName</key>
    <string>LocalQuark-rust</string>
    <key>CFBundleExecutable</key>
    <string>LocalQuark</string>
    <key>CFBundleIdentifier</key>
    <string>com.hawkyin.localquark-rust</string>
    <key>CFBundleName</key>
    <string>LocalQuark-rust</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>2.0.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSAppTransportSecurity</key>
    <dict>
        <key>NSAllowsArbitraryLoads</key>
        <true/>
    </dict>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
PLIST

# 2) PkgInfo (classic 4-byte creator + type)
printf 'APPL????' > "$CONTENTS_DIR/PkgInfo"

# 3) MacOS/LocalQuark bash launcher
cat > "$CONTENTS_DIR/MacOS/LocalQuark" <<'LAUNCHER'
#!/usr/bin/env bash
# LocalQuark-rust .app entry point.
#
# macOS runs this script when the user double-clicks (or `open`s) the
# .app. We resolve the bundle path, hand the bootstrap off to
# run-localquark.sh in the background, then exit so macOS releases
# the .app. The webdav child is started with `nohup ... & disown`
# inside run-localquark.sh, so it survives our exit.

set -euo pipefail
SELF="${BASH_SOURCE[0]:-$0}"
EXEC_DIR="$(cd "$(dirname "$SELF")" && pwd)"
APP_DIR="$(cd "$EXEC_DIR/../.." && pwd)"
RES_DIR="$APP_DIR/Contents/Resources"
BIN_DIR="$RES_DIR/bin"
START_LOG="$HOME/Library/Logs/LocalQuark-rust-launcher.log"

mkdir -p "$(dirname "$START_LOG")"

# Default WebDAV creds (override via env or LocalQuark/cookies.json)
export LOCALQUARK_HOST="${LOCALQUARK_HOST:-127.0.0.1}"
export LOCALQUARK_PORT="${LOCALQUARK_PORT:-8443}"
export LOCALQUARK_AUTH_USER="${LOCALQUARK_AUTH_USER:-ujRx4Js1D}"
export LOCALQUARK_AUTH_PASSWORD="${LOCALQUARK_AUTH_PASSWORD:-DVzv3ELQ3icn}"
export LOCALQUARK_MOUNT_POINT="${LOCALQUARK_MOUNT_POINT:-/Volumes/LocalQuark}"
# .app launcher exits after bootstrap; webdav child keeps running.
export LOCALQUARK_DETACH=1
# repo root + SCRIPT_DIR pinned to the bundle so lib-common.sh / setup-tls.sh /
# run-localquark.sh resolve the helper, certs, binary inside the .app.
export LOCALQUARK_REPO_ROOT="$RES_DIR"
export SCRIPT_DIR="$BIN_DIR"
export QUARKDRIVE_WEBDAV_BIN="$BIN_DIR/quarkdrive-webdav"
export LOCALQUARK_LOG_FILE="${LOCALQUARK_LOG_FILE:-$HOME/Library/Logs/LocalQuark-rust-webdav.log}"

# Cookies: parse ~/Library/Application Support/LocalQuark/cookies.json
# into a single Cookie header value and export as LOCALQUARK_QUARK_COOKIE.
# run-localquark.sh will pass it to webdav as --quark-cookie.
COOKIE_JSON="$HOME/Library/Application Support/LocalQuark/cookies.json"
if [ -f "$COOKIE_JSON" ]; then
    COOKIE_STR="$(/usr/bin/python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print('; '.join(f'{k}={v}' for k,v in d.items()))" "$COOKIE_JSON" 2>/dev/null || true)"
    [ -n "$COOKIE_STR" ] && export LOCALQUARK_QUARK_COOKIE="$COOKIE_STR"
fi

# Idempotent: if webdav is already listening on LOCALQUARK_PORT, skip
# the bootstrap and just reveal the mount in Finder.
if /usr/sbin/lsof -nP -iTCP:"$LOCALQUARK_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    [ -d "$LOCALQUARK_MOUNT_POINT" ] && /usr/bin/open -a Finder "$LOCALQUARK_MOUNT_POINT" || true
    exit 0
fi

# Hand the bootstrap to run-localquark.sh in the background.
nohup "$BIN_DIR/run-localquark.sh" >> "$START_LOG" 2>&1 &
disown 2>/dev/null || true

# Wait up to ~6s for the listener. If startup fails (helper missing,
# cookies expired, cert generation locked), still bring up Finder so
# the user can read the start log from ~/Library/Logs/.
LISTEN_OK=0
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if /usr/sbin/lsof -nP -iTCP:"$LOCALQUARK_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
        LISTEN_OK=1
        break
    fi
    sleep 0.3
done

if [ -d "$LOCALQUARK_MOUNT_POINT" ]; then
    /usr/bin/open -a Finder "$LOCALQUARK_MOUNT_POINT"
else
    /usr/bin/open -a Finder /Volumes
fi

if [ "$LISTEN_OK" -ne 1 ]; then
    /usr/bin/osascript -e "display alert \"LocalQuark-rust\" message \"webdav did not bind $LOCALQUARK_HOST:$LOCALQUARK_PORT within 6s. See $START_LOG.\" as critical" >/dev/null 2>&1 || true
fi
exit 0
LAUNCHER
chmod 0755 "$CONTENTS_DIR/MacOS/LocalQuark"

# 4) bash scripts
cp "$SCRIPTS_SRC/lib-common.sh"           "$BIN_DIR/lib-common.sh"
cp "$SCRIPTS_SRC/run-localquark.sh"       "$BIN_DIR/run-localquark.sh"
cp "$SCRIPTS_SRC/teardown-localquark.sh"  "$BIN_DIR/teardown-localquark.sh"
cp "$SCRIPTS_SRC/setup-tls.sh"            "$BIN_DIR/setup-tls.sh"
cp "$SCRIPTS_SRC/helper-client.sh"        "$BIN_DIR/helper-client.sh"
cp "$SCRIPTS_SRC/status-localquark.sh"    "$BIN_DIR/status-localquark.sh"
cp "$SCRIPTS_SRC/install-helper.sh"       "$BIN_DIR/install-helper.sh"
cp "$SCRIPTS_SRC/uninstall-helper.sh"     "$BIN_DIR/uninstall-helper.sh"
chmod 0755 "$BIN_DIR"/*.sh

# 5) quarkdrive-webdav binary
cp "$WEBDAV_BIN" "$BIN_DIR/quarkdrive-webdav"
chmod 0755 "$BIN_DIR/quarkdrive-webdav"

# 6) helper
cp "$HELPER_BIN" "$HELPER_DIR/com.localquark.webdav-helper"
chmod 0755 "$HELPER_DIR/com.localquark.webdav-helper"
cp "$HELPER_PLIST_SRC" "$HELPER_DIR/Resources/com.localquark.webdav-helper.plist"
chmod 0644 "$HELPER_DIR/Resources/com.localquark.webdav-helper.plist"

# 7) ad-hoc codesign so Gatekeeper does not quarantine on first launch
codesign --force --deep --sign - "$APP_DIR" 2>/dev/null || true

# 8) 生成分发说明（INSTALL.md），随 .app 一起发送给接收者
DIST_DIR="$(dirname "$APP_DIR")"
cat > "$DIST_DIR/INSTALL.md" << 'INSTALL_MD'
# LocalQuark-rust 安装说明

> 本 app 使用 ad-hoc 签名（非 Apple 开发者证书），macOS Gatekeeper
> 会阻止首次运行。请按以下步骤操作，**只需做一次**。

---

## 第一步：解除 Gatekeeper 隔离

将 `LocalQuark-rust.app` 移动到 `/Applications` 后，打开「终端」执行：

```bash
xattr -cr /Applications/LocalQuark-rust.app
```

> 如果你把 app 放在其他位置，把路径换成实际路径即可。

---

## 第二步：安装特权 Helper（需要管理员密码）

打开「终端」执行：

```bash
/Applications/LocalQuark-rust.app/Contents/Resources/bin/install-helper.sh
```

输入 macOS 登录密码后，Helper 会安装到系统，只需安装一次。

---

## 第三步：首次使用 — 设置夸克 Cookie

1. 在 Chrome / Brave / Arc / Edge 中登录 [夸克网盘](https://pan.quark.cn)
2. 双击打开 `LocalQuark-rust.app`
3. app 会自动从浏览器读取 Cookie 并挂载网盘到 `/Volumes/LocalQuark`

> **注意**：Cookie 有效期约 30 天，到期后需要重新登录浏览器，
> 然后重启 app 自动刷新。

---

## 日志位置

| 文件 | 用途 |
|------|------|
| `~/Library/Logs/LocalQuark-rust-launcher.log` | 启动日志 |
| `~/Library/Logs/LocalQuark-rust-webdav.log`   | WebDAV 服务日志 |

---

## 卸载

```bash
/Applications/LocalQuark-rust.app/Contents/Resources/bin/uninstall-helper.sh
rm -rf /Applications/LocalQuark-rust.app
```

---

## 系统要求

- macOS 12.0 (Monterey) 或更高版本
- Chrome / Brave / Arc / Edge（需登录夸克网盘）
INSTALL_MD

echo "==> built $APP_DIR"
echo "==> install guide: $DIST_DIR/INSTALL.md"
echo "    contents:"
find "$APP_DIR" -type f -maxdepth 4 | sort | sed 's/^/      /'
