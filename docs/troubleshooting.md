# 故障排除指南

> 仓库路径：`/Users/HawkSept/myproject/myapp/localquark-rust/`
> 最后更新：2026-07-05
>
> 本文档提供 QuarkDrive-WebDAV 常见问题的诊断和解决方案。
>
> **目录约定速览（先看这里）**：
>
> | 路径 | 写谁读 | 内容 |
> |------|--------|------|
> | `~/Library/Application Support/QuarkDrive/` | Rust daemon (`main.rs`/`mount.rs`) | 自签证书 `cert.pem` / `key.pem`、WebDAV 凭据 `webdav.passwd` |
> | `~/Library/Application Support/LocalQuark/` | Python 版 launcher / 浏览器抓取器 | `cookies.json`、`certs/`（helper 信任证书副本）、历史日志 `localquark.log` / `proxy_subprocess.log` |
> | `~/Library/Logs/LocalQuark-rust-launcher.log` | `.app` 启动器 | 启动 / 挂载流程日志 |
> | `~/Library/Logs/LocalQuark-rust-webdav.log` | Rust daemon | 真实的 daemon 日志（tracing_subscriber 输出） |
>
> 上表里 QuarkDrive 是 Rust 重写后的权威目录；LocalQuark 是早期 Python 版的残留，写新文档时不要照抄。

---

## HTTPS 终结代理失效

### 症状
- 日志出现 `TLS handshake error`
- `lsof -i :8443` 无进程
- 挂载后 Finder 显示"无法连接"

### 检查清单
```bash
# 1. 8443 端口是否被 quarkdrive-webdav 监听
lsof -i :8443

# 2. 8443 自签证书是否存在
ls -la ~/Library/Application\ Support/QuarkDrive/cert.pem
ls -la ~/Library/Application\ Support/QuarkDrive/key.pem

# 3. 直接 curl 测试 8443
curl -vk -X OPTIONS -u quasar:<passwd-in-webdav.passwd> https://127.0.0.1:8443/ 2>&1 | grep -E "(HTTP|subject)"

# 4. 8080 backend 是否存活
curl -X OPTIONS -u quasar:<passwd-in-webdav.passwd> http://127.0.0.1:8080/
```

### 修复

```bash
# 删除旧证书让启动时自动重新生成
rm -f ~/Library/Application\ Support/QuarkDrive/cert.pem
rm -f ~/Library/Application\ Support/QuarkDrive/key.pem
pkill -f quarkdrive-webdav
# 重新启动 .app 或二进制
```

---

## 挂载相关问题

### 问题：Finder 中挂载点为空

#### 检查清单
1. ✅ 应用是否在运行？检查菜单栏图标或 `ps aux | grep quarkdrive-webdav`
2. ✅ 挂载点是否存在？`ls -la ~/Mount/Quark`
3. ✅ WebDAV 服务是否可达？`curl -X OPTIONS -u quasar:<passwd-in-webdav.passwd> http://127.0.0.1:8080/`
4. ✅ Cookie 是否有效？查看日志或尝试手动刷新（菜单栏 → 立即刷新 Cookie）

#### 解决方案
```bash
# 强制刷新挂载点
touch ~/Mount/Quark/.refresh

# 重启应用（保留挂载点）
killall quarkdrive-webdav
./target/release/quarkdrive-webdav

# 手动卸载后重新挂载
umount ~/Mount/Quark
./target/release/quarkdrive-webdav  # 会自动重新挂载
```

### 问题：Finder 提示“无法连接到服务器”

#### 原因分析
- WebDAV 服务未启动
- 端口被占用或防火墙阻止
- TLS 证书不受信任（HTTPS 模式）
- 认证失败

#### 检查步骤
```bash
# 检查进程是否运行
pgrep -l quarkdrive-webdav

# 检查端口监听
lsof -i :8080
lsof -i :8443

# 测试 HTTP 可达性
curl -v -X OPTIONS -u quasar:<passwd-in-webdav.passwd> http://127.0.0.1:8080/ 2>&1 | grep -E "(HTTP|WWW-Authenticate)"

# 测试 HTTPS 可达性（忽略证书错误）
curl -vk -X OPTIONS -u quasar:<passwd-in-webdav.passwd> https://127.0.0.1:8443/ 2>&1 | grep -E "(HTTP|WWW-Authenticate)"
```

