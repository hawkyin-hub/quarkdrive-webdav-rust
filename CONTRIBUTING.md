# Contributing to QuarkDrive-WebDAV

> 仓库路径：`/Users/HawkSept/myproject/myapp/localquark-rust/`
> 单 crate Rust 应用 + macOS 单 `.app` 打包。
> 最后更新：2026-07-05

感谢你对 QuarkDrive-WebDAV 项目的关注！以下是一些帮助你快速参与开发并提交高质量贡献的指南。



---

## 开发环境准备

### 前置要求
- **Rust**：版本 >= 1.80（`rustc --version` 检查）
- **Cargo**：Rust 自带的包管理器
- **macOS**：本项目主要针对 macOS 开发（挂载、托盘、Cookie 抓取均依赖 macOS API）

### 仓库位置

活跃开发代码在本机统一目录：

```bash
# /Users/HawkSept/myproject/myapp/localquark-rust/
cd /Users/HawkSept/myproject/myapp/localquark-rust/quarkdrive-webdav
```

> ⚠️ 旧的 `~/Documents/localquark-rust/` 已废弃，所有改动请在上方路径进行。

### 克隆与构建（二进制）

```bash
cd /Users/HawkSept/myproject/myapp/localquark-rust/quarkdrive-webdav
cargo build --release
# 产物：./target/release/quarkdrive-webdav
```

### 构建 macOS 单 `.app`（推荐）

```bash
# 仓库根目录执行
cd /Users/HawkSept/myproject/myapp/localquark-rust
./scripts/build-app.sh
# 产物：./dist/LocalQuark-rust.app
# 双击即可：自动挂载到 /Volumes/LocalQuark 并打开 Finder
```

### 运行测试

```bash
cd quarkdrive-webdav
cargo test              # 单元测试
cargo clippy            # 代码检查
cargo fmt --check       # 格式化检查
```

---

## 项目结构

```
localquark-rust/                        # 仓库根（本机路径 ~/myproject/myapp/localquark-rust/）
├── quarkdrive-webdav/                   # 唯一活跃 Rust crate
│   ├── src/
│   │   ├── main.rs      # 启动编排、CLI 解析、信号处理
│   │   ├── lib.rs       # 模块声明
│   │   ├── vfs.rs       # ⚠️ 虚拟文件系统核心（红色区域）
│   │   ├── webdav.rs    # ⚠️ dav-server 协议适配（红色区域）
│   │   ├── mount.rs     # ⚠️ macOS mount_webdav 封装（红色区域）
│   │   ├── proxy.rs     # ⚠️ HTTPS 终结代理 8443（红色区域）
│   │   ├── cookie.rs    # Chromium Cookie 解密
│   │   ├── health.rs    # 健康检查 + 自动修复
│   │   ├── tray.rs      # macOS 菜单栏托盘
│   │   ├── cache.rs     # Moka 内存缓存
│   │   └── notifier.rs  # osascript 通知
│   ├── drive/           # 夸克 API 客户端
│   └── Cargo.toml
├── scripts/
│   └── build-app.sh     # 单 .app 打包脚本（红色区域）
├── docs/                # 项目文档
│   ├── API.md
│   ├── DEPLOYMENT.md
│   ├── install.md
│   ├── troubleshooting.md
│   └── archive/
├── ARCHITECTURE.md
├── CHANGELOG.md
├── CONTRIBUTING.md
├── README.md
└── AGENTS.md            # 项目级指引（含红色区域 + 性能锚点）
```

详见 [AGENTS.md](AGENTS.md) 中"红色区域"段。

---

## 代码规范

### 格式化
- 使用 `cargo fmt` 统一代码格式
- 提交前务必执行 `cargo clippy -- -D warnings`

### 命名规范
- Rust 标准命名：`snake_case`（函数/变量），`CamelCase`（类型/枚举）
- 模块名：`snake_case`
- 常量：全大写 `SCREAMING_SNAKE_CASE`

