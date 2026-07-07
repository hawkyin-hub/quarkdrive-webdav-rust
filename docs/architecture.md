# Architecture

## Bird's-eye view

```
 +----------------+        WebDAV (HTTPS)        +-------------------------+
 |   Finder +     | <-------------------------> |  quarkdrive-webdav      |
 |  macOS         |     https://127.0.0.1:8443   |  (this repo, Rust)      |
 |  webdavfs_agent|                             |                         |
 +----------------+                             |  +-------------------+  |
                                                |  | dav-server (axum) |  |
                                                |  +---------+---------+  |
                                                |            |            |
                                                |  +---------v---------+  |
                                                |  | vfs layer         |  |
                                                |  |  - open/close     |  |
                                                |  |  - read/write     |  |
                                                |  |  - dir cache      |  |
                                                |  +---------+---------+  |
                                                |            |            |
                                                |  +---------v---------+  |
                                                |  | drive client      |  |
                                                |  |  - Quark REST API |  |
                                                |  +---------+---------+  |
                                                |            |            |
                                                +------------|------------+
                                                             |
                                                             v
                                                  +-----------------+
                                                  |  pan.quark.cn   |
                                                  |  (private REST) |
                                                  +-----------------+
```

## Process model

QuarkDrive-WebDAV runs as a **single multi-thread Tokio process**. Inside it:

| Task | Role |
|------|------|
| `proxy::run` | Listens on `127.0.0.1:8443`, TLS, forwards to backend |
| `backend::serve` | Listens on `127.0.0.1:8080`, dav-server, our `vfs.rs` impl |
| `mount_supervisor_loop` | Polls `/sbin/mount` every 3s; auto-recovers lost mount |
| `admin::serve` | Listens on `127.0.0.1:8444`, serves admin panel + JSON API |
| `notifier` | macOS `osascript display notification` (e.g. auto-recovery) |

A crash in any one task logs a warning but **does not exit the process** (see `§2.3` in [development.md](development.md)).

## Module map (source layout)

```
quarkdrive-webdav/src/
  main.rs           # CLI parsing, spawn tasks, mount supervisor
  lib.rs            # module re-exports
  vfs.rs            # dav-server filesystem trait impl (open, read, write, dir)
  cache.rs          # 2-tier cache: PROPFIND dir cache + chunk cache
  drive/mod.rs      # Quark Drive REST client (upload, download, list, ...)
  drive/model.rs    # Quark REST DTOs
  proxy.rs          # HTTPS termination + request forwarding
  webdav.rs         # backend HTTP listener (axum)
  health.rs         # cookie + webdav health checks (reused by admin)
  admin.rs          # admin HTTP server + HTML UI
  notifier.rs       # macOS notification center bridge
  tray.rs           # optional menubar UI (not bundled in default build)
```

### Request flow example: drag-drop upload

1. Finder writes to `/Volumes/LocalQuark/foo.txt`. macOS `webdavfs_agent` issues `PUT /foo.txt`.
2. Proxy accepts the TLS connection, auths with Basic credentials, forwards to backend.
3. Backend `webdav` accepts PUT, calls our `QuarkDriveFileSystem::open(path, write+create)`.
4. Finder streams body chunks via `write_buf`. We accumulate into a temp file on disk (parallel to computing MD5/SHA1).
5. Finder closes the file. Our `flush()` runs `do_flush()`:
   - `drive.up_pre` → Quark creates a fresh upload task, returns `fid` + S3-style chunk upload URLs.
   - For each chunk, we `drive.up_part` (sequential, since Quark requires `part_thread:1`).
   - `drive.up_auth_and_commit` then `drive.finish` to finalize.
6. Old `fid` (if overwrite) deleted; `register_active_write` makes the new file visible to Finder immediately.

### Concurrency model

- Tokio multi-thread runtime. The actix-like dav-server is internal; we own the worker pool size implicitly.
- Per-path write serialization: `vfs::write_lock_for(path)` mutexes concurrent PUTs to the same file (§2.1 in [development.md](development.md)).
- Read concurrency: separate read path. Chunked download uses `buffered(7)` futures for paginated directory listings, sequential for streaming reads.

## Where to look next

- [development.md](development.md) — build, test, debug
- [troubleshooting.md](troubleshooting.md) — known failure modes
- [security.md](security.md) — cookie/TLS/threat model