#### 解决方案
- 更换端口：`quarkdrive-webdav -p 8081`
- 检查防火墙：`sudo /usr/libexec/ApplicationFirewall/socketfilterfw --listapp | grep quarkdrive`
- 对于 HTTPS：在钥匙串访问中信任自签名证书或使用 `--no-tls`（仅 HTTP）

---

## 性能问题

### 问题：上传/下载速度慢

#### 可能原因
1. **网络限制**：夸克网盘本地带宽限制
2. **串行上传**：`upload_chunk` 是串行；早期尝试过 `buffer_unordered(4)` 并发但已被回滚（Quark API `part_thread:1` 限制），不是 bug。
3. **频繁 IO**：旧版本有 `flush()` 延迟（已修复）

#### 检查步骤
```bash
# 查看日志中是否有并发上传提示
grep -i "parallel" ~/Library/Logs/LocalQuark-rust-webdav.log

# 测试本地磁盘速度（排除网络因素）
dd if=/dev/zero of=/tmp/testfile bs=1M count=1024 oflag=dsync
```

#### 解决方案
- 确认使用最新版本（Phase 2 删 sleep、Phase 3 删 flush 真实生效；Phase 1 是历史尝试，已回滚）
- 检查网络连接（Wi-Fi 信号强度、有线 vs 无线）
- 避免在高峰时段传输大文件

---

## Cookie 问题

### 问题：Cookie 抓取失败

#### 日志特征
```
[ERROR cookie::store] Failed to decrypt cookie: Invalid padding
[ERROR cookie::store] No valid session cookie found
```

#### 原因
- 浏览器未登录夸克网盘
- 浏览器使用主密码加密 Cookie（Chrome 设置 → 自动填充 → 密码）
- Keychain 访问被拒绝

#### 解决方案
1. 确保浏览器已登录 `pan.quark.cn` 或 `drive.quark.cn`
2. 禁用浏览器主密码：
   - Chrome：`chrome://settings/passwords` → 关闭“提供保存密码”
3. 重置 Keychain 访问权限：
   - 打开钥匙串访问 → 搜索 “Chrome Safe Storage” → 右键 → 获取信息 → 访问控制 → 允许所有应用程序访问

---

## 日志与调试

### 查看日志

两份日志各看各的：

| 用途 | 路径 |
|------|------|
| `.app` 启动器（挂载流程） | `~/Library/Logs/LocalQuark-rust-launcher.log` |
| **Rust daemon（真相）** | `~/Library/Logs/LocalQuark-rust-webdav.log` |
| 历史 Python 版 daemon（旧） | `~/Library/Application Support/LocalQuark/localquark.log` |

实时查看：
```bash
tail -f ~/Library/Logs/LocalQuark-rust-webdav.log      # daemon
tail -f ~/Library/Logs/LocalQuark-rust-launcher.log    # .app launcher
```

### 调试模式

启用详细日志：
```bash
RUST_LOG=debug ./target/release/quarkdrive-webdav
RUST_LOG=trace ./target/release/quarkdrive-webdav  # 最详细
```

### 常用调试命令

```bash
# 检查挂载状态
mount | grep Quark

# 查看进程树
pstree -p $(pgrep quarkdrive-webdav)

# 检查打开的文件描述符
lsof -p $(pgrep -f quarkdrive-webdav)

# 测试 WebDAV 响应
curl -X PROPFIND -H "Depth: 1" -u quasar:<passwd-in-webdav.passwd> http://127.0.0.1:8080/ 2>/dev/null | xmllint --format -
```

---

## 已知限制

| 限制 | 说明 | 变通方案 |
|------|------|----------|
| 大文件上传卡住 | macOS `mount_webdav` 对大文件 PUT 有超时限制 | 使用命令行工具如 `rclone` 或 CyberDuck 传输大文件 |
| 同时只能挂载一个实例 | `mount_webdav` 不支持同一挂载点多次挂载 | 使用不同挂载点：`~/Mount/Quark1`, `~/Mount/Quark2` |
| 后台运行时偶尔被系统挂起 | 长时间无活动后 App Nap | 在“系统设置 → 电池”中禁用 App Nap 或保持轻量活动 |
| 某些特殊字符文件名 | WebDAV 协议对编码有限制 | 避免使用控制字符和非 Unicode 字符 |

---

## 紧急恢复

