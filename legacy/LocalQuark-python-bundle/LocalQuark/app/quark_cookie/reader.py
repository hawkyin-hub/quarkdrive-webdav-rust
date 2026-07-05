#!/usr/bin/env python3
"""从 Chromium 系浏览器读取夸克网盘 cookie，写入 cookies.json。

支持的浏览器:Chrome / Brave / Edge / Arc / Chromium。
优先级:用户在 config 里指定的 > 默认顺序。
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

try:
    from .config import browser_paths, load, resolve_path  # 包内导入(frozen 模式)
except ImportError:
    from config import browser_paths, load, resolve_path  # 直接跑脚本时

# 夸克网盘相关域名
QUARK_DOMAINS = ("pan.quark.cn", "drive.quark.cn", "quark.cn")

# Chrome macOS cookie 加密实际参数(参考 pycookiecheat / Chromium 源码):
#   PBKDF2-HMAC-SHA1, salt = b"saltysalt", iterations = 1003, key length = 16
#   AES-128-CBC, IV 固定为 b" " * 16
#   encrypted_value 头 3 字节为 b"v10" / b"v11"
#   Chrome cookies SQLite meta.version >= 24 时,解密后前 32 字节是
#   SHA256(host_key) 域名前缀,必须剥掉
PBKDF2_ITERATIONS = 1003
COOKIE_DB_VERSION_WITH_DOMAIN_HASH = 24


def read_chrome_safe_storage_key() -> bytes | None:
    """从 macOS Keychain 读取 Chrome Safe Storage 密钥,PBKDF2 派生 16 字节 AES key。"""
    try:
        result = subprocess.run(
            [
                "security", "find-generic-password",
                "-w", "-s", "Chrome Safe Storage",
                "-a", "Chrome",
            ],
            capture_output=True, text=True, check=True,
        )
        password = result.stdout.strip()
    except subprocess.CalledProcessError:
        return None
    if not password:
        return None
    from hashlib import pbkdf2_hmac
    return pbkdf2_hmac("sha1", password.encode("utf-8"),
                       b"saltysalt", PBKDF2_ITERATIONS, 16)


def _read_cookie_db_version(conn: sqlite3.Connection) -> int:
    """读 SQLite meta 表里的 version;缺失/异常返回 0。"""
    try:
        conn.text_factory = bytes
        row = conn.execute("SELECT value FROM meta WHERE key='version'").fetchone()
        if row and row[0] is not None:
            try:
                return int(row[0])
            except ValueError:
                return 0
    except sqlite3.OperationalError:
        pass
    finally:
        conn.text_factory = str
    return 0


def decrypt_value(encrypted: bytes, key: bytes,
                  cookie_db_version: int = 0) -> str | None:
    """解密 v10/v11 Chrome cookie value。

    Args:
        encrypted: Cookies.encrypted_value 原始字节(已含 b"v10"/b"v11" 前缀)
        key: PBKDF2 派生的 16 字节 AES key
        cookie_db_version: Chrome cookies SQLite meta.version,
            >= 24 时解密后还要剥 32 字节 SHA256(host_key)
    """
    if not encrypted:
        return ""
    if not (encrypted.startswith(b"v10") or encrypted.startswith(b"v11")):
        # 未加密的明文
        try:
            return encrypted.decode("utf-8")
        except UnicodeDecodeError:
            return encrypted.decode("latin-1", errors="replace")
    try:
        from Crypto.Cipher import AES
    except ImportError:
        print("需要 pycryptodome: pip install pycryptodome --break-system-packages", file=sys.stderr)
        return None
    try:
        iv = b" " * 16
        cipher = AES.new(key, AES.MODE_CBC, iv)
        plaintext = cipher.decrypt(encrypted[3:])
        if cookie_db_version >= COOKIE_DB_VERSION_WITH_DOMAIN_HASH:
            # 32 字节 SHA256(host_key) 域名前缀
            plaintext = plaintext[32:]
        # PKCS#7 去除填充
        pad = plaintext[-1]
        if 1 <= pad <= 16:
            plaintext = plaintext[:-pad]
        return plaintext.decode("utf-8")
    except UnicodeDecodeError:
        # 解出来不是合法 UTF-8,几乎肯定是 key 错或版本判断错。
        # 整条 cookie 丢弃,绝不 latin-1 兜底,避免污染下游。
        return None
    except Exception as e:
        print(f"解密失败: {e}", file=sys.stderr)
        return None


def extract_cookies_from_db(db_path: Path, key: bytes) -> dict[str, str]:
    """从单个 Cookies SQLite 抽取夸克网盘相关 cookie。"""
    cookies: dict[str, str] = {}
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
        tmp_path = Path(tmp.name)
    try:
        shutil.copy2(db_path, tmp_path)
        conn = sqlite3.connect(f"file:{tmp_path}?mode=ro", uri=True)
        try:
            db_version = _read_cookie_db_version(conn)
            cur = conn.execute(
                "SELECT host_key, name, value, encrypted_value FROM cookies "
                "WHERE host_key LIKE ? OR host_key LIKE ? OR host_key LIKE ?",
                (f"%{QUARK_DOMAINS[0]}%", f"%{QUARK_DOMAINS[1]}%", f"%{QUARK_DOMAINS[2]}%"),
            )
            bad = 0
            for _host_key, name, plain, enc in cur.fetchall():
                value = plain if plain else decrypt_value(enc, key, db_version)
                if value is None:
                    bad += 1
                    continue
                cookies[name] = value
            if bad:
                print(f"[extract] {db_path.parent.name}: {bad} 条 cookie 解失败,已丢弃", file=sys.stderr)
        finally:
            conn.close()
    finally:
        tmp_path.unlink(missing_ok=True)
    return cookies


def find_browser_with_quark() -> tuple[str, Path] | None:
    """找到第一个含夸克 cookie 的浏览器 profile。"""
    key = read_chrome_safe_storage_key()
    if key is None:
        print("找不到 Chrome Safe Storage keychain 条目,请确保 Chrome 至少运行过一次。", file=sys.stderr)
        return None

    cfg = load()
    for name, path in browser_paths(cfg):
        if not path.exists():
            continue
        try:
            cookies = extract_cookies_from_db(path, key)
        except Exception as e:
            print(f"[{name}] 读取失败: {e}", file=sys.stderr)
            continue
        if cookies:
            print(f"[{name}] 找到 {len(cookies)} 个夸克 cookie", file=sys.stderr)
            return name, path
    return None


# WebKit epoch (1601-01-01) to Unix epoch (1970-01-01),in microseconds.
# Chrome cookies.last_access_utc / creation_utc are WebKit 时间戳 (μs)。
_WEBKIT_TO_UNIX_US = 11644473600000000


def _get_last_access_unix(db_path: Path) -> float:
    """读 SQLite,取该浏览器夸克 cookie 的 max(last_access_utc) → Unix 秒。

    列不存在或 SQLite 读不到时返 0.0(表示"不知道活跃度",让上层 fallback 到首个浏览器)。
    """
    if not db_path.exists():
        return 0.0
    try:
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp:
            tmp_path = Path(tmp.name)
        try:
            shutil.copy2(db_path, tmp_path)
            conn = sqlite3.connect(f"file:{tmp_path}?mode=ro", uri=True)
            try:
                # 先看 last_access_utc 列是否存在(Chrome v24+ 有)
                cols = {row[1] for row in conn.execute("PRAGMA table_info(cookies)").fetchall()}
                if "last_access_utc" not in cols:
                    return 0.0
                row = conn.execute(
                    "SELECT MAX(last_access_utc) FROM cookies "
                    "WHERE host_key LIKE ? OR host_key LIKE ? OR host_key LIKE ?",
                    (f"%{QUARK_DOMAINS[0]}%", f"%{QUARK_DOMAINS[1]}%", f"%{QUARK_DOMAINS[2]}%"),
                ).fetchone()
                if not row or not row[0]:
                    return 0.0
                return (int(row[0]) - _WEBKIT_TO_UNIX_US) / 1_000_000
            finally:
                conn.close()
        finally:
            tmp_path.unlink(missing_ok=True)
    except Exception:
        return 0.0


def find_best_browser_with_quark() -> tuple[str | None, Path | None, float]:
    """找到"最近一次访问过夸克"的浏览器 profile。

    返回 3-tuple: (browser_name, db_path, last_access_unix_seconds)。
    没有任何浏览器含夸克 cookie 时返回 (None, None, 0.0)。

    与 find_browser_with_quark 的区别:多浏览器的场景下,优先选最近活跃的,
    而不是按 config.yaml 里写死的顺序。这样用户从 Safari / Edge / 夸克客户端
    登录后,定时刷新能自动跟上,不会被老浏览器覆盖。
    """
    key = read_chrome_safe_storage_key()
    if key is None:
        print("找不到 Chrome Safe Storage keychain 条目,请确保 Chrome 至少运行过一次。", file=sys.stderr)
        return None, None, 0.0

    cfg = load()
    candidates: list[tuple[float, str, Path]] = []
    for name, path in browser_paths(cfg):
        if not path.exists():
            continue
        try:
            cookies = extract_cookies_from_db(path, key)
        except Exception as e:
            print(f"[{name}] 读取失败: {e}", file=sys.stderr)
            continue
        if not cookies:
            continue
        last = _get_last_access_unix(path)
        print(
            f"[{name}] 找到 {len(cookies)} 个夸克 cookie, last_access_unix={last:.0f}",
            file=sys.stderr,
        )
        candidates.append((last, name, path))

    if not candidates:
        return None, None, 0.0
    candidates.sort(key=lambda x: x[0], reverse=True)
    _, best_name, best_path = candidates[0]
    return best_name, best_path, candidates[0][0]


def main() -> int:
    parser = argparse.ArgumentParser(description="从浏览器提取夸克网盘 cookies")
    parser.add_argument("-c", "--config", default=None,
                        help="配置文件路径(默认 config.yaml)")
    parser.add_argument("-o", "--output", default=None,
                        help="输出 JSON 路径,覆盖配置")
    parser.add_argument("--quiet", action="store_true", help="安静模式")
    args = parser.parse_args()

    cfg = load(Path(args.config) if args.config else None)
    out_path = resolve_path(args.output or cfg["cookies_file"])

    def log(msg: str) -> None:
        if not args.quiet:
            print(msg)

    found = find_browser_with_quark()
    if not found:
        log("未在任何浏览器找到夸克网盘 cookies,请先在 Chrome 登录 https://pan.quark.cn")
        return 1
    name, path = found
    key = read_chrome_safe_storage_key()
    cookies = extract_cookies_from_db(path, key)

    if not cookies:
        log("浏览器里没有夸克网盘的 cookie,先登录一下吧。")
        return 1

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(cookies, indent=2, ensure_ascii=False))
    out_path.chmod(0o600)
    log(f"已写入 {out_path} ({len(cookies)} 项,来源:{name})")
    return 0


if __name__ == "__main__":
    sys.exit(main())


# ---------- 手动粘贴 cookie 字符串解析 ----------
# 用户在 Chrome DevTools → Network → Request Headers → Cookie,或 console 跑
# copy(document.cookie),粘贴到这里。返回 dict[str, str]。
#
# 当前主路径:SQLite 解密出来的是 v10 ciphertext,跟浏览器实际收到的 cookie
# value 是两码事(早期走 SQLite 那条路导致 backend 401)。用户复制出来的
# Cookie header 才是夸克服务器真正认可的明文。

REQUIRED_COOKIE_KEYS = ("__pus", "__puus", "isg")


def parse_cookie_header(s: str) -> dict[str, str]:
    """解析 "k=v; k=v" 形式的 Cookie header 字符串。

    容错:
      - 允许 ";" 后接任意空白(包括换行)
      - 忽略空段、"=" 前没 key 的段
      - 内部不 URL-decode(浏览器 Cookie header 本就是明文)
    """
    cookies: dict[str, str] = {}
    if not s:
        return cookies
    for raw_pair in s.split(";"):
        pair = raw_pair.strip()
        if not pair or "=" not in pair:
            continue
        k, _, v = pair.partition("=")
        k = k.strip()
        v = v.strip().replace("\x00", "")
        if k and v:
            cookies[k] = v
    return cookies


def save_cookies_from_header(s: str, out_path: Path) -> tuple[bool, str]:
    """解析 + 校验关键字段 + 原子写入 cookies.json。"""
    cookies = parse_cookie_header(s)
    if not cookies:
        return False, "没解析到任何 cookie(请检查粘贴内容)"
    if not any(k in cookies for k in REQUIRED_COOKIE_KEYS):
        return False, (
            f"缺少关键字段 {REQUIRED_COOKIE_KEYS[0]} / {REQUIRED_COOKIE_KEYS[1]} / "
            f"{REQUIRED_COOKIE_KEYS[2]},粘贴的可能不是夸克 Cookie。"
        )
    out_path.parent.mkdir(parents=True, exist_ok=True)
    # 哨兵:refresh_once / 后台轮询读到 __pus_manual 就跳过 SQLite 覆盖,
    # 否则用户粘贴的真 cookie 会被 SQLite 解密的 binary 覆盖回 401 状态。
    cookies["__pus_manual"] = "1"
    tmp = out_path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(cookies, indent=2, ensure_ascii=False))
    tmp.chmod(0o600)
    os.replace(tmp, out_path)
    return True, f"已写入 {len(cookies)} 项(含哨兵 __pus_manual) → {out_path}"
