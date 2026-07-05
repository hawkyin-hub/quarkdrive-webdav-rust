<!-- headroom:rtk-instructions -->
# RTK (Rust Token Killer) - Token-Optimized Commands

When running shell commands, **always prefix with `rtk`**. This reduces context
usage by 60-90% with zero behavior change. If rtk has no filter for a command,
it passes through unchanged — so it is always safe to use.

## Key Commands
```bash
# Git (59-80% savings)
rtk git status          rtk git diff            rtk git log

# Files & Search (60-75% savings)
rtk ls <path>           rtk read <file>         rtk grep <pattern>
rtk find <pattern>      rtk diff <file>

# Test (90-99% savings) — shows failures only
rtk pytest tests/       rtk cargo test          rtk test <cmd>

# Build & Lint (80-90% savings) — shows errors only
rtk tsc                 rtk lint                rtk cargo build
rtk prettier --check    rtk mypy                rtk ruff check

# Analysis (70-90% savings)
rtk err <cmd>           rtk log <file>          rtk json <file>
rtk summary <cmd>       rtk deps                rtk env

# GitHub (26-87% savings)
rtk gh pr view <n>      rtk gh run list         rtk gh issue list

# Infrastructure (85% savings)
rtk docker ps           rtk kubectl get         rtk docker logs <c>

# Package managers (70-90% savings)
rtk pip list            rtk pnpm install        rtk npm run <script>
```

## Rules
- In command chains, prefix each segment: `rtk git add . && rtk git commit -m "msg"`
- For debugging, use raw command without rtk prefix
- `rtk proxy <cmd>` runs command without filtering but tracks usage
<!-- /headroom:rtk-instructions -->

---

# LocalQuark-Rust 项目指引

> 仓库：`/Users/HawkSept/myproject/myapp/localquark-rust`
> 单 crate Rust 应用 + macOS 单 `.app` 打包。
> 最后更新：2026-07-05

## 项目结构

| 路径 | 角色 | 备注 |
|------|------|------|
| `quarkdrive-webdav/` | Rust daemon crate（活跃代码） | 唯一需编译的二进制 |
| `quarkdrive-webdav/src/vfs.rs` | 虚拟文件系统核心 | 见下方红色区域 |
| `quarkdrive-webdav/src/webdav.rs` | HTTP 请求处理器 | 见下方红色区域 |
| `quarkdrive-webdav/src/mount.rs` | macOS `mount_webdav` 封装 | 见下方红色区域 |
| `quarkdrive-webdav/src/cookie.rs` | Chromium Cookie 解密 |  |
| `quarkdrive-webdav/src/health.rs` | 健康检查 + 自动重挂 |  |
| `quarkdrive-webdav/src/tray.rs` | 菜单栏托盘（macOS 独占） |  |
| `quarkdrive-webdav/src/proxy.rs` | HTTPS 终结代理（8443） | webdavfs_agent 强制要求 HTTPS |
| `quarkdrive-webdav/src/cache.rs` | Moka 内存缓存 |  |
| `scripts/build-app.sh` | 单 `.app` 打包脚本 | 输出 `dist/LocalQuark-rust.app` |
| `docs/` | 项目文档 | 架构 / API / 部署 / 排错 |
| `docs/archive/rust-migration.md` | 历史设计草案 | 仅作参考，不要按其实现 |
| `legacy/` | 旧 Python/Tauri 产物 | 不参与编译，纯历史 |
| `localquark-app/` | Tauri 空壳 | 已 deprecated |

## 常用命令

```bash
# 进入活跃 crate
cd quarkdrive-webdav

# 仅编译二进制（开发期）
rtk cargo build --release

# 编译并打包成单一 macOS .app
./scripts/build-app.sh

# 直接启动二进制（手动测试，跳过 .app）
./target/release/quarkdrive-webdav \
  --quark-cookie "sl-session=xxx; __pus=yyy" \
  --mount-point ~/Mount/Quark

# 仅 server 模式（不挂载、不菜单栏、不健康检查）
./target/release/quarkdrive-webdav --serve-only

# 健康检查子命令（打印后退出）
./target/release/quarkdrive-webdav health
```

## 红色区域（修改前必须确认）

为保证挂载、上传、Finder 兼容稳定，以下模块除非有充分理由和测试，否则不要改：

> ⚠️ 上方历史注释已经过时。**Phase 1 实际已回滚为串行上传**，因为 Quark API
> 元数据返回 `part_thread:1`，并发会触发 `PartNotSequential`。**Phase 2 / Phase 3 真实生效**。
> 详细真实状态与可复现基准：[docs/PERFORMANCE.md](docs/PERFORMANCE.md)
- `quarkdrive-webdav/src/webdav.rs` — `dav-server` 协议适配层
- `quarkdrive-webdav/src/mount.rs` — `mount_webdav -S -o url=...` 包装，macOS 版本差异敏感
- `quarkdrive-webdav/src/proxy.rs` — HTTPS 终结代理，`mount_webdav` 强依赖

## 长期记录（每个项目一个）

| 文件 | 用途 | 触发时机 |
|------|------|---------|
| `bug修复经验.md`（位于项目根） | 每次 bug 修复的真实经验 | 用户**已确认**修复成功后立即追加 |

### `bug修复经验.md` 写法要求