-### 应用完全无响应
```bash
# 1. 杀死所有相关进程
pkill -f quarkdrive-webdav
pkill -f LocalQuark-rust   # .app launcher

# 2. 卸载挂载点（如果被占用）
umount -f ~/Mount/Quark 2>/dev/null || diskutil umount force ~/Mount/Quark
umount -f /Volumes/LocalQuark 2>/dev/null || diskutil umount force /Volumes/LocalQuark

# 3. 清理临时文件（可选）
rm -rf ~/Library/Application\ Support/LocalQuark/quarkdrive-webdav/*.tmp

# 4. 重新启动
open dist/LocalQuark-rust.app
# 或
./target/release/quarkdrive-webdav
```

### helper-client 卡在 `mkdir /Volumes/LocalQuark`

**症状**：launcher 日志一直循环 `helper detected; mounting via helper-client`，
`pgrep -fl webdav-helper` 显示 `client mkdir /Volumes/LocalQuark` 不退出。
Finder 看不到 `/Volumes/LocalQuark`（目录存在但没有真正挂载上）。

**原因**：
1. helper 把证书加进 Keychain 这一步超时被 SIGKILL（`trust-cert failed (rc=137)`）。
2. helper-client 阻塞在 `mkdir` 上，没有超时退出。

**修复**：
```bash
# 1. 先看 launcher 日志确认是 trust-cert 失败
tail -50 ~/Library/Logs/LocalQuark-rust-launcher.log
grep -E "trust-cert|helper-client" ~/Library/Logs/LocalQuark-rust-launcher.log

# 2. 杀掉卡住的 helper-client
sudo pkill -9 -f webdav-helper

# 3. 删除 helper 信任过的旧证书再重试（避免同名冲突）
sudo security delete-certificate -c "LocalQuark WebDAV" \
    /Library/Keychains/System.keychain 2>/dev/null || true

# 4. 重新打开 .app
open dist/LocalQuark-rust.app
```

**预防**：
- `.app` 第一次启动时在「隐私与安全性」点击确认一次 helper 安装请求。
- 不要在已经挂载 `/Volumes/LocalQuark` 的情况下双击 `.app`，先卸载。
---

## 凭据落盘路径

daemon 启动时会自动生成 WebDAV 凭据（Basic Auth）并落盘到：

```
~/Library/Application Support/QuarkDrive/webdav.passwd     # 用户名/密码
~/Library/Application Support/QuarkDrive/cert.pem          # 自签 TLS 证书
~/Library/Application Support/QuarkDrive/key.pem           # 自签 TLS 私钥
```

权限均为 `0o600`。如果 `/Volumes/LocalQuark` 挂载后认证失败：

```bash
# 查看实际凭据
cat ~/Library/Application\ Support/QuarkDrive/webdav.passwd

# 用实际凭据 curl 验证
USER=$(awk -F= '{print $1}' ~/Library/Application\ Support/QuarkDrive/webdav.passwd)
PASS=$(awk -F= '{print $2}' ~/Library/Application\ Support/QuarkDrive/webdav.passwd)
curl -X OPTIONS -u "$USER:$PASS" https://127.0.0.1:8443/
```

> 修改 `--webdav-auth-user` / `--webdav-auth-password` 后建议删掉 `webdav.passwd` 让其重新生成。

---

## 双端口矩阵

| 端口 | 协议 | 角色 | 暴露范围 |
|------|------|------|----------|
| `8080` | HTTP | WebDAV backend（dav-server 直连） | 仅 `127.0.0.1`，不要对外 |
| `8443` | HTTPS | 终结代理（自签证书） | 仅 `127.0.0.1`，mount_webdav 目标 |

> macOS 26.6+ 的 `webdavfs_agent` 拒绝 HTTP + Basic Auth（`Authentication method (Basic) too weak`），所以 mount_webdav 必须走 8443 HTTPS。8080 是 backend 内部端口，不要直接挂。
---

## 获取帮助

在提交 Issue 前，请提供：

1. **系统信息**：`sw_vers` 和 `uname -a`
2. **日志文件**：附 `~/Library/Logs/LocalQuark-rust-webdav.log`（daemon）和 `~/Library/Logs/LocalQuark-rust-launcher.log`（启动器）最近 50 行；若是直接跑二进制，附 stdout/stderr。
3. **复现步骤**：详细描述触发问题的确切操作
4. **期望行为 vs 实际行为**

---

*最后更新：2026-07-05*
