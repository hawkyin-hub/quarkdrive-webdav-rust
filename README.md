# QuarkDrive-WebDAV

> 将夸克网盘挂载为本地磁盘，用 Finder 直接浏览、编辑、传输文件。

[![Crates.io](https://img.shields.io/crates/v/quarkdrive-webdav.svg)](https://crates.io/crates/quarkdrive-webdav)
[![Docker Image](https://img.shields.io/badge/version-latest-blue)](https://ghcr.io/chenqimiao/quarkdrive-webdav)

> 仓库根：`/Users/HawkSept/myproject/myapp/localquark-rust`
> 活跃 crate：`quarkdrive-webdav/`
> 文档版本：2026-07-05

---

## 核心特性

| 特性 | 说明 |
|------|------|
| 🎯 **Finder 直接访问** | 通过 macOS 原生 `mount_webdav` 挂载，Finder 无需任何客户端即可使用 |
| 🔐 **Cookie 自动抓取** | 从 Chrome / Brave / Edge / Arc 等浏览器自动解密提取夸克 Cookie，无需手动复制 |
| ⚡ **上传优化** | Phase 2 删除多余 `sleep`、Phase 3 移除 `consume_buf.flush()`；Phase 1 并发版本因 Quark API 限制已回滚为串行（详见 [docs/PERFORMANCE.md](docs/PERFORMANCE.md)） |
| 🛡️ **健康检查与自动恢复** | 每 60 秒检测服务与挂载状态，故障时自动重连 |
| 🖥️ **菜单栏托盘** | 原生 macOS 托盘图标，随时查看状态、刷新 Cookie、在 Finder 中显示 |
| 🔒 **HTTPS 终结代理** | 自签名证书 + macOS 26.6+ 的 `webdavfs_agent` 强制 HTTPS（拒绝 HTTP+Basic Auth） |
| 📦 **单 `.app` 打包** | `scripts/build-app.sh` 一键生成 `dist/LocalQuark-rust.app`，自动起 daemon + 挂载 + 打开 Finder |

---

## 快速开始

### 前置要求

- macOS 13+（Monterey 或更新版本；macOS 26.6+ 的 `webdavfs_agent` 强制 HTTPS）
- Rust 1.80+（从源代码构建时）
- Chrome / Brave / Edge / Arc 任一浏览器已登录夸克网盘
- Xcode Command Line Tools（含 `mount_webdav`、`/usr/bin/openssl`）

### 路径 A：直接启动二进制（开发期）

```bash
git clone https://github.com/chenqimiao/quarkdrive-webdav.git
cd quarkdrive-webdav
cargo build --release
./target/release/quarkdrive-webdav
```

CLI 默认值：

| 参数 | 默认 | 说明 |
|------|------|------|
| `--host` / `--port` | `127.0.0.1:8443` | HTTPS 终结代理监听地址（mount_webdav 连接的目标） |
| `--backend-host` / `--backend-port` | `127.0.0.1:8080` | HTTP WebDAV 后端（仅本地） |
| `--mount-point` | `~/Mount/Quark` | CLI 模式挂载点 |
| `--webdav-auth-user` | `quasar` | WebDAV 用户名 |
| `--webdav-auth-password` | 自动生成并落盘 | 持久化到 `~/Library/Application Support/QuarkDrive/webdav.passwd`（0o600） |

### 路径 B：打包成单一 macOS `.app`（用户期）

```bash
# 在仓库根目录执行
./scripts/build-app.sh
# 产物：dist/LocalQuark-rust.app
open dist/LocalQuark-rust.app
```

打包脚本会把 Rust 二进制 + 自签证书生成器 + `run-localquark.sh` 启动器一并塞进 `.app/Contents/Resources/bin/`，launcher 自动起 daemon + 挂载 + 打开 Finder。详细流程见 [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)。

打开 `.app` 后会自动：

1. 从你的浏览器解密提取夸克 Cookie
2. 启动 WebDAV 后端（HTTP 8080，仅本地）
3. 启动 HTTPS 终结代理（8443，自签证书）
4. 调用 `mount_webdav -S` 挂载到 `/Volumes/LocalQuark`（bundled app 默认值）
5. 在 Finder 中打开挂载点
6. 在菜单栏显示状态图标

### 挂载点（两条路径不同）

| 启动方式 | 默认挂载点 | 覆盖方式 |
|----------|-----------|----------|
| 直接跑二进制 | `~/Mount/Quark`（经 `expand_home` 展开） | `--mount-point` 或 `MOUNT_POINT` 环境变量 |
| 跑 bundled `.app` | `/Volumes/LocalQuark` | 设置 `LOCALQUARK_MOUNT_POINT` 后重新打包 |

`mount.rs` 内部会先 `create_dir_all(mount_point)`，所以两个默认都生效；如要换名字（如 `/Volumes/Quark`），把 `LOCALQUARK_MOUNT_POINT=/Volumes/Quark` 传给 launcher 即可。

### 从 GitHub Releases 下载

访问 [Releases 页面](https://github.com/chenqimiao/quarkdrive-webdav/releases) 下载 macOS 二进制包，赋予执行权限后运行。

### Docker 部署

```bash
docker run -d --name=quarkdrive-webdav --restart=unless-stopped -p 8080:8080 \
  -e QUARK_COOKIE='your quark cookie' \
  -e WEBDAV_AUTH_USER=quasar \
  -e WEBDAV_AUTH_PASSWORD=secret \
  ghcr.io/chenqimiao/quarkdrive-webdav:latest
```

> Docker 部署时只暴露 8080 端口；要在 macOS Finder 挂载必须再起一层 HTTPS 终结代理（见 `quarkdrive-webdav/src/proxy.rs` 设计）。

### 启动后

> 详见上文「路径 B」。

打开 Finder，侧栏点击 **LocalQuark**，即可开始使用。

---

## 架构概览

以下是系统高级架构图：

```
┌─────────────────────────────────────────────────────────┐
│                     macOS 系统                          │
│                                                         │
│   Finder  ──HTTPS─▶  QuarkDrive-WebDAV  ──API──▶ 夸克网盘
│                     ├─ backend  HTTP 8080 (本地)        │
│                     ├─ proxy    HTTPS 8443 (终结)       │
│                     ├─ cookie  (Keychain 抓取)          │
│                     ├─ health  (60s 周期)               │
│                     └─ tray    (菜单栏)                  │
└─────────────────────────────────────────────────────────┘
```

> 为什么需要 HTTPS 终结代理：macOS 26.6+ 的 `webdavfs_agent` 拒绝 HTTP + Basic Auth（错误原文 `Authentication method (Basic) too weak`）。架构上 backend 仍保留 HTTP 是为了开发期 `curl` 调试方便；对外只暴露 8443 HTTPS。详见 [ARCHITECTURE.md](ARCHITECTURE.md) 与 [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)。

有关详细架构、模块职责和数据流图，请参考 [ARCHITECTURE.md](ARCHITECTURE.md)。

---

## 配置指南

### 命令行参数

所有配置均可通过命令行参数或环境变量设置。**显式命令行参数 > 环境变量 > 内置默认值**。

| 参数 | 环境变量 | 描述 | 默认值 |
|------|----------|------|--------|
| `-c, --quark-cookie <COOKIE>` | `QUARK_COOKIE` | 夸克网盘 Cookie（分号分隔的键值对） | 从浏览器自动抓取 |
| `-U, --webdav-auth-user <USER>` | `WEBDAV_AUTH_USER` | WebDAV 认证用户名 | `quasar` |
| `-W, --webdav-auth-password <PASS>` | `WEBDAV_AUTH_PASSWORD` | WebDAV 认证密码 | 自动生成的 24 字节随机令牌，落盘 `webdav.passwd` |
| `--host <HOST>` | `HOST` | HTTPS 终结代理监听地址（对外） | `127.0.0.1` |
| `--port <PORT>` | `PORT` | HTTPS 终结代理端口（对外） | `8443` |
| `--backend-host <HOST>` | `BACKEND_HOST` | HTTP WebDAV 后端监听地址（仅本地） | `127.0.0.1` |
| `--backend-port <PORT>` | `BACKEND_PORT` | HTTP WebDAV 后端端口 | `8080` |
| `--tls-cert <CERT>` | `TLS_CERT` | TLS 证书路径 | 自动生成的自签名证书（`~/Library/Application Support/QuarkDrive/cert.pem`） |
| `--tls-key <KEY>` | `TLS_KEY` | TLS 私钥路径 | 自动生成的自签名私钥（`~/Library/Application Support/QuarkDrive/key.pem`） |
| `--mount-point <PATH>` | `MOUNT_POINT` | CLI 模式挂载点路径 | `~/Mount/Quark` |
| `--serve-only` | — | 仅 server 模式（不挂载、不菜单栏、不健康检查） | `false` |
| `--cookie-refresh-secs <N>` | — | Cookie 后台刷新周期（秒） | `43200` |
| `--health-check-secs <N>` | — | 健康检查周期（秒） | `60` |
| `--debug` | — | 启用 debug 日志 | `false` |
| `--warm-root <BOOL>` | — | 挂载前根 PROPFIND 预热（失败不阻塞） | `true` |
| `--no-mount` | `NO_MOUNT` | 禁用自动挂载（bundled app 用） | `false` |
| `--no-tray` | `NO_TRAY` | 禁用菜单栏托盘（bundled app 用） | `false` |
| `-h, --help` | — | 显示帮助信息 |  |

### 环境变量映射

所有命令行参数都有对应的环境变量（见上表）。例如：
- `QUARK_COOKIE` 对应 `--quark-cookie`
- `WEBDAV_AUTH_USER` 对应 `--webdav-auth-user`
- `PORT` 对应 `--port`

在 Docker 或系统服务中，推荐使用环境变量进行配置。

### bundled app launcher 专用环境变量

`scripts/build-app.sh` 生成的 `LocalQuark` launcher 会读取：

| 变量 | 默认 | 说明 |
|------|------|------|
| `LOCALQUARK_HOST` | `127.0.0.1` | 同 `--host` |
| `LOCALQUARK_PORT` | `8443` | 同 `--port` |
| `LOCALQUARK_AUTH_USER` | `ujRx4Js1D` | bundled app 默认用户名（**生产建议改**） |
| `LOCALQUARK_AUTH_PASSWORD` | `DVzv3ELQ3icn` | bundled app 默认密码（**生产建议改**） |
| `LOCALQUARK_MOUNT_POINT` | `/Volumes/LocalQuark` | bundled app 挂载点 |
| `LOCALQUARK_DETACH` | `1` | `1` = launcher 派生 daemon 后立即返回 |
| `LOCALQUARK_LOG_FILE` | `~/Library/Logs/LocalQuark-rust-webdav.log` | daemon 日志 |

---

## 性能优化

本项目经过三轮系统性性能优化：

| 阶段 | 优化内容 | 效果 |
|------|---------|------|
| **Phase 1** | ❌ `upload_chunk` 尝试 `buffer_unordered(4)` 并发；因 Quark API `part_thread:1` 限制触发 `PartNotSequential`，已回滚为串行 | 历史版本 5MB 上传曾达 ~8s；当前为 API 串行耗时 |
| **Phase 2** | 删除 8 处多余的 `sleep` 延迟（缓存同步） | 目录列表从 ~2s → ~200ms |
| **Phase 3** | 移除 `consume_buf` 的每写 `flush()` | 减少磁盘 IO syscall，内存占用 -26% |

源码中的锚点位置：

- Phase 1：`quarkdrive-webdav/src/vfs.rs` 中 `do_flush` 的 spawn 任务、`upload_chunk` 函数（line 1341+），循环是 `for chunk_idx in 1..= chunk_count`
- Phase 2：`vfs.rs` 的 `remove_file` / `remove_dir` / 缓存清除路径
- Phase 3：`vfs.rs::consume_buf`

> 真实状态与可复现测试见 [docs/PERFORMANCE.md](docs/PERFORMANCE.md)。

---

## 文档目录

| 文档 | 说明 |
|------|------|
| [README.md](README.md) | 项目介绍（本文件） |
| [ARCHITECTURE.md](ARCHITECTURE.md) | 系统架构、模块职责、数据流图 |
| [docs/API.md](docs/API.md) | 命令行参数、WebDAV 接口、环境变量 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 开发指南、提交规范 |
| [CHANGELOG.md](CHANGELOG.md) | 版本历史与变更记录 |
| [docs/install.md](docs/install.md) | 详细安装指南 |
| [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) | 单 `.app` 打包与部署流程 |
| [docs/PERFORMANCE.md](docs/PERFORMANCE.md) | 性能基线 / 可复现测试 / Phase 真实状态 |
| [docs/troubleshooting.md](docs/troubleshooting.md) | 常见问题与解决方案 |
| [docs/archive/](docs/archive/) | 历史文档归档 |

---

## 常见问题

### Finder 中挂载点显示为空？

1. 尝试在 Finder 中按 `Cmd + R` 刷新当前目录
2. 若仍为空，尝试重启应用：`killall LocalQuark-rust; open dist/LocalQuark-rust.app`
3. 检查 WebDAV 后端是否正常：`curl -X PROPFIND -u quasar:<webdav-passwd> http://127.0.0.1:8080/`
4. 检查 HTTPS 终结代理是否正常：`curl -k -X PROPFIND -u quasar:<webdav-passwd> https://127.0.0.1:8443/`
5. 查看 daemon 日志：`tail -f ~/Library/Logs/LocalQuark-rust-webdav.log`

完整排错手册见 [docs/troubleshooting.md](docs/troubleshooting.md)。

### 上传大文件时 Finder 挂起？

这是 macOS `mount_webdav` 的已知限制。建议通过命令行上传大文件或使用专门的 WebDAV 客户端（如 CyberDuck）。

### Cookie 抓取失败？

确保：
- 浏览器（Chrome 等）已登录夸克网盘并保持运行
- 浏览器未使用主密码（Master Password）加密 Cookie
- Keychain 访问权限未被系统限制
 - Keychain 中 `Chrome Safe Storage` 项存在（`security find-generic-password -s "Chrome Safe Storage" -w` 能返回明文）

---

## 鸣谢

感谢所有捐赠者和贡献者！

| 日期 | 渠道 | 捐赠者 | 金额 |
|:---:|:---:|:---:|:---:|
| 2026-06-06 | WeChat | J\*o | ¥100.00 |
| 2026-03-26 | WeChat | M\*u | ¥50.00 |
| 2026-03-25 | WeChat | \*途 | ¥10.00 |
| 2025-08-06 | WeChat | \*平 | ¥18.50 |
| 2025-05-04 | WeChat | L\*s | ¥100.00 |
| 2025-01-07 | WeChat | \*良 | ¥25.00 |
| **合计** |  | **5 位** | **¥303.50** |

---

## 🚨 免责声明

本项目仅供学习和研究目的，不得用于任何商业活动。用户在使用本项目时应遵守所在地区的法律法规，对于违法使用所导致的后果，本项目及作者不承担任何责任。
使用本项目即表示您已阅读并同意本免责声明的全部内容。

---

## 许可证

MIT License - 详见 [LICENSE](quarkdrive-webdav/LICENSE) 文件。
