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

# quarkdrive-webdav 模块指引

> crate 路径：`/Users/HawkSept/myproject/myapp/localquark-rust/quarkdrive-webdav`
> 二进制：`target/release/quarkdrive-webdav`
> 最后更新：2026-07-05

## 模块职责速查

| 模块 | 行数级 | 职责 | 红色区域 |
|------|--------|------|---------|
| `src/main.rs` | ~14K | 启动编排、CLI 解析、信号处理 |  |
| `src/lib.rs` | ~10 行 | 模块声明 |  |
| `src/vfs.rs` | ~70K | `DavFileSystem` 实现 + 夸克 API 对接 | ⚠️ **核心，禁止随意改动** |
| `src/webdav.rs` | ~40K | `WebDavServer` + dav-server handler | ⚠️ **核心** |
| `src/mount.rs` | ~4K | `mount_webdav -S -o url=...` 包装 | ⚠️ **macOS 版本敏感** |
| `src/cookie.rs` | ~13K | Chromium Cookie 解密（Keychain + PBKDF2 + AES-CBC） |  |
| `src/health.rs` | ~4K | 60s 周期健康检查 + 自动修复 |  |
| `src/tray.rs` | ~5K | macOS 菜单栏托盘 |  |
| `src/proxy.rs` | ~15K | HTTPS 终结代理（8443） | ⚠️ **架构关键** |
| `src/cache.rs` | ~5K | Moka 内存缓存 |  |
| `src/notifier.rs` | <1K | osascript 通知包装 |  |
| `src/drive/` | — | 夸克 API 客户端（不变更） |  |

## 红色区域说明

### `vfs.rs` — 性能优化锚点

修改前必须先理解以下三阶段优化，它们是经验证过的稳定性与性能平衡：

| 优化 | 位置 | 行为 | 警告 |
|------|------|------|------|
| **Phase 1** | `upload_chunk` | `buffer_unordered(4)` 并发上传分片 | 不要降到 2 或升到 8 — 4 是带宽/拥塞平衡点 |
| **Phase 2** | `remove_file` / `remove_dir` / 清除缓存后 | 移除 `sleep(1s)` / `sleep(2s)` | 缓存 invalidate 已足够，不需要额外 sleep |
| **Phase 3** | `consume_buf` | 移除 `file.flush()` | 关闭文件句柄时 OS 自动 flush；不要加回 |

性能基准详见根 [CHANGELOG.md](../CHANGELOG.md)。

### `mount.rs` — macOS 差异敏感

几个细节踩过坑：

1. 必须在调用 `mount_webdav` 之前 `create_dir_all(mount_point)`，否则 webdavfs_agent 找不到挂载点会报 `Operation not permitted`。
2. `-S` 标志允许自签证书（HTTPS 终结代理必需）。
3. `username=` / `password=` 中含 `,` `=` 时用 POSIX 单引号转义（见 `shell_escape`）。
4. `is_mounted` 通过 `/sbin/mount` 文本匹配验证；如未来 macOS 改输出格式，需要同步更新。

### `proxy.rs` — HTTPS 终结代理

存在意义：macOS 26.6 的 `webdavfs_agent` 拒绝 HTTP + Basic Auth，错误原文 `Mount failed, Authentication method (Basic) too weak`。所以架构必须是：

```
quarkdrive-webdav backend  127.0.0.1:8080  (HTTP, 只本地)
        |
        v
quarkdrive-webdav proxy    127.0.0.1:8443  (HTTPS, 自签证书)
        |
        v
mount_webdav -S            https://127.0.0.1:8443  /Volumes/LocalQuark
```

不要把 8080 端口暴露到对外，**只** 8443 是 mount_webdav 的目标。

## 开发流程

```bash
cd quarkdrive-webdav

# 编译检查（增量）
rtk cargo check

# 编译 release
rtk cargo build --release

# 格式化 + 静态检查
rtk cargo fmt
rtk cargo clippy --all-targets -- -D warnings

# 单测（覆盖 cookie 解密、shell_escape 等纯逻辑）
rtk cargo test

# 启动并打日志
RUST_LOG=quarkdrive_webdav=debug,reqwest=warn \
  ./target/release/quarkdrive-webdav --debug
```

## 提 PR 前清单

- [ ] `rtk cargo fmt` 无 diff
- [ ] `rtk cargo clippy --all-targets -- -D warnings` 零警告
- [ ] `rtk cargo test` 全绿
- [ ] 修改涉及 `vfs.rs` / `webdav.rs` / `mount.rs` / `proxy.rs` 时附手动测试记录（上传/挂载/Finder 显示）
- [ ] `CHANGELOG.md` 加条目
