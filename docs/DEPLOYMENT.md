# 单 `.app` 打包与部署

> 本文档描述如何使用 `scripts/build-app.sh` 把 Rust daemon + 启动器 + 证书生成器打包成单一 macOS `.app`，并部署到本机。

最后更新：2026-07-05

---

## 1. 产出物

脚本运行成功后产出：

```
dist/LocalQuark-rust.app/
├── Contents/
│   ├── Info.plist
│   ├── PkgInfo
│   ├── MacOS/
│   │   └── LocalQuark            # launcher (bash)
│   └── Resources/
│       ├── bin/                  # 所有运行时脚本 + 二进制
│       │   ├── quarkdrive-webdav
│       │   ├── run-localquark.sh
│       │   ├── teardown-localquark.sh
│       │   ├── status-localquark.sh
│       │   ├── setup-tls.sh
│       │   ├── install-helper.sh
│       │   ├── uninstall-helper.sh
│       │   ├── helper-client.sh
│       │   └── lib-common.sh
│       ├── helper/
│       │   ├── com.localquark.webdav-helper
│       │   └── Resources/
│       │       └── com.localquark.webdav-helper.plist
│       └── certs/                # 自签证书运行时生成目录
```

`.app` 本身 `LSUIElement=true`，不弹窗口，菜单栏有托盘图标。

## 2. 打包步骤

```bash
# 在仓库根目录
cd /Users/HawkSept/myproject/myapp/localquark-rust

# 1) 编译 release 二进制（脚本会复用，不重复编译）
cd quarkdrive-webdav
cargo build --release
cd ..

# 2) 打包成 .app
./scripts/build-app.sh

# 3) 启动
open dist/LocalQuark-rust.app
```

`build-app.sh` 内部流程：

1. 检查所有依赖路径（Rust 二进制、Helper 二进制、Helper plist、脚本源）
2. 清理并重建 `dist/LocalQuark-rust.app`
3. 写 `Info.plist`（bundle id `com.hawkyin.localquark-rust`，最小 macOS 12）
4. 写 launcher 到 `Contents/MacOS/LocalQuark`
5. 拷贝所有脚本 + 二进制 + Helper 到 `Contents/Resources/`
6. `chmod 0755` 所有可执行
7. 用自签名证书 `codesign --force --deep --sign -` ad-hoc 签名（可选；用户首次启动可能仍被 Gatekeeper 警告）

## 3. 启动器（launcher）行为

`Contents/MacOS/LocalQuark`（bash）做的事：

1. 读 `LOCALQUARK_HOST/PORT/AUTH_USER/AUTH_PASSWORD/MOUNT_POINT/DETACH/LOG_FILE` 环境变量
2. 尝试从 `~/Library/Application Support/LocalQuark/cookies.json` 读 cookie（如果有）
3. 如果 8443 已经在 LISTEN，直接 `open Finder $LOCALQUARK_MOUNT_POINT` 退出
4. 否则 `nohup run-localquark.sh >> ~/Library/Logs/LocalQuark-rust-launcher.log 2>&1 &` 派生
5. 轮询端口最多 6 秒
6. `open Finder $LOCALQUARK_MOUNT_POINT`（或 fallback 到 `/Volumes`）
7. 启动失败时弹 `osascript` 警告

## 4. run-localquark.sh 行为

> 注：脚本名沿用历史命名，实际只跑 Rust daemon；Python 部分在 Rust 化后已删除。

1. 解析环境变量与 cookie
2. `setup-tls.sh` 生成自签证书到 `Contents/Resources/certs/`
3. 起 `quarkdrive-webdav` daemon（自动起 backend HTTP + proxy HTTPS + mount）
4. 守护循环：每 5s 检测 daemon 是否在；不在就重启（带健康检查）
5. 接收 SIGTERM/SIGINT 走 graceful shutdown

## 5. 部署到系统

### 5.1 复制到 `/Applications`

```bash
cp -R dist/LocalQuark-rust.app /Applications/
open /Applications/LocalQuark-rust.app
```

首次启动会被 Gatekeeper 拦截：

- 系统设置 → 隐私与安全性 → 仍要打开
- 或者 `xattr -dr com.apple.quarantine dist/LocalQuark-rust.app`

### 5.2 ad-hoc 签名（已包含）

`build-app.sh` 末尾会跑：

```bash
codesign --force --deep --sign - dist/LocalQuark-rust.app
```

自签名能解决大部分启动拦截；分发到他人需要 Developer ID 签名。

### 5.3 开机自启（可选）

```bash
# 安装 launchd 用户级 agent
./dist/LocalQuark-rust.app/Contents/Resources/bin/install-helper.sh

# 卸载
./dist/LocalQuark-rust.app/Contents/Resources/bin/uninstall-helper.sh
```

> 注意：helper 不是 daemon 本身，是一个 SMJobBless 风格的助手，用于 root 权限操作（如 mount_webdav 失败时尝试 diskutil umount）。

## 6. 卸载

```bash
# 停 daemon
./dist/LocalQuark-rust.app/Contents/Resources/bin/teardown-localquark.sh

# 卸载 helper
./dist/LocalQuark-rust.app/Contents/Resources/bin/uninstall-helper.sh

# 删 app
rm -rf /Applications/LocalQuark-rust.app

# 清状态
rm -rf ~/Library/Application\ Support/QuarkDrive
rm -rf ~/Library/Application\ Support/LocalQuark
rm -rf ~/Library/Logs/LocalQuark-rust-*.log
```

## 7. 自定义

### 换挂载点名字

```bash
LOCALQUARK_MOUNT_POINT=/Volumes/Quark ./scripts/build-app.sh
```

或在 launcher 里覆盖：

```bash
LOCALQUARK_MOUNT_POINT=/Volumes/Quark open dist/LocalQuark-rust.app
```

### 换默认凭据

修改 `scripts/build-app.sh` 里 launcher 模板的：

```bash
export LOCALQUARK_AUTH_USER="${LOCALQUARK_AUTH_USER:-ujRx4Js1D}"
export LOCALQUARK_AUTH_PASSWORD="${LOCALQUARK_AUTH_PASSWORD:-DVzv3ELQ3icn}"
```

建议首次打包前改成强密码。

### 跨机分发

要把 `.app` 发给别人，需要：

1. Developer ID 签名（`codesign -s "Developer ID Application: <Your Name>"`）
2. 公证（`xcrun notarytool submit` + staple）
3. 不签名也可以让对方 `xattr -dr com.apple.quarantine` 后启动

## 8. 排错

| 现象 | 排查 |
|------|------|
| `.app` 双击没反应 | `tail ~/Library/Logs/LocalQuark-rust-launcher.log` |
| daemon 启动失败 | `tail ~/Library/Logs/LocalQuark-rust-webdav.log` |
| Finder 打开是空目录 | 见 [troubleshooting.md](troubleshooting.md) 「Finder 空目录」一节 |
| 8443 端口占用 | `lsof -i :8443` 杀掉占用进程，或改 `LOCALQUARK_PORT` |
| 自签证书被拒 | `xattr -dr com.apple.quarantine dist/LocalQuark-rust.app` |
