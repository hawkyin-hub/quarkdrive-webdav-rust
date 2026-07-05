#!/usr/bin/env bash
# 安装 / 重装两个 LaunchAgent:
#   com.localquark.refresher   - 按 refresh_interval 拉取新 cookies
#   com.localquark.healthcheck - 按 mount_check_interval 检查状态,异常时通知

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AGENTS_DIR="$HOME/Library/LaunchAgents"
mkdir -p "$AGENTS_DIR"

PY="$(command -v python3)"

# 读 config,得到两个 interval
read -r REFRESH_INTERVAL HEALTH_INTERVAL < <(python3 - <<PY
import sys; sys.path.insert(0, "$PROJECT_DIR/quark_cookie")
import yaml
from pathlib import Path
cfg_path = Path("$PROJECT_DIR") / "config.yaml"
if cfg_path.exists():
    cfg = yaml.safe_load(cfg_path.read_text()) or {}
else:
    cfg = {}
print(cfg.get("refresh_interval", 43200))
print((cfg.get("healthcheck") or {}).get("mount_check_interval", 3600))
PY
)

# 首次跑:浏览器里有没有夸克 cookie
if ! python3 "$PROJECT_DIR/quark_cookie/reader.py" -o "$PROJECT_DIR/quark_cookie/cookies.json" --quiet 2>/dev/null; then
    echo "⚠️  浏览器里没找到夸克 cookie。"
    echo "请现在打开 Chrome(或 Brave/Edge),登录 https://pan.quark.cn"
    echo "登录后按回车继续..."
    read -r _
fi

# refresher plist
REFRESHER_PLIST="$AGENTS_DIR/com.localquark.refresher.plist"
sed -e "s|__PYTHON__|$PY|g" \
    -e "s|__REFresher_PATH__|$PROJECT_DIR/quark_cookie/refresher.py|g" \
    -e "s|__PROJECT_DIR__|$PROJECT_DIR|g" \
    -e "s|<integer>43200</integer>|<integer>$REFRESH_INTERVAL</integer>|g" \
    "$PROJECT_DIR/scripts/com.localquark.refresher.plist.template" > "$REFRESHER_PLIST"

# healthcheck plist
HEALTH_PLIST="$AGENTS_DIR/com.localquark.healthcheck.plist"
sed -e "s|__PYTHON__|$PY|g" \
    -e "s|__HEALTH_PATH__|$PROJECT_DIR/quark_cookie/healthcheck.py|g" \
    -e "s|__PROJECT_DIR__|$PROJECT_DIR|g" \
    -e "s|__HEALTH_INTERVAL__|$HEALTH_INTERVAL|g" \
    "$PROJECT_DIR/scripts/com.localquark.healthcheck.plist.template" > "$HEALTH_PLIST"

# 加载
launchctl unload "$REFRESHER_PLIST" 2>/dev/null || true
launchctl load "$REFRESHER_PLIST"
launchctl unload "$HEALTH_PLIST" 2>/dev/null || true
launchctl load "$HEALTH_PLIST"

# 立刻跑一次
launchctl start com.localquark.refresher
launchctl start com.localquark.healthcheck

echo "✅ LaunchAgents 已安装"
echo "  refresher  - 每 ${REFRESH_INTERVAL}s"
echo "  healthcheck- 每 ${HEALTH_INTERVAL}s"
echo "日志:$PROJECT_DIR/refresher.log  /  $PROJECT_DIR/healthcheck.log"