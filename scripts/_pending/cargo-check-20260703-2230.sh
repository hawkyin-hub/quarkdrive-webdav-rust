#!/usr/bin/env bash
# 第一次编译检查
set -euo pipefail
cd /Users/HawkSept/myproject/myapp/localquark-rust/quarkdrive-webdav
cargo check --color never 2>&1 | tee /Users/HawkSept/myproject/myapp/localquark-rust/scripts/_pending/cargo-check-20260703-2230.log
