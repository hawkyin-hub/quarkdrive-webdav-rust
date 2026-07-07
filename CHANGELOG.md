# Changelog

All notable changes to this project are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows [SemVer](https://semver.org/).

## [Unreleased]

### Fixed
- Drag-drop no longer creates 0-byte stubs + `(1)` siblings — `do_flush` recognizes the macOS `webdavfs_agent` 0-byte probe and skips `up_pre`.
- Mount-supervisor now waits 2 s before re-mounting so Finder has time to release vnodes.
- `build_deploy_test.sh` now unmounts before killing the server, so `webdavfs_agent` exits cleanly instead of leaving stale vnodes.

## [1.3.9] — 2026-07-05

### Fixed
- Health checker treats Cookie as healthy when at least one known key (`sl-session`, `__pus`, `__kp`, `__uid`, `isQuark`, `grey-id`, `ctoken`) is present (was: required `sl-session`, which is rarely set by Quark directly).
- `vfs.read_dir` filters out macOS AppleDouble metadata (`._*`, `.DS_Store`, `.Trashes`, `.fseventsd`) so orphan `._doc` files from prior macOS uploads no longer block directory deletion.
- Upload-success orphans (`new_fid` from `do_flush` before `commit`/`finish`) are now removed on failure paths.
- `remove_active_write` is called on `FS::remove_file` to drop stale `active_writes` cache entries.

### Added
- Admin HTTP server at `127.0.0.1:8444` with HTML panel and JSON endpoints (`/api/status`, `/api/repair`, `/api/refresh`, `/api/cache/info`, `/api/cache/clear`, `/api/logs/tail`).
- Self-healing directory listing when stale parent cache hides the target file (re-walks from root).
- `mount_supervisor_loop` keeps the WebDAV mount alive without external cron.

### Changed
- macOS mount path is hard-coded to `/Volumes/LocalQuark` (was: configuration-dependent, occasionally drifted to `~/Mount/Quark`).
- `quarkdrive-webdav` now self-restarts cleanly when the binary is replaced via `build_deploy_test.sh`.

## [1.3.0] — 2026-06-25

### Changed
- Switched from Python (LocalQuark legacy helper) to a pure-Rust implementation in `quarkdrive-webdav/`.
- Tokio multi-thread runtime replaces asyncio.
- Removed legacy `dav-server-basic` crate in favor of merged crates.

### Added
- Cookie refresh via the privileged Helper (`com.localquark.webdav-helper`).
- Auto-reconnect after webdavfs_agent reconnects.

## [1.0.0] — 2025-04-12

### Added
- Initial release: WebDAV adapter for Quark Drive on macOS.

[Unreleased]: https://github.com/hawkyin-hub/quarkdrive-webdav-rust/compare/v1.3.9...HEAD
[1.3.9]: https://github.com/hawkyin-hub/quarkdrive-webdav-rust/compare/v1.3.0...v1.3.9
[1.3.0]: https://github.com/hawkyin-hub/quarkdrive-webdav-rust/compare/v1.0.0...v1.3.0
[1.0.0]: https://github.com/hawkyin-hub/quarkdrive-webdav-rust/releases/tag/v1.0.0
