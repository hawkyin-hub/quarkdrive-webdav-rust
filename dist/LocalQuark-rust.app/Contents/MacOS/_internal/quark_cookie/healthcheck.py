#!/usr/bin/env python3
"""健康检查:
1. cookies.json 是否存在 + 是否含夸克网盘必要字段
2. cookies 里的 expires 是否在 renew_before_expiry 内,是则触发一次 refresher
3. 挂载点是否在线(mount 输出)
4. 有问题就发 macOS 通知
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent

try:
    from .config import load, resolve_path
except ImportError:
    from config import load, resolve_path

# 必要 cookie 字段(夸克网盘核心认证)。缺了基本就废了
REQUIRED_COOKIES = ("__pus", "sl-session")


def log(cfg: dict, msg: str) -> None:
    line = f"[{datetime.now().isoformat(timespec='seconds')}] {msg}"
    print(line)
    p = resolve_path(cfg["health_log_file"])
    p.parent.mkdir(parents=True, exist_ok=True)
    with p.open("a") as f:
        f.write(line + "\n")


def notify(title: str, body: str) -> None:
    """macOS 通知中心。失败也不抛。"""
    safe_title = title.replace('"', "'")
    safe_body = body.replace('"', "'")
    script = f'display notification "{safe_body}" with title "{safe_title}"'
    try:
        subprocess.run(["osascript", "-e", script], capture_output=True, timeout=5)
    except Exception:
        pass


def check_cookies(cfg: dict) -> tuple[bool, str]:
    cookies_path = resolve_path(cfg["cookies_file"])
    if not cookies_path.exists():
        return False, f"cookies.json 不存在:{cookies_path}"
    try:
        cookies = json.loads(cookies_path.read_text())
    except Exception as e:
        return False, f"cookies.json 解析失败:{e}"
    if not cookies:
        return False, "cookies.json 为空"
    missing = [k for k in REQUIRED_COOKIES if k not in cookies]
    if missing:
        return False, f"缺少必要 cookie 字段:{','.join(missing)}(可能已退出登录)"
    return True, f"cookies OK({len(cookies)} 项)"


def check_mount(cfg: dict) -> tuple[bool, str]:
    mount_point = str(resolve_path(cfg["mount_point"]))
    if not Path(mount_point).exists():
        return False, f"挂载点目录不存在:{mount_point}"
    # macOS:检查 /sbin/mount 输出里是否包含
    try:
        out = subprocess.run(["/sbin/mount"], capture_output=True, text=True, check=True).stdout
    except Exception as e:
        return False, f"无法读取 mount 表:{e}"
    if mount_point in out:
        return True, "挂载点在线"
    return False, f"挂载点未挂载:{mount_point}"


def maybe_renew(cfg: dict) -> bool:
    """距过期 renew_before_expiry 秒以内,主动跑一次 refresher。"""
    cookies_path = resolve_path(cfg["cookies_file"])
    try:
        cookies = json.loads(cookies_path.read_text())
    except Exception:
        return False
    expires = cookies.get("__pus_expires") or cookies.get("__pus_expire")
    if not expires:
        return False
    try:
        exp_ts = float(expires)
    except (TypeError, ValueError):
        return False
    now = time.time()
    threshold = cfg["healthcheck"].get("renew_before_expiry", 86400)
    if exp_ts - now > threshold:
        return False
    log(cfg, f"cookies 将在 {int(exp_ts - now)}s 后过期,主动刷新")
    rc = subprocess.run(
        [sys.executable, str(HERE / "refresher.py")],
        capture_output=True, text=True,
    ).returncode
    return rc == 0


def main() -> int:
    cfg = load()
    if not cfg["healthcheck"].get("enabled", True):
        return 0

    failures: list[str] = []
    ok_msgs: list[str] = []

    for name, fn in [("cookies", check_cookies), ("mount", check_mount)]:
        ok, msg = fn(cfg)
        (ok_msgs if ok else failures).append(f"[{name}] {msg}")

    for m in ok_msgs:
        log(cfg, "✓ " + m)
    for m in failures:
        log(cfg, "✗ " + m)

    if failures:
        notify("LocalQuark 异常", " | ".join(failures))
        # 挂载掉了就尝试拉起
        if any("挂载点" in f for f in failures):
            project_dir = HERE.parent
            mp = str(resolve_path(cfg["mount_point"]))
            log(cfg, "尝试重新挂载...")
            subprocess.Popen(
                ["bash", str(project_dir / "mount" / "quark_mount.sh")],
                stdout=open(resolve_path(cfg["log_file"]), "a"),
                stderr=subprocess.STDOUT,
            )
        return 1

    # 一切正常时,若 cookie 快过期则静默续期
    if maybe_renew(cfg):
        log(cfg, "已自动续期 cookies")
    return 0


if __name__ == "__main__":
    sys.exit(main())