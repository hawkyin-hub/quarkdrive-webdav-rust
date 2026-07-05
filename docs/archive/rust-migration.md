# QuarkDrive-WebDAV 全量 Rust 化架构设计

> 状态：草案 v1（待签字）
> 日期：2026-07-03
> 目标：消灭 Python 依赖，将 `LocalQuark-rust.app`（PyInstaller 打包）与
> `QuarkDrive-WebDAV.app`（Rust daemon）合并为**单一 Rust `.app`**。

---

## 1. 现状与痛点

| 问题 | 现象 | 根因 |
|---|---|---|
| 挂载总不成功 | Finder 无法弹密码框 / 弹完连不上 | Cookie 链路断裂：Python 版 `reader.py` 解不出 `sl-session`；`quark_cookie/cookies.json` 长期缺失关键字段 |
| 重挂是哑火 | 日志循环 `✗ [mount] 挂载点未挂载 ... 尝试重新挂载 ...` | `healthcheck.py` 调用 `mount/quark_mount.sh`，但仓库中**无此文件** |
| 双 app 互相依赖 | 一个负责抓 cookie，一个负责起 webdav | 架构分裂：cookie 抓取 (Python) 与 webdav 服务 (Rust) 分属两个 app，通过 `/tmp/quark_cookie_backup` 串接 |
| 源码漂移 | `LocalQuark/app/src/quark_cookie/` 仅剩 `.pyc` | Python 项目被裁切，源码不在版本控制中 |

---

## 2. 目标与不目标

**做**：

- **单一 `.app`**：`QuarkDrive-WebDAV.app`，内含一个 Rust 二进制。
- Rust 端实现 **cookie 抓取 → webdav 服务 → 健康检查 → 自动重挂 → 菜单栏 UI**。
- macOS menu bar 托盘图标：状态 / 立即刷新 / 在 Finder 中显示 / 退出。
- 12 小时定时刷新 cookie；菜单里"立即刷新"按钮可手动触发。
- 启动后自动调用 `mount_webdav` 挂到 `~/Mount/Quark`（macOS 原生命令，零三方依赖）。

**不做**：

- 不引入 macFUSE / fuse-t。
- 不写 LaunchAgent 安装器（暂时用 `launchctl bootstrap` 用户态 inline；后续可补独立子命令）。
- 不重写 `drive` 模块的云盘 API 调用（保持现有 `reqwest` + 夸克官方端点）。
- 不实现上传大文件的分片策略变更（vfs.rs 的 Write Folding 已就绪）。

---

## 3. 仓库结构调整

```
localquark-rust/
├── quarkdrive-webdav/                # 现有 Rust crate（扩展）
│   ├── src/
│   │   ├── main.rs                   # 启动入口（重写）
│   │   ├── lib.rs
│   │   ├── cookie.rs                 # 【新】cookie 抓取
│   │   ├── mount.rs                  # 【新】mount_webdav 包装
│   │   ├── health.rs                 # 【新】健康检查 + 重挂
│   │   ├── notifier.rs               # 【新】osascript 通知
│   │   ├── tray.rs                   # 【新】macOS 菜单栏
│   │   ├── cache.rs                  # 现有
│   │   ├── drive/                    # 现有
│   │   ├── vfs.rs                    # 现有
│   │   └── webdav.rs                 # 现有
│   ├── Cargo.toml                    # 加依赖
│   └── dist/
│       └── QuarkDrive-WebDAV.app/    # 保留
└── localquark-app/                   # 废弃（Tauri 空壳，不删源码，标记 deprecated）
```

**删除**：`quarkdrive-webdav/dist/LocalQuark-rust.app/`（整个目录）。

---

## 4. 新增模块设计

### 4.1 `cookie.rs` —— Chromium cookie 抓取

**输入**：浏览器优先级（默认 `chrome → brave → edge → arc → chromium`）。
**输出**：`HashMap<String, String>`，键名如 `__pus`、`__puus`、`sl-session`、`isg`。

