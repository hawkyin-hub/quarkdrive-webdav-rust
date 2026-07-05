"""LocalQuark 配置加载。"""
from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import yaml

DEFAULT_BROWSERS: dict[str, str] = {
    "chrome":          "~/Library/Application Support/Google/Chrome/Default/Cookies",
    "chrome_profile1": "~/Library/Application Support/Google/Chrome/Profile 1/Cookies",
    "brave":           "~/Library/Application Support/BraveSoftware/Brave-Browser/Default/Cookies",
    "edge":            "~/Library/Application Support/Microsoft Edge/Default/Cookies",
    "arc":             "~/Library/Application Support/Arc/User Data/Default/Cookies",
    "chromium":        "~/Library/Application Support/Chromium/Default/Cookies",
}

DEFAULTS: dict[str, Any] = {
    "cookies_file": "quark_cookie/cookies.json",
    "mount_point": "~/Mount/Quark",
    "browser_priority": ["chrome", "brave", "edge"],
    "refresh_interval": 43200,
    "log_file": "refresher.log",
    "health_log_file": "healthcheck.log",
    "healthcheck": {
        "enabled": True,
        "renew_before_expiry": 600,
        "mount_check_interval": 3600,
    },
    "webdav_host": "127.0.0.1",
    "webdav_port": 8080,
}


def project_dir() -> Path:
    return Path(__file__).resolve().parents[1]


def resolve_path(p: str) -> Path:
    p = os.path.expanduser(p)
    path = Path(p)
    if not path.is_absolute():
        path = (project_dir() / path).resolve()
    return path


def load(config_path: Path | None = None) -> dict[str, Any]:
    cfg = dict(DEFAULTS)
    path = config_path or (project_dir() / "config.yaml")
    if path.exists():
        with path.open() as f:
            user_cfg = yaml.safe_load(f) or {}
        _deep_merge(cfg, user_cfg)
    return cfg


def _deep_merge(base: dict, over: dict) -> None:
    for k, v in over.items():
        if isinstance(v, dict) and isinstance(base.get(k), dict):
            _deep_merge(base[k], v)
        else:
            base[k] = v


def browser_paths(cfg: dict[str, Any]) -> list[tuple[str, Path]]:
    out: list[tuple[str, Path]] = []
    for name in cfg["browser_priority"]:
        raw = DEFAULT_BROWSERS.get(name)
        if raw is None:
            continue
        out.append((name, resolve_path(raw)))
    return out