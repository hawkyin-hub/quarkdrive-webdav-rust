# quarkdrive-webdav (crate)

The Rust crate that powers [QuarkDrive-WebDAV](../README.md). Exposes a `WebDAV` server that proxies Quark Drive's private REST API.

## Quick start

```bash
# from repo root
cargo build --release -p quarkdrive-webdav

./target/release/quarkdrive-webdav --serve-only \
  --host 127.0.0.1 --port 8443 \
  --quark-cookie "<paste from browser devtools>"
```

Then mount from Finder:

```bash
/bin/launchctl asuser $(id -u) /sbin/mount_webdav -s -v LocalQuark \
  https://127.0.0.1:8443/ /Volumes/LocalQuark
```

See [../docs/install.md](../docs/install.md) for the end-to-end install path including the privileged Helper that reads browser Cookies.

## What changed vs the upstream crate

This crate is a maintained fork of `chenqimiao/quarkdrive-webdav` with the following changes (full history in [../CHANGELOG.md](../CHANGELOG.md)):

| Area | Change |
|------|--------|
| Upload | Concurrency cap from Quark `part_thread`; buffered `JoinSet`; background `do_flush` keeps Finder snappy |
| Download | `--prefer-http-download` redirects Finder straight to the Quark CDN |
| Mount | `mount_supervisor_loop` auto-recovers lost mounts in ~8 seconds |
| Cache | Per-path write lock; chunk cache persisted across restarts |
| Admin | Built-in HTTP admin on `127.0.0.1:8444` (status / repair / refresh / cache clear / log tail) |
| Packaging | `.app` bundle with LaunchAgent + privileged Helper for browser Cookie access |

## Project layout

```
quarkdrive-webdav/
  Cargo.toml
  src/
    main.rs           # CLI, signal handling, spawn tasks
    lib.rs            # module re-exports
    vfs.rs            # dav-server filesystem trait impl
    cache.rs          # PROPFIND dir cache + chunk cache
    drive/
      mod.rs          # Quark Drive REST client
      model.rs        # Quark REST DTOs
    proxy.rs          # HTTPS termination + forwarding
    webdav.rs         # backend HTTP listener
    health.rs         # cookie + webdav health checks
    admin.rs          # admin HTTP server + HTML UI
    notifier.rs       # macOS notification bridge
    tray.rs           # optional menubar UI (off by default)
  AGENTS.md           # in-tree module conventions
```

The protected surfaces (per [`../.github/CODEOWNERS`](../.github/CODEOWNERS)) are `webdav.rs`, `proxy.rs`, `mount.rs`, and `scripts/`. Touching these requires explicit ack.

## Building

| Command | What |
|---------|------|
| `cargo build --release -p quarkdrive-webdav` | optimized binary |
| `cargo clippy --all-targets -- -D warnings` | lint gate |
| `cargo fmt` | format |
| `bash scripts/build-app.sh` | package as `.app` |
| `bash scripts/build_deploy_test.sh` | full build → install → restart → test |

## Releasing

1. Update version in `Cargo.toml`.
2. Add an entry to [../CHANGELOG.md](../CHANGELOG.md).
3. `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo build --release -p quarkdrive-webdav`.
4. `git tag vX.Y.Z && git push --tags`. CI publishes the `.app` artifact to GitHub Releases.

## License

[MIT](../LICENSE). Original work © Qimiao Chen; modifications for the `hawkyin-hub/quarkdrive-webdav-rust` fork by the maintainers listed in [Cargo.toml](Cargo.toml).
