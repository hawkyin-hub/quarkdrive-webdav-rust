# QuarkDrive-WebDAV

> 把夸克网盘 (Quark Drive) 挂载成 macOS Finder 里的一个原生磁盘。

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.74+](https://img.shields.io/badge/rust-1.74%2B-orange.svg)](https://www.rust-lang.org)
[![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey.svg)](https://github.com/hawkyin-hub/quarkdrive-webdav-rust/releases)

[English](#english) | [中文](#中文)

---

## 中文

### 它做什么

把夸克网盘挂载成一个 macOS 原生 WebDAV 卷（默认 `/Volumes/LocalQuark`），使你可以在 Finder 里：

- **直接拖放**：把本地文件拖进网盘，就像 U 盘一样
- **直接编辑**：双击打开 `.docx`/`.md`/`.txt` 等文件，保存即上传
- **直接读取**：流式读取大文件，不先把整个文件下载到本地
- **断线自动重连**：内置 supervisor，挂载丢失后 8 秒内自动恢复

### 工作原理（一句话）

跑一个本地 Rust 进程，对外是 HTTPS WebDAV（`https://127.0.0.1:8443`），对内代理你的夸克网盘 QuarkDrive（`pan.quark.cn`）的私有 HTTP API。macOS 通过系统自带的 `mount_webdav` 客户端连到这个本地服务。

```
Finder (macOS webdavfs_agent)
       │
       ▼  https://127.0.0.1:8443
quarkdrive-webdav (this repo, Rust)
       │
       ▼  HTTPS + 私有 API
pan.quark.cn (Quark Drive 私有 REST)
```

### 安装

#### 方式 A：下载预编译 `.app`（最简单）

前往 [Releases](https://github.com/hawkyin-hub/quarkdrive-webdav-rust/releases) 拖动最新 `LocalQuark-rust.app` 到 `/Applications`。

第一次启动会弹 macOS Gatekeeper，按提示：
```bash
xattr -cr /Applications/LocalQuark-rust.app
```

#### 方式 B：从源码 build

依赖：Rust 1.74+、macOS 12+、Xcode Command Line Tools。

```bash
git clone https://github.com/hawkyin-hub/quarkdrive-webdav-rust.git
cd quarkdrive-webdav-rust
cargo build --release -p quarkdrive-webdav
./scripts/build-app.sh         # 打包成 .app
sudo cp -R dist/LocalQuark-rust.app /Applications/
```

详见 [docs/install.md](docs/install.md)。

### 快速开始

1. **登录夸克**：用 Chrome / Brave / Arc / Edge 任意一个打开 [pan.quark.cn](https://pan.quark.cn) 并保持登录。
2. **安装 Helper**：首次需要授予 macOS 权限读浏览器 Cookie（一次性）：
   ```bash
   /Applications/LocalQuark-rust.app/Contents/Resources/bin/install-helper.sh
   ```
3. **启动**：双击 `LocalQuark-rust.app`，Finder 会自动打开 `/Volumes/LocalQuark`。
4. **使用**：直接拖文件进去。

管理面板：[http://127.0.0.1:8444](http://127.0.0.1:8444)

### Cookie 过期怎么办

Cookie 默认 ~30 天。过期后无需重新登录夸克：
- 再访问一次 [pan.quark.cn](https://pan.quark.cn) 刷新 Cookie 即可
- 或打开管理面板的 **Cookie 刷新** 按钮

### 文档地图

| 文档 | 看什么 |
|------|--------|
| [docs/install.md](docs/install.md) | 详细安装步骤 |
| [docs/architecture.md](docs/architecture.md) | 系统架构、模块分工 |
| [docs/development.md](docs/development.md) | 开发者指南、代码结构、调试技巧 |
| [docs/deployment.md](docs/deployment.md) | 部署到生产环境、守护进程 |
| [docs/performance.md](docs/performance.md) | 上传/下载性能优化记录 |
| [docs/troubleshooting.md](docs/troubleshooting.md) | 常见问题与故障排查 |
| [docs/security.md](docs/security.md) | 安全模型、Cookie 处理、threat model |
| [docs/api.md](docs/api.md) | 管理面板 HTTP API |
| [CHANGELOG.md](CHANGELOG.md) | 变更日志 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 如何贡献代码 |

### 贡献

欢迎 PR！读 [CONTRIBUTING.md](CONTRIBUTING.md) 看开发流程。

### 许可证

[MIT](LICENSE)

---

## English

### What it does

Mounts Quark Drive (夸克网盘) as a native macOS WebDAV volume at `/Volumes/LocalQuark`. Use Finder as if it were a local disk:

- Drag-and-drop upload / download
- Open and edit `.docx`, `.md`, etc. directly (saves go straight to the cloud)
- Stream large files without downloading them first
- Auto-reconnect on mount loss (8-second watchdog)

### How it works

A local Rust process that fronts Quark Drive's private REST API as an HTTPS WebDAV server at `https://127.0.0.1:8443`. macOS's built-in `mount_webdav` connects to it.

```
Finder (macOS webdavfs_agent)
       │
       ▼  https://127.0.0.1:8443
quarkdrive-webdav (this repo, Rust)
       │
       ▼  HTTPS + private REST
pan.quark.cn
```

### Install

Download a release `.app` from [Releases](https://github.com/hawkyin-hub/quarkdrive-webdav-rust/releases), or build from source (see [docs/install.md](docs/install.md)).

### Quick start

1. Sign in to [pan.quark.cn](https://pan.quark.cn) in Chrome / Brave / Arc / Edge.
2. Run `.../install-helper.sh` once to grant Cookie access.
3. Launch `LocalQuark-rust.app`; Finder opens `/Volumes/LocalQuark` automatically.
4. Drag files in.

Admin panel: [http://127.0.0.1:8444](http://127.0.0.1:8444)

### Project status

This project is in active maintenance but is **not** affiliated with Quark Drive or its operators. Use at your own risk and respect Quark Drive's terms of service.

### Contributing

PRs welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

### License

[MIT](LICENSE)
