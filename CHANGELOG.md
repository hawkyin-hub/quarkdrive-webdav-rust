# Changelog

All notable changes to the QuarkDrive-WebDAV project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/lang/zh-CN/spec/v2.0.0.html). Dates follow YYYY-MM-DD format.

## [Unreleased] - 2026-07-05

### Added
- **HTTPS 终结代理** (`quarkdrive-webdav/src/proxy.rs`)：本地起 8443 自签 HTTPS 终结代理，转发到 8080 HTTP backend，应对 macOS 26.6+ `webdavfs_agent` 拒绝 HTTP + Basic Auth 的限制。
- **单 `.app` 打包脚本** (`scripts/build-app.sh`)：`cargo build --release` + `Info.plist` + 启动器脚本，输出可双击运行的 `dist/LocalQuark-rust.app`，启动后自动挂载到 `/Volumes/LocalQuark` 并打开 Finder。
- **`warm_root` 启动预热** (`vfs.rs`)：默认开启 `--warm-root`，启动时主动 PROPFIND 一次根目录，让 moka 缓存就绪，首屏 Finder 响应更快。
- **`webdav.passwd` 持久化** (`mount.rs::write_passwd`)：随机生成的 WebDAV 凭据落到 `~/Library/Application Support/QuarkDrive/webdav.passwd`（权限 0o600），跨启动复用、避免重启挂载后认证失败。
- **CLI 双端口矩阵**：新增 `--host` / `--backend-host` / `--backend-port` 参数（frontend `127.0.0.1:8443`、backend `127.0.0.1:8080`），并保持 `--port` 兼容旧用法。
- **健康检查子命令**：`./target/release/quarkdrive-webdav health` 打印 `HealthReport` 后退出，方便 CI/排错。
- 新增 `docs/DEPLOYMENT.md`：完整 `.app` 打包 / 部署 / 卸载 / 排错手册。
- 新增 `docs/PERFORMANCE.md`：性能基线与可复现测试方法；明确 Phase 1（并发上传）已被回滚为串行、Phase 2（删 sleep）/ Phase 3（删 flush）真实生效。
- 完善 `ARCHITECTURE.md`：架构图 + 数据流 + 端口矩阵 + 关键设计决策（包含 HTTPS 终结代理动机）。
- 完善 `AGENTS.md`（仓库根 + `quarkdrive-webdav/`）：项目结构表、红色区域清单、性能 Phase 1~3 锚点。
- 新增 `CONTRIBUTING.md`：贡献流程、本仓库路径约定、bundled app 构建脚本说明。
- 修复 docs 中不一致：
  - `troubleshooting.md` 顶部仓库路径 `~/myapp/localquark-rust/` → `/Users/HawkSept/myproject/myapp/localquark-rust/`
  - 日志路径统一为 `~/Library/Logs/LocalQuark-rust-webdav.log` / `LocalQuark-rust-launcher.log`
  - 区分 QuarkDrive（Rust daemon 状态）vs LocalQuark（Python 残留 / cookies）
  - 加 helper-client 卡在 `mkdir /Volumes/LocalQuark` 的修复流程
  - `API.md` 同上路径修正 + `killall` 命令修正为 `quarkdrive-webdav`

### Changed
- **活跃代码目录统一**：迁移至 `/Users/HawkSept/myproject/myapp/localquark-rust/`，旧的 `~/Documents/localquark-rust/` 标记为废弃。
- **挂载点两套默认值**：
  - bundled `.app` 启动器 → `/Volumes/LocalQuark`
  - 普通 daemon 二进制 → `~/Mount/Quark`
- **默认 WebDAV 用户**：`admin` → `quasar`。
- **README / API 文档默认值统一**：端口、挂载点、用户、cookie 路径全部对齐 `main.rs` 中实际默认值。

### Fixed
- macOS Finder 在某些情况下挂载后显示空目录（通过 HTTPS 终结代理 + `warm_root` 缓解）。
- TLS 自签证书缺失或过期时无法启动（启动时自动重新生成 `~/Library/Application Support/QuarkDrive/cert.pem` + `key.pem`）。

---

## [1.4.0] - 2026-07-04

### Added
- **macOS 菜单栏托盘** (`tray.rs`)：原生托盘图标，支持状态显示 / 立即刷新 Cookie / 在 Finder 中显示 / 退出。
- **Cookie 自动抓取** (`cookie.rs`)：从 Chromium 系浏览器（Chrome / Brave / Edge / Arc）SQLite cookie 库中解密提取夸克网盘 Cookie，支持 v10/v11 AES-128-CBC。
- **健康检查与自动重挂** (`health.rs`)：每 60 秒检测一次 WebDAV 服务与挂载状态，故障时自动修复。
- **TLS/HTTPS 支持**：使用自签名证书提供 HTTPS WebDAV 服务。
- **性能优化 Phase 1~3**：上传并发、缓存同步延迟、磁盘 IO flush 优化（详见 README）。

### Changed
- **统一项目目录**：活跃开发代码集中到 `~/myproject/myapp/localquark-rust/`，废弃 `~/Documents/localquark-rust/` 旧目录。
- **依赖更新**：新增 `rusqlite`、`aes`、`cbc`、`pbkdf2`、`tray-icon`、`tempfile`、`rand` 等 crate。

### Fixed
- 修复大文件上传时因串行 chunk 导致的耗时问题（改为 `buffer_unordered(4)` 并发上传）。
  > **更正**：并发版本随后因 Quark API 返回 `part_thread:1`（触发 `PartNotSequential`）被回滚。当前仍是串行。详见 `docs/PERFORMANCE.md`。
- 移除不必要的 `sleep` 延迟，提升目录遍历响应速度。
- 移除 `consume_buf` 中的每写 `flush()`，减少磁盘 IO 瓶颈。

### Deprecated
- `localquark-app/`（Tauri 空壳项目）标记为 deprecated，后续将被移除。
- Python 版本（`LocalQuark/` 目录）仅保留历史参考，不再维护。

---

## [1.3.9] - 2026-06-28

### Changed
- 升级 `tokio` 到 1.45.1，`reqwest` 到 0.12.20。
- 优化 `vfs.rs` 中的缓存刷新策略。

---

## [1.3.8] - 2026-06-15

### Added
- 支持 `mount_webdav` 原生挂载，替代之前的 FUSE 方案。
- 新增 `proxy.rs` 模块，支持本地 HTTP/HTTPS 代理。

### Fixed
- 修复 WebDAV 根路径 PROPFIND 响应兼容性（macOS Finder）。

---

## [1.3.7] - 2026-06-01

### Added
- 支持多线程运行时配置：`num_cpus` 动态调整 worker 线程数。

### Changed
- 默认 chunk size 从 16KB 提升到 256KB，减少小文件碎片。

---

## [1.3.0] - 2026-05-10

### Added
- 首次实现完整的 WebDAV 协议支持（基于 `dav-server` crate）。
- 支持夸克网盘文件的上传、下载、目录浏览。
- 基于 `moka` 的内存缓存系统。

---

## [1.0.0] - 2026-04-01

### Added
- 项目初始版本，提供基础的 WebDAV 服务。
- 支持通过命令行指定 Cookie 和用户名密码。
