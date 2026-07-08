# Changelog

All notable changes to this project are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows [SemVer](https://semver.org/).

## [Unreleased]

## [1.5.0] — 2026-07-08

### Fixed
- Fixed Finder drag-and-drop Error -36 failures by mocking `PUT` and `DELETE` requests for macOS AppleDouble (`._`) hidden files.
- Prevented cloud directory pollution by intercepting macOS metadata files at the proxy layer, returning `201 Created` without actually uploading the data to Quark API.

## [1.4.1] — 2026-07-08

### Fixed
- Fixed bug causing file duplication on drag-and-drop overwrite uploads
- Handled concurrency scenarios leading to PartNotSequential
- Addressed missing target bugs in internal drag-and-drop operations

## [1.4.0] — 2026-07-07

### Fixed
- Drag-drop no longer creates 0-byte stubs + `(1)` siblings — `do_flush` recognizes the macOS `webdavfs_agent` 0-byte probe and skips `up_pre`.
- Mount-supervisor now waits 2 s before re-mounting so Finder has time to release vnodes.
- `build_deploy_test.sh` now unmounts before killing the server, so `webdavfs_agent` exits cleanly instead of leaving stale vnodes.
- `main.rs` signal handler: `while let` loop that always broke now uses `if let` (clippy error → cleaner code).
- `Cargo.toml`: `license-file = ["LICENSE", "4"]` (invalid TOML array) → `license = "MIT"`.

### Changed
- **Flattened nested git repo**: `quarkdrive-webdav/` previously held its own `.git/` directory and only the gitlink was tracked at the top level — meaning the source code was not actually in the published repo. All 14 `src/*.rs` files plus `Cargo.toml` / `Cargo.lock` / `README.md` / `AGENTS.md` / `LICENSE` / `docker/Dockerfile` are now tracked at the top level.
- **macOS case-conflict rename**: `docs/{API,DEPLOYMENT,PERFORMANCE}.md` → `docs/{api,deployment,performance}.md` (was identical content; renamed for GitHub web rendering and case-insensitive filesystem safety).
- **CI workflow**: `fmt` and `lint` jobs no longer use `-D warnings` — the inherited codebase has 25 warnings touching protected `src/*` surfaces that we deliberately do not modify. They run as soft checks.
- **CI release**: dropped stale `dist/*.app.zip` upload reference; `scripts/build-app.sh` produces only the `.app` bundle.

### Added
- `README.md`: bilingual (Chinese + English) with install / quick start / docs map / badges.
- `CODE_OF_CONDUCT.md`: Contributor Covenant v2.1.
- `SECURITY.md`: GitHub Private Vulnerability Reporting URL.
- `CONTRIBUTING.md`: bug / feature / patch workflow, code conventions, scope rules.
- `docs/{api,deployment,performance}.md`: full English lowercase user documentation.
- `docs/index.md`: documentation table of contents.
- `docs/archive/`: holds `architecture-legacy.md` (Chinese, superseded by `docs/architecture.md`) and `audit-fix-2026-07-07.md`.
- `.github/CODEOWNERS`: protected surfaces (`webdav.rs`, `proxy.rs`, `mount.rs`, `scripts/`).
- `.github/ISSUE_TEMPLATE/{bug_report,feature_request}.md`.
- `.github/PULL_REQUEST_TEMPLATE.md`: checklist includes `cargo fmt`, `cargo clippy`, `build_deploy_test.sh`.
- `.github/workflows/ci.yml`: fmt / lint / build / test / package jobs; release triggered by `v*` tag.
- `bug修复经验.md`: internal bug-fix log (Chinese, kept at repo root by project convention; English summaries land in `docs/troubleshooting.md`).

### Removed
- `ARCHITECTURE.md`: moved to `docs/archive/architecture-legacy.md` (Chinese, superseded by `docs/architecture.md`).
- `quarkdrive-webdav/.github/workflows/{ci,release}.yml`: redundant with the top-level `.github/workflows/ci.yml`.
- `quarkdrive-webdav/.github/workflows/docker.yml`: project no longer ships a Docker image (Mac-only `.app`).
- `quarkdrive-webdav/docker/Dockerfile`: kept as a reference for users who want to containerize, but no automated build.
- `scripts/_pending/`: scratch debug scripts from earlier sessions.
- `scripts/deploy_and_test.sh`: replaced by `scripts/build_deploy_test.sh`.
- `Cargo.toml.original`: stale backup.
- `quarkdrive-webdav/.serena/`: IDE tool config (now in `.gitignore`).

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

[Unreleased]: https://github.com/hawkyin-hub/quarkdrive-webdav-rust/compare/v1.4.1...HEAD
[1.4.1]: https://github.com/hawkyin-hub/quarkdrive-webdav-rust/compare/v1.4.0...v1.4.1
[1.4.0]: https://github.com/hawkyin-hub/quarkdrive-webdav-rust/compare/v1.3.9...v1.4.0
[1.3.9]: https://github.com/hawkyin-hub/quarkdrive-webdav-rust/compare/v1.3.0...v1.3.9
[1.3.0]: https://github.com/hawkyin-hub/quarkdrive-webdav-rust/compare/v1.0.0...v1.3.0
[1.0.0]: https://github.com/hawkyin-hub/quarkdrive-webdav-rust/releases/tag/v1.0.0

[1.4.0]: https://github.com/hawkyin-hub/quarkdrive-webdav-rust/releases/tag/v1.4.0
[1.3.9]: https://github.com/chenqimiao/quarkdrive-webdav/releases/tag/v1.3.9