### 错误处理
- 优先使用 `anyhow::Result<T>` 或 `thiserror` 自定义错误
- 避免裸 `unwrap()` / `expect()`，生产代码必须处理错误

---

## 提交规范

我们使用 [Conventional Commits](https://www.conventionalcommits.org/) 规范，格式如下：

```
<type>(<scope>): <subject>

<body>

<footer>
```

### Type 类型
| 类型 | 说明 |
|------|------|
| `feat` | 新增功能 |
| `fix` | Bug 修复 |
| `perf` | 性能优化 |
| `docs` | 文档更新 |
| `refactor` | 重构（不改变行为）|
| `test` | 测试相关 |
| `chore` | 构建/工具/依赖更新 |

### 示例
```
feat(mount): 支持 mount_webdav -S 参数

允许在遇到非标准 WebDAV 服务器时启用兼容模式。

fix(vfs): 修复大文件上传内存泄漏

perf(upload): upload_chunk 改为 buffered(4) 并发
```

---

## PR 流程

1. **Fork** 仓库，创建 feature 分支：`git checkout -b feat/your-feature`
2. **开发** 并确保通过 `cargo test && cargo clippy`
3. **提交** 符合 Conventional Commits 规范的 commit
4. **推送** 分支到 fork：`git push origin feat/your-feature`
5. **提交 PR**，描述清楚：
   - 改了什么
   - 为什么改
   - 测试方式和结果

---

## 🚫 红色区域（请勿修改）

为保证挂载、上传、Finder 兼容稳定，以下模块除非有充分理由和测试，否则**不要改**：

| 文件 | 修改风险 | 性能/挂载锚点 |
|------|----------|----------------|
| `quarkdrive-webdav/src/vfs.rs` | 上传/目录性能、缓存失效逻辑 | ❌ Phase 1 `buffer_unordered(4)` 并发已回滚为串行（Quark API `part_thread:1`）；✅ Phase 2 删除 `sleep(1s)/sleep(2s)`；✅ Phase 3 删除 `consume_buf.flush()` |
| `quarkdrive-webdav/src/webdav.rs` | HTTP 协议适配层，PROPFIND/PUT/DELETE |  |
| `quarkdrive-webdav/src/mount.rs` | `mount_webdav -S -o url=...` macOS 版本差异 | `create_dir_all` + `shell_escape` + `is_mounted` |
| `quarkdrive-webdav/src/proxy.rs` | HTTPS 终结代理，mount_webdav 强依赖 | 8080 HTTP backend → 8443 HTTPS frontend |
| `scripts/build-app.sh` | `.app` 启动器默认值；改了之后 bundled app 挂载路径会变 |  |

具体锚点源码注释详见 `quarkdrive-webdav/AGENTS.md` 中"红色区域说明"段。

---

## 测试策略

| 模块 | 测试类型 | 覆盖率目标 |
|------|---------|-----------|
| `cookie.rs` | 单元测试（合成 ciphertext） | 核心解密路径 |
| `mount.rs` | Mock 子进程测试 | 参数验证 |
| `health.rs` | 单元测试（故障组合） | 三种故障路径 |
| `vfs.rs` | 集成测试（IT） | 与 Quark API 交互 |
| `tray.rs` | 手动测试 | GUI 交互 |

---

## 安全须知

- **Cookie 文件**：`cookie.rs` 处理用户浏览器 Cookie，必须注意内存安全，禁止日志打印敏感信息
- **TLS 证书**：自签名证书仅用于开发，生产环境建议使用受信任 CA 证书
- **挂载权限**：`mount_webdav` 涉及系统调用，测试时注意不要在关键路径挂载

---

## 获取帮助

- 提交 Issue 前请先搜索已有 Issue
- 提供详细的复现步骤、环境信息、日志输出
- 性能问题请附带 `cargo bench` 或火焰图

---

再次感谢你的贡献！🙏

---

*最后更新：2026-07-05*
