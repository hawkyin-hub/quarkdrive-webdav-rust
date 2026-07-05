# LocalQuark-rust 安装说明

> 本 app 使用 ad-hoc 签名（非 Apple 开发者证书），macOS Gatekeeper
> 会阻止首次运行。请按以下步骤操作，**只需做一次**。

---

## 第一步：解除 Gatekeeper 隔离

将 `LocalQuark-rust.app` 移动到 `/Applications` 后，打开「终端」执行：

```bash
xattr -cr /Applications/LocalQuark-rust.app
```

> 如果你把 app 放在其他位置，把路径换成实际路径即可。

---

## 第二步：安装特权 Helper（需要管理员密码）

打开「终端」执行：

```bash
/Applications/LocalQuark-rust.app/Contents/Resources/bin/install-helper.sh
```

输入 macOS 登录密码后，Helper 会安装到系统，只需安装一次。

---

## 第三步：首次使用 — 设置夸克 Cookie

1. 在 Chrome / Brave / Arc / Edge 中登录 [夸克网盘](https://pan.quark.cn)
2. 双击打开 `LocalQuark-rust.app`
3. app 会自动从浏览器读取 Cookie 并挂载网盘到 `/Volumes/LocalQuark`

> **注意**：Cookie 有效期约 30 天，到期后需要重新登录浏览器，
> 然后重启 app 自动刷新。

---

## 日志位置

| 文件 | 用途 |
|------|------|
| `~/Library/Logs/LocalQuark-rust-launcher.log` | 启动日志 |
| `~/Library/Logs/LocalQuark-rust-webdav.log`   | WebDAV 服务日志 |

---

## 卸载

```bash
/Applications/LocalQuark-rust.app/Contents/Resources/bin/uninstall-helper.sh
rm -rf /Applications/LocalQuark-rust.app
```

---

## 系统要求

- macOS 12.0 (Monterey) 或更高版本
- Chrome / Brave / Arc / Edge（需登录夸克网盘）