**算法**（与原 Python 版等价）：

1. `security find-generic-password -w -s "Chrome Safe Storage" -a "Chrome"` 拿 keychain 明文密码。
2. `pbkdf2_hmac("sha1", password, b"saltysalt", 1003, 16)` 派生 16 字节 AES key。
3. 把 `~/Library/Application Support/<Browser>/<Profile>/Cookies` 拷到 `tempfile::tempdir()`，避免锁库。
4. `PRAGMA table_info(cookies)` 拿列；过滤 `host_key LIKE %pan.quark.cn% OR %drive.quark.cn% OR %quark.cn%`。
5. `meta.version` ≥ 24 时，解密后剥前 32 字节 `SHA256(host_key)`。
6. ciphertext 以 `v10` / `v11` 起头；AES-128-CBC，IV 固定 `b" " * 16`。
7. PKCS#7 去 padding；UTF-8 解码失败 → 整条丢弃。

**接口**：

```rust
pub struct CookieStore { inner: Arc<DashMap<String, String>> }

impl CookieStore {
    pub async fn from_chromium(priority: &[BrowserKind]) -> Result<Self>;
    pub fn get(&self, key: &str) -> Option<String>;
    pub fn snapshot(&self) -> HashMap<String, String>;
}
```

### 4.2 `mount.rs` —— WebDAV 挂载

**命令**：

```bash
mount_webdav -S -o url=https://127.0.0.1:8443,username=<u>,password=<p> ~/Mount/Quark
```

**挂载密码**：用 `CookieStore::snapshot()` 派生一次随机 token，写到
`~/Library/Application Support/QuarkDrive/webdav.passwd`（0o600）。同时作为 `--auth-password` 传给 webdav。

**接口**：

```rust
pub struct MountConfig { pub mount_point: PathBuf, pub webdav_url: String, pub user: String, pub pass: String }
pub async fn mount(cfg: &MountConfig) -> Result<()>;
pub async fn is_mounted(point: &Path) -> bool;
pub async fn unmount(point: &Path) -> Result<()>;
```

### 4.3 `health.rs` —— 健康检查 + 自动重挂

**指标**：

- `CookieStore::get("sl-session")` 必须存在。
- webdav `OPTIONS /` 必须返回 `200`。
- `/sbin/mount` 输出包含挂载点路径。

**调度**：60s 一次（初版硬编码，后续读 config）。

**故障恢复**：

```
cookies 缺失  → 调 cookie::CookieStore::from_chromium()
webdav 不可达 → 杀进程 + 重启 webdav task
挂载掉了     → mount::mount()
```

**接口**：

```rust
pub struct HealthChecker { ... }
impl HealthChecker {
    pub async fn check(&self) -> HealthReport;
    pub async fn repair(&self, report: &HealthReport) -> Result<()>;
}
```

### 4.4 `notifier.rs` —— macOS 通知

封装 `osascript -e 'display notification "..." with title "..."'`，失败不抛。

```rust
pub fn notify(title: &str, body: &str);
```

### 4.5 `tray.rs` —— 菜单栏

**选项**：

| 菜单项 | 行为 |
|---|---|
| 状态：`在线` / `未挂载` / `Cookie 失效` | 动态更新（每次 tick 重查 `health`） |
| 立即刷新 cookie | `CookieStore::from_chromium()` |
| 在 Finder 中显示 | `open ~/Mount/Quark` |
| 退出 | 取消所有 task + `mount::unmount` + `std::process::exit(0)` |

**实现**：选 `tauri::SystemTray`（已有 Tauri 项目骨架）还是 `tray-icon`（轻量 crate）。建议 `tray-icon` —— Tauri 拉整个 WebView 太重，而我们不需要窗口。

```toml
tray-icon = "0.21"
```

