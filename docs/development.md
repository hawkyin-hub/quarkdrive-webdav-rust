# Development Guide

## Setup

Required:
- Rust **1.74+** (edition = "2024")
- macOS 12+ (for the helper that reads browser cookies)
- Xcode Command Line Tools (`xcode-select --install`)
- macFUSE / native `mount_webdav` (preinstalled on macOS 12+)

```bash
git clone https://github.com/hawkyin-hub/quarkdrive-webdav-rust.git
cd quarkdrive-webdav-rust
cargo build --release -p quarkdrive-webdav
```

Test without packaging:
```bash
./target/release/quarkdrive-webdav --serve-only --host 127.0.0.1 --port 8443 \
    --quark-cookie "<paste from browser devtools>"
```

## Project layout

```
.                         # workspace root
  Cargo.toml              # (optional) workspace, see /quarkdrive-webdav/Cargo.toml
  README.md
  CHANGELOG.md
  CONTRIBUTING.md
  LICENSE
  docs/                   # user-facing docs
  scripts/                # build & deployment automation
  quarkdrive-webdav/      # main crate
    Cargo.toml
    src/
      main.rs
      vfs.rs
      cache.rs
      drive/
      proxy.rs
      webdav.rs
      health.rs
      admin.rs
      notifier.rs
      tray.rs
    docker/               # Dockerfile (linux build path)
    AGENTS.md             # in-tree module conventions
```

## Build commands

| Command | What it does |
|---------|--------------|
| `cargo build` | dev build |
| `cargo build --release` | optimized build |
| `cargo test` | run unit tests (limited coverage today) |
| `cargo clippy --all-targets` | lint |
| `cargo fmt` | format |
| `bash scripts/build-app.sh` | package as `.app` (macOS) |
| `bash scripts/build_deploy_test.sh` | full build → install → restart → test loop |

## Logging

Default: `quarkdrive_webdav=info`. To debug:

```bash
RUST_LOG=quarkdrive_webdav=debug,reqwest=debug,proxy=debug ./quarkdrive-webdav --serve-only ...
```

Useful filters:
- `RUST_LOG=quarkdrive_webdav=trace,reqwest=warn` — noisy low-level
- `RUST_LOG=quarkdrive_webdav::vfs=debug` — only the VFS layer

## Code conventions (the §-rules)

These are referenced from inline comments. When editing code, preserve them.

- **§1.1**: never block tokio workers — wrap blocking I/O in `tokio::fs` or `spawn_blocking`.
- **§1.2**: errors are logged via `tracing::error!`, never swallowed silently.
- **§2.1**: same-path writes serialize through `fs.write_lock_for(path)`.
- **§2.2**: per-path upload generation counter; a superseded PUT cleans its temp file.
- **§2.3**: a task panic or `?` in a `tokio::spawn` body must not exit the process; log and keep serving.
- **§3**: when touching vfs.rs / cache.rs / drive/mod.rs / main.rs (admin/server entry) / tray.rs, do NOT touch webdav.rs / mount.rs / proxy.rs without explicit ack.

## Adding an admin endpoint

`src/admin.rs` is the entry point. Pattern:

```rust
(match_method, "/api/foo") => api_foo(&state).await,
```

`api_foo(&AdminState)` returns `Response<BoxBody>`. Use the existing helpers (`json_response`, `text_response`) for shape.

Always add:
- A row in the admin HTML panel (`src/admin.rs`, the `INDEX_HTML` const).
- Documentation update in [api.md](api.md).

## Release checklist

1. `cargo fmt && cargo clippy --all-targets -- -D warnings`
2. `cargo build --release` produces a working binary.
3. `scripts/build_deploy_test.sh` end-to-end passes.
4. Update `CHANGELOG.md`.
5. Tag `vX.Y.Z` and push; CI will publish the `.app` + `Cargo` artifact.
