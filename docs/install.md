# 安装指南

> 仓库路径：`/Users/HawkSept/myproject/myapp/localquark-rust/`
> 最后更新：2026-07-05

本指南覆盖两种使用方式：

- **路径 A：直接运行二进制** — 开发期/CI 用
- **路径 B：使用 bundled `.app`** — 普通用户用（推荐）

---

## 前置要求

| 依赖 | 版本 | 说明 |
|------|------|------|
| macOS | 13+（推荐 14+） | 仅 macOS；Linux/Win 不在支持范围 |
| Rust toolchain | stable ≥ 1.80 | `rustup install stable` |
| Xcode Command Line Tools | 最新 | `xcode-select --install`（提供 `/usr/bin/openssl` 用于自签证书） |
| `mount_webdav` | 系统自带 | macOS 12+ 已内置；启动器会调用 |

> ⚠️ 需要从 Chromium 系浏览器（Chrome / Brave / Edge / Arc）抓取 Cookie 时必须先登录 [pan.quark.cn](https://pan.quark.cn)。

---

## 路径 A：从源码构建二进制

适合：开发调试、CI、想自定义 CLI 参数的用户。

```bash
# 1. 进入 crate 目录
cd /Users/HawkSept/myproject/myapp/localquark-rust/quarkdrive-webdav

# 2. 编译 release 版本
cargo build --release

# 3. 产物路径
./target/release/quarkdrive-webdav

# 4. 启动（默认自动抓 Cookie + 挂载到 ~/Mount/Quark）
./target/release/quarkdrive-webdav

# 5. 自定义参数
./target/release/quarkdrive-webdav \
  --quark-cookie "sl-session=xxx; __pus=yyy" \
  --mount-point ~/Mount/Quark \
  --webdav-auth-user quasar
```

---

## 路径 B：构建 macOS 单 `.app`（推荐）

适合：日常使用，要双击启动、要托盘图标、要自动挂载 + 自动打开 Finder 的用户。

```bash
# 1. 在仓库根目录执行打包脚本
cd /Users/HawkSept/myproject/myapp/localquark-rust
./scripts/build-app.sh

# 2. 产物路径
./dist/LocalQuark-rust.app

# 3. 双击 .app 启动
open ./dist/LocalQuark-rust.app
# 或 Finder 中双击

# 4. 行为
#    - 在菜单栏出现 QuarkDrive 图标
#    - 自动挂载到 /Volumes/LocalQuark
#    - 自动打开 Finder 指向挂载点
```

`build-app.sh` 内部会：

1. 调用 `cargo build --release` 编译 `quarkdrive-webdav` 二进制
2. 复制到 `dist/LocalQuark-rust.app/Contents/MacOS/`
3. 生成 `Info.plist` + 启动器 shell 脚本
4. （首次启动时）`create_dir_all /Volumes/LocalQuark` + 调 `mount_webdav -S`

详细字段、launcher 行为、自定义打包见 [docs/DEPLOYMENT.md](DEPLOYMENT.md)。

---

## 首次使用步骤

1. **登录夸克网盘**：在 Chrome / Brave / Edge / Arc 任一浏览器打开 [pan.quark.cn](https://pan.quark.cn) 并完成登录。
2. **启动应用**：
   - 路径 A：`./target/release/quarkdrive-webdav`
   - 路径 B：双击 `dist/LocalQuark-rust.app`
3. **检查挂载点**：
   - 路径 A：`ls -la ~/Mount/Quark`
   - 路径 B：`ls -la /Volumes/LocalQuark`
4. **Finder 浏览**：应能看到夸克网盘根目录的文件树。

---

## 升级

```bash
cd /Users/HawkSept/myproject/myapp/localquark-rust
git pull                       # 拉取最新代码
./scripts/build-app.sh         # 重新打包 .app
# 或
cd quarkdrive-webdav && cargo build --release
```

升级前建议先：

```bash
pkill -f quarkdrive-webdav    # 停掉旧进程
umount ~/Mount/Quark 2>/dev/null || true
umount /Volumes/LocalQuark 2>/dev/null || true
```

---

## 卸载

```bash
# 1. 卸载挂载点
umount ~/Mount/Quark 2>/dev/null || diskutil umount force ~/Mount/Quark
umount /Volumes/LocalQuark 2>/dev/null || diskutil umount force /Volumes/LocalQuark

# 2. 删除应用
rm -rf dist/LocalQuark-rust.app

# 3. 删除运行时数据（可选）
rm -rf ~/Library/Application\ Support/QuarkDrive

# 4. 清理构建产物（可选）
cd /Users/HawkSept/myproject/myapp/localquark-rust
cargo clean
```

---

## 常见问题

详见 [docs/troubleshooting.md](troubleshooting.md)。