### 4.6 `main.rs` —— 启动编排

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let cookie_store = CookieStore::from_chromium(&DEFAULT_BROWSERS).await?;
    let webdav_password = generate_token();
    save_password_to_disk(&webdav_password)?;
    let fs = build_fs(cookie_store.clone())?;
    spawn_webdav(fs, webdav_password.clone());
    mount::mount(&MountConfig { user, pass: webdav_password, ... })?;
    spawn_periodic_cookie_refresh(cookie_store, Duration::from_secs(12 * 3600));
    spawn_health_loop(cookie_store);
    tray::run(...).await;        // 阻塞，菜单栏退出触发 shutdown
    Ok(())
}
```

---

## 5. Cargo.toml 新增依赖

```toml
[dependencies]
rusqlite = { version = "0.32", features = ["bundled"] }
aes = "0.8"
cbc = "0.1"
pbkdf2 = "0.12"
sha1 = "0.10"   # 已有
hex = "0.4"     # 已有
security-framework = "3"   # macOS Keychain
tray-icon = "0.21"
tempfile = "3"   # 解锁 SQLite 副本
```

（其余 `reqwest`、`hyper`、`dav-server`、`moka`、`dashmap` 等保持现状。）

---

## 6. 安装 / 卸载

### 安装

```bash
cp -R dist/QuarkDrive-WebDAV.app /Applications/
open /Applications/QuarkDrive-WebDAV.app   # 首次启动生成自签证书 + 抓 cookie
```

不写 LaunchAgent（用户主动运行 `.app`）。退出后挂载点保留；下次启动 app 自动重新挂载。

### 卸载

```bash
umount ~/Mount/Quark
rm -rf /Applications/QuarkDrive-WebDAV.app
rm -rf ~/Library/Application\ Support/QuarkDrive
```

---

## 7. 测试策略

| 模块 | 测试 |
|---|---|
| `cookie` | 用合成 v10 ciphertext（已知 plaintext + key）做 round-trip；剥 `SHA256(host_key)` 前缀的边界用例 |
| `mount` | mock `mount_webdav` 子进程，验证参数 |
| `health` | 单元测试三种故障组合 + repair 路径 |
| `tray` | 跳过 GUI 测试（手测） |

---

## 8. 迁移 / 删除清单

- [ ] 删除 `quarkdrive-webdav/dist/LocalQuark-rust.app/` 整个目录
- [ ] 删除 `quarkdrive-webdav/dist/LocalQuark-rust.app/Contents/MacOS/_internal/scripts/install_agent.sh` 与 `uninstall.sh`（已被新方案取代）
- [ ] 标记 `localquark-app/` 为 deprecated（README 加一行）
- [ ] `quarkdrive-webdav/Cargo.toml` 删除 `quarkdrive-webdav` 元数据里的 `systemd.service` 引用（macOS 不需要）
- [ ] 更新根 `AGENTS.md`：移除 RTK 提示中跟 Python 流程相关的描述

---

## 9. 风险与权衡

| 风险 | 缓解 |
|---|---|
| Chromium SQLite schema 变化导致解密失败 | 把 `meta.version` 与 `last_access_utc` 列存在与否都做容错 |
| macOS `security` 命令弹 GUI 密码框 | 用户首次启动 Chrome 时已解锁 keychain；非交互场景失败要提示用户跑一次 Chrome |
| `mount_webdav` 在某些 macOS 版本挂载后 Finder 行为异常 | macOS 13+ 已稳定；旧版不承诺 |
| 单二进制内多 tokio task 编排 | 用 `tokio::sync::watch` 通道做 shutdown 信号 |

---

## 10. 后续可能扩展（不在本次范围）

- 多账户（不同 cookie 切不同挂载点）。
- 系统偏好设置面板（用 `objc` 调 `NSWindow`）。
- LaunchAgent 模式（开机自启 + 后台守护，无菜单栏）。