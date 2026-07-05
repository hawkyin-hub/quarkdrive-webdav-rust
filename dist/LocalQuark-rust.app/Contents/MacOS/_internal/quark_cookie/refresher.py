#!/usr/bin/env python3
"""refresher:被 LaunchAgent 调用,把浏览器 cookie 同步到 cookies.json。

带日志和失败检测。失败时保留旧文件,避免破坏挂载。
"""
from __future__ import annotations

import json
import os
import sys
from datetime import datetime
from pathlib import Path

HERE = Path(__file__).resolve().parent

try:
    from .config import load, resolve_path
except ImportError:
    from config import load, resolve_path


def log(msg: str) -> None:
    line = f"[{datetime.now().isoformat(timespec='seconds')}] {msg}"
    print(line)
    log_path = resolve_path(load()["log_file"])
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("a") as f:
        f.write(line + "\n")


def main() -> int:
    log("refresher 启动")
    try:
        try:
            from .reader import (
                extract_cookies_from_db,
                find_browser_with_quark,
                read_chrome_safe_storage_key,
            )
        except ImportError:
            from reader import (
                extract_cookies_from_db,
                find_browser_with_quark,
                read_chrome_safe_storage_key,
            )
    except ImportError as e:
        log(f"导入失败:{e}")
        return 1

    found = find_browser_with_quark()
    if not found:
        log("未找到含夸克 cookie 的浏览器,保留旧 cookies.json")
        return 2
    name, db_path = found
    key = read_chrome_safe_storage_key()
    cookies = extract_cookies_from_db(db_path, key)

    if not cookies:
        log(f"[{name}] 没拿到 cookie,保留旧文件")
        return 2

    out_path = resolve_path(load()["cookies_file"])
    # 哨兵检查:用户手动粘贴的 cookie 优先,不被 SQLite 覆盖
    if out_path.exists():
        try:
            existing = json.loads(out_path.read_text())
            if existing.get("__pus_manual"):
                log(f"检测到 __pus_manual 哨兵,跳过 SQLite 覆盖 → {out_path}")
                return 0
        except Exception:
            pass

    out_path.parent.mkdir(parents=True, exist_ok=True)
    tmp = out_path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(cookies, indent=2, ensure_ascii=False))
    tmp.chmod(0o600)
    os.replace(tmp, out_path)
    log(f"成功更新 ({len(cookies)} 项,来源:{name}) → {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())