每条修复记录采用「**现象 → 根因 → 改动 → 验证 → 经验**」五段式，不写完不许划上 complete。
每条要保留：用户原话 / 日志片段 / 关键行号 / 复现命令，确保下次同症可直接命中。

未确认的修复**不许**写入，因为可能引入误导。

## 测试快捷脚本

任何源码改动（性能优化、bug fix、协议层调整）后，跑一次端到端部署验证：

```bash
./scripts/build_deploy_test.sh
```

该脚本一次性完成：

1. `cargo build --release`（活跃 crate 是 `quarkdrive-webdav/`）
2. `scripts/build-app.sh` 打包成 `.app`
3. 杀掉旧 `quarkdrive-webdav` / `run-localquark.sh` 并 `diskutil unmount force /Volumes/LocalQuark`
4. 安装到 `/Applications/LocalQuark-rust.app`
5. `open -a LocalQuark-rust` 重启
6. 轮询 `mount` 状态直到 `/Volumes/LocalQuark` 出现，最长等 30s

> **规则**：所有"测试用"的重复命令（编译→打包→安装→杀进程→启动→验证）一律放进 sh 脚本里，
> 不要再逐步手动执行。每个阶段之间保留 `sleep`，让 mount/proxy 有时间稳定。

## 关键设计决策（不要随意推翻）

1. **HTTPS 终结代理**：macOS 26.6 的 `webdavfs_agent` 拒绝 HTTP + Basic Auth（`Authentication method (Basic) too weak`），所以 daemon 内部架构必须是 `backend (HTTP 8080) → proxy (HTTPS 8443) → mount_webdav`。详见 `docs/DEPLOYMENT.md` 与 `ARCHITECTURE.md`。
2. **挂载点默认值有两套**：CLI 二进制默认 `~/Mount/Quark`；`scripts/build-app.sh` 打包后默认 `/Volumes/LocalQuark`（`mount.rs` 会 `create_dir_all`，所以两者都生效）。
3. **WebDAV 凭据**：`auth_user` 默认 `quasar`，`auth_password` 启动时随机生成并落盘 `~/Library/Application Support/QuarkDrive/webdav.passwd`（0o600）。CLI 示例用 `admin:admin` 是历史示例，不要复制到文档里。
4. **Cookie 来源**：仅从 Chromium 系浏览器（Chrome / Brave / Edge / Arc / Chromium）抓取。不支持 Firefox。

## 性能基准（已验证）

| 指标 | 优化前 | 优化后 | 备注 |
|------|--------|--------|------|
| 5MB 文件上传 | _TBD_ | _TBD_ | Phase 1 实际回滚为串行；5MB 上传当前是 API 串行耗时 |
| 目录列表响应 | ~2s | ~200ms | Phase 2 删除多余 `sleep` |
| 内存占用 (RSS) | ~50MB | ~37MB | Phase 3 移除 `flush()` |

> 上表数字以 [docs/PERFORMANCE.md](docs/PERFORMANCE.md) 为准，"优化前"列只用于历史记录。

## 文档索引

| 文档 | 用途 |
|------|------|
| [README.md](README.md) | 项目介绍、快速开始 |
| [ARCHITECTURE.md](ARCHITECTURE.md) | 模块职责、数据流图 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 贡献流程、提交规范 |
| [CHANGELOG.md](CHANGELOG.md) | 版本变更 |
| [docs/API.md](docs/API.md) | CLI / WebDAV / 环境变量参考 |
| [docs/install.md](docs/install.md) | 安装步骤速览 |
| [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) | 单 `.app` 打包 + 部署 |
| [docs/PERFORMANCE.md](docs/PERFORMANCE.md) | 性能基线 / 可复现测试 / Phase 真实状态 |
| [docs/troubleshooting.md](docs/troubleshooting.md) | 排错手册 |
| [docs/archive/rust-migration.md](docs/archive/rust-migration.md) | 历史设计草案（仅参考） |

## 重复命令固化（脚本清单）

按用户规则"测试相关的重复命令全部先变成 sh 文件"，已沉淀：

| 脚本 | 用途 | 是否重新编译 |
|------|------|--------------|
| `scripts/build-app.sh` | 单 `.app` 打包（产物 `dist/LocalQuark-rust.app`） | 否（依赖已编译的 `target/release/quarkdrive-webdav`） |
| `scripts/build_deploy_test.sh` | 编译 → 打包 → 装到 /Applications → 启动 → 等挂载（代码改动后用） | **是** |
| `scripts/restart_app.sh` | 杀掉旧服务 → 同步最新 cookie → 启动 .app → 验证 PROPFIND（cookie 过期时用） | 否 |

**何时跑哪个**：
- 改了 Rust 代码 → `scripts/build_deploy_test.sh`
- cookie 过期导致网盘加载失败 → `scripts/restart_app.sh`
- 想重新打 .app 但不装到系统 → `scripts/build-app.sh`

## 新增规则（由用户纠正沉淀）

1. **复杂 Bug 严禁高频猜测性试错**：针对多并发穿透、协议死锁、系统句柄冲突等疑难杂症，必须首先全面静态审查已知的 Python 遗留实现（尤其是 `legacy/` 目录下解码的完整代码）和环境状态。在整理出具有确定性的根因逻辑链条之前，禁止通过“发现一个疑似问题就运行一次编译部署”的方式盲目高频测试，避免浪费 Token 和开发时间。

