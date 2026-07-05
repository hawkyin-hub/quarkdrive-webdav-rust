#!/usr/bin/env bash
# 第一次编译检查,把所有 warning/error 都吐出来
set -euo pipefail
cd /Users/HawkSept/myproject/myapp/localquark-rust/quarkdrive-webdav
cargo check --color never 2>&1 | tee /Users/HawkSept/myproject/myapp/localquark-rust/scripts/_pending/cargo-check-20260703-2200.log
