# Bug 修复经验

> 本文件记录本项目历次 bug 修复的真实经过。**仅当用户已确认修复成功**后才追加。
> 每条记录保持"现象 → 根因 → 改动 → 验证"四段式，方便下次遇到类似症状快速定位。

| 编号 | 日期 | 一句话症状 | 一句话根因 | 验证方式 |
|------|------|-----------|-----------|---------|
| 1 | 2026-07-05 | 大文件下载不匀速，忽快忽慢 | 精确范围缓存导致高频小请求，无对齐 & 并发重复下载 | dd 测试 + 拖放视频文件验证，4MB chunk 缓存成功落盘 |
| 7 | 2026-07-07 | 本地构建/GitHub发布报错找不到Helper | 打包脚本路径强耦合被 git 忽略的 legacy 构建缓存目录 | 干净环境 swift build 成功，build_deploy_test.sh 及 restart_app.sh 跑通挂载 |

## 详细记录

## 2026-07-05: 大文件下载速度波动与极慢问题优化

### 故障表现
- **用户角度**：从挂载盘拖放/下载大文件时，速度不匀速，忽快忽慢，进度条跳跃非常大，总体下载速度极其缓慢。
- **日志/数据**：在 ``~/Library/Logs/LocalQuark-rust-webdav.log`` 中伴随高频微小的 Range GET 请求，同时在夸克网盘连接抖动时出现大量重试。

### 根因分析
- **高频小请求**：旧版 Rust 实现中的缓存是“精确范围缓存”（Exact Range Cache）。当 macOS Finder 以 128KB 甚至更小的 count 块并发读取大文件时，因为每次请求的 (offset, len) 对不上，导致 100% 缓存未命中。代理被迫对每一个微小区间单独向网络发起 Range 下载，造成高昂的 HTTP 连接/握手 RTT 开销。
- **并发下载冗余**：多并发读取同个大分块时缺少互斥机制，导致网络上同时在重复拉取相同的分块。

### 改动
- **对齐 4MB 视窗缓存**：在 [vfs.rs](./quarkdrive-webdav/src/vfs.rs) 中引入了 `const CHUNK_SIZE: u64 = 4 * 1024 * 1024;`（即 4MB）分块对齐。每次未命中时一次性将整块对齐 4MB 数据拉下存盘，后续在此区间的微小读取全部从本地 4MB 缓存中切片返回。
- **双检排队锁**：在结构体中添加 `chunk_locks` 排队锁，拉取前获取该锁，并在加锁前后进行“双检（Double-Checked）”本地磁盘已缓存状态，消除并发重复下载。

### 验证
- **落盘验证**：检查 `~/Library/Caches/LocalQuark/chunks/236efa62e2bbb2ab/` 下，成功写入多份 19:10 的 `4.0M` bin 缓存分块（如 `4194304-8388608.bin`）。
- **性能表现**：用户拷贝大文件，除第一次拉取对齐分块稍有热身卡顿外，随后完全正常。第二次拖放完全在本地命中，极其丝滑匀速。

### 经验
- WebDAV 网盘大文件读取极忌讳“微小精确范围拉取”，必须将 Range 映射到物理对齐窗口（如 4MB），并通过双检排队锁去重，方可彻底吃满 CDN 带宽，防止网络开销阻塞客户端。

## 2026-07-05: 拖放延迟 + 上传覆盖 + 孤儿文件清理

### 故障表现
- **拖放延迟**：从挂载盘往 Finder 拖文件时，从拖动到开始复制要等好几秒。
- **上传覆盖失败**：curl PUT 同名文件第一次成功，第二次（覆盖）500 失败，第三次成功。Finder 拖放覆盖时也会"卡住不动"。
- **孤儿文件残留**：日志中 `commit upload failed error=error sending request`，但夸克云端留下半成品文件。

### 根因分析
- **`is_url_expired` 过期边界过宽（300s）**：夸克 CDN URL 典型效期 30min。PROPFIND 拿到的 URL 经常已接近过期窗口，read_bytes 走"已过期 → 调 `get_download_url`"分支，触发 8s 刷新超时 → 用户感知的"拖放卡顿"。
- **`get_download_url` 超时 8s 过长**：拖放时 Finder 等不了 8s 就放弃了。
- **覆盖上传 `remove_file` 失败被 swallow**：[vfs.rs:968](./quarkdrive-webdav/src/vfs.rs) 和 [vfs.rs:1229](./quarkdrive-webdav/src/vfs.rs) 的 `if let Err(err) = ... remove_file() ... { error!(...) }` 只 log 不返回。即使删除失败也继续 `up_pre` 创建新文件，结果是两个同名文件共存（一个旧 fid + 一个新 fid）。
- **commit/finish 失败无清理**：上传 commit / finish 阶段失败时，chunks 已经全部上传到 OSS，但夸克云端留下"未提交"的孤儿文件。下次 Finder 拖放访问时会反复失败/重试，表现为 Finder 卡住不动。
- **9 处错误消息全是 `delete file failed`**：[drive/mod.rs](./quarkdrive-webdav/src/drive/mod.rs) 的 `rename_file / move_file / create_folder / get_quota / up_pre / up_hash / auth / finish` 函数都用了同一句 `"delete file failed: {}"`，无法定位具体哪个 API 出错。

### 改动
- **`is_url_expired` 安全边距 300s → 60s**：[vfs.rs:1852](./quarkdrive-webdav/src/vfs.rs) `return current_ts + 60 >= expires;` 同步更新单元测试 `test_is_url_expired_within_60s_buffer` / `test_is_url_expired_beyond_60s_buffer` / `test_is_url_expired_exactly_at_boundary`。
- **`get_download_url` 超时 8s → 3s**：[vfs.rs:1661](./quarkdrive-webdav/src/vfs.rs) `Duration::from_secs(3)`，同步更新日志文本。
- **覆盖上传 `remove_file` 失败不再 swallow**：[vfs.rs:968](./quarkdrive-webdav/src/vfs.rs) 改用 `?` 直接返回 `FsError::GeneralFailure`，[vfs.rs:1229](./quarkdrive-webdav/src/vfs.rs) `upload_mini_byte_file` 同样修复。
- **commit/finish 失败清理孤儿**：[vfs.rs:1144-1175](./quarkdrive-webdav/src/vfs.rs) 失败时调 `drive_inner.remove_file(&new_fid, true)` 删除刚 up_pre 创建的孤儿文件。需要先在 spawn 前把 `self.file.fid` clone 进闭包（变量名 `new_fid`）。
- **9 处错误消息改成对应函数语义**：[drive/mod.rs:524/550/598/613/648/674/760/852](./quarkdrive-webdav/src/drive/mod.rs) 全部改完，分别对应 `rename_file / move_file / create_folder / get_quota / up_pre / up_hash / upload_part_auth / finish_upload`。`delete_file` 自身保持 `"delete file failed"`。

### 验证
- **拖放延迟**：`curl GET /.DS_Store` cold 2.785s（首次从云端拉），warm 8ms（chunk cache 命中），348 倍提升。chunk cache 目录有 28+ 文件，最近 22:05 创建的证明缓存机制生效。
- **覆盖完整成功**：v1 (204 5.1s) → v2 (204 5.2s) → PROPFIND 仅看到 1 个 `test_overwrite.txt` → 下载云端文件 diff v2 完全匹配。
- **孤儿清理代码就绪**：commit/finish 流程未失败，所以 cleanup 没触发（这是预期的）。日志路径已就绪，未来出现 OSS 限速时自动清理孤儿。

### 经验
- 涉及"覆盖/替换"语义的 API 链（先 delete 再 create），`remove_file` 失败必须返回错误，不能 swallow。否则会留下两个不同 fid 的"同名文件"，Finder/curl 等客户端行为完全混乱。
- 上传分块写入和"提交元数据"是分离的两步（OSS 直传 + 夸克 finish）。中间任何一步失败都要回滚已写入的数据（孤儿文件），否则会被后续访问者反复遇到。
- 错误消息必须按操作命名，不要所有 API 共用同一句 `"X failed"`，否则定位问题时无法判断是哪一步出错。

## 2026-07-05: 拖放误报"是否替换"（destination 没有同名文件）

### 故障表现
- **用户角度**：拖放一个新文件到挂载目录，目的地明明没有同名文件，Finder 仍然弹出"是否替换？"对话框。
- **触发条件**：用户在 45 秒内"上传过同名文件 → 删过该文件 → 再拖同名文件"。

### 根因分析
- 上传 commit 成功后，[vfs.rs:1183](./quarkdrive-webdav/src/vfs.rs) `register_active_write` 把文件注册到 `active_writes` 这个 DashMap 里，缓存 **45 秒**。
- 设计的目的是让 Finder 在上传完成后立刻能看到文件，而不用等缓存过期。
- `read_dir`（[vfs.rs:488-498](./quarkdrive-webdav/src/vfs.rs)）合并 active_writes 到 PROPFIND 结果里。
- **但 active_writes 永远不会被清掉** — 没有 `remove_active_write` 函数，只有 `insert` + `retain`（45 秒自动过期）。
- 用户删除文件 X（移到废纸篓）时，[vfs.rs:702](./quarkdrive-webdav/src/vfs.rs) 只调 `dir_cache.invalidate_parent`，**没清 active_writes**。
- 之后 45 秒内用户拖新文件 X 到同目录 → Finder PROPFIND → active_writes 仍含 X → 弹"是否替换"对话框（但云端实际没有 X）。

### 改动
- 加 [`remove_active_write`](./quarkdrive-webdav/src/vfs.rs) 函数（紧跟 `register_active_write` 之后），与 `remove_uploading_file` 对称。
- FS 层 `remove_file` 在 `dir_cache.invalidate_parent` 之后调 `remove_active_write(parent, file_name)`。

### 验证
- E2E 复现：PUT v1 → DELETE → PROPFIND 看 `test_replace.txt` 数量 = 0（active_writes 已清）→ PUT v2 (201，**没报覆盖**) → PROPFIND = 1。
- 修复后行为符合 macOS Finder 预期。

### 经验
- 任何"短期缓存"机制（active_writes 45s）必须配对"清空机制"，否则 DELETE / RENAME 路径就会产生幽灵状态。
- 设计时不能只考虑"正常路径"（上传 + 看），也要考虑"删除路径"（上传 → 删 → 重传）。

## 2026-07-05: admin 误报 "CookieMissing"（cookie 实际有效）

### 故障表现
- **用户角度**：浏览器打开 `http://127.0.0.1:8444/`，看到健康等级 `CookieMissing`，但实际上传/下载都正常。
- **触发条件**：用 `--quark-cookie` 显式传 cookie 字符串启动。

### 根因分析
- [`health.rs:70`](./quarkdrive-webdav/src/health.rs) 用 `cookies.get("sl-session").is_some()` 判断 cookie 健康。
- `sl-session` 是浏览器 cookie db 里 `drive.quark.cn` 域名下的 key；但 `--quark-cookie` 显式传入时通常**不包含** `sl-session`（用户贴出的 cookie 串只有 `__pus / __kp / __uid / isg / __puus` 等）。
- 所以"sl-session 不存在" ≠ "cookie 无效"，旧判定过于苛刻，导致 admin 永远显示 CookieMissing。

### 改动
- [`health.rs:70-82`](./quarkdrive-webdav/src/health.rs) 新逻辑：cookie 数量 > 0 且**至少**包含一个关键 key (`sl-session` / `__pus` / `__kp` / `__uid`)。
- 这些 key 在 `--quark-cookie` 显式串里都有（drive 真实依赖的是 `__pus / __kp / __uid`，`sl-session` 只是浏览器版的别名）。

### 验证
- `/api/status` 从 `{"cookies_ok":false,"level":"CookieMissing"}` 变成 `{"cookies_ok":true,"level":"Healthy"}`。
- `/api/cookies` 显示 16 项 key，含 `__pus / __kp / __uid`，新判定返回 true。

### 经验
- "健康判定" 必须覆盖所有合法的输入路径，不能只假设某一种 cookie 来源（这里既支持浏览器 cookie db 又支持 `--quark-cookie` 串）。
- 关键 key 的选取要看 drive 的真实依赖（看 `drive/mod.rs` 里 `cookie.resolve()` 真正读哪些 key），而不是凭直觉挑一两个。

## Bug #3: macOS Finder "in use" 无法删除 webdav 上含 AppleDouble 残留的文件夹

**日期**: 2026-07-06
**触发场景**: 在 `/Volumes/LocalQuark` 上传 `.doc` 文件后删除原文件,留下 `._*.doc` 孤儿,导致整个父文件夹无法删除

### 故障表现

- macOS Finder 拖拽 `中国商飞灾备咨询交付物` 文件夹到废纸篓,弹窗:
  > The operation can't be completed because the item "中国商飞灾备咨询交付物" is in use.
- 终端 `rm -rf` 报 `Directory not empty` 而非 `in use`
- 其他同类文件夹都能正常删除 → 不是 mount/webdavfs 全局问题
- 该文件夹 mode `drwx------` (但其他正常文件夹也是 700, mode 不是问题)

### 原因分析 (有铁证,不是猜)

1. **Quark 后端真的存了 2 个 `._*.doc` 文件** — curl PROPFIND 直接命中:
   ```
   /中国商飞灾备咨询交付物/._13.1《商飞CDDS业务系统容灾演练报告》.doc  (4096 bytes, etag 1000-655c4834e1f30)
   /中国商飞灾备咨询交付物/._4.《商飞网络信息安全建设方案》.doc       (4096 bytes, etag 1000-655c477633408)
   ```
   完整 DAV:propstat 返回 200, etag / creationdate / getlastmodified 都来自 Quark 后端, 不是 webdavfs_agent 客户端伪造。

2. **这些 `._*` 是 macOS Finder 上传时自动创建的 AppleDouble metadata** (资源 fork / 图标信息), 用户在 macOS 本地删除原 `.doc` 后, 孤儿 `._*.doc` 留在 Quark 云端。

3. **vfs.rs read_dir 把 Quark 返回的所有 files 不加过滤塞给 PROPFIND** — 包括 `._*.doc`。webdavfs_agent 看到的就是"非空目录", DELETE 子文件时云端找不到对应原文件, 错误码让 macOS 误判为"in use"。

4. **请求没到 WebDAV server?** — 日志全是 rustls SNI warning, 没有 PROPFIND/DELETE 记录。这是因为 webdavfs_agent 在 vnode 层直接拒绝 rmdir 非空目录, 不发 HTTP 请求。

### 修复

`vfs.rs` 的 `read_dir` 在合并 `merged_files` 之后, 输出 DavDirEntry 流之前, 增加一行过滤:

```rust
// 4. 过滤 macOS AppleDouble / dotfile metadata。
merged_files.retain(|name, _| !is_macos_metadata_file(name));
```

辅助函数:

```rust
fn is_macos_metadata_file(name: &str) -> bool {
    if name == ".DS_Store" || name == ".Trashes"
        || name == ".fseventsd" || name == ".Spotlight-V100"
        || name == ".TemporaryItems" || name == "Thumbs.db"
        || name == "desktop.ini" || name == ".hidden"
    {
        return true;
    }
    if name.starts_with("._") {
        return true; // AppleDouble 资源 fork
    }
    false
}
```

仅影响 WebDAV 视图, 不动 Quark 后端实体 — 后续 macOS 客户端重新上传会自动清掉孤儿。

### 改动结果 (端到端验证)

```
=== PROPFIND 返回 改前 vs 改后 ===
改前: 2 个 ._*.doc  (含完整 etag/length/lastmodified)
改后: 0 个, 目录返回为空 (只有 self href)

=== rm -rf /Volumes/LocalQuark/中国商飞灾备咨询交付物 ===
改前: rm: ... Directory not empty (exit 1)
改后: (no output, exit 0) → 文件夹彻底删除
```

### 教训

1. **不要相信 Finder 的 "in use" 字面含义** — 实际可能是 webdavfs_agent vnode 层的"非空"判断被 PROPFIND 误导。
2. **PROPFIND 响应要过滤 macOS AppleDouble / dotfile metadata**, 这些是 macOS 上传到 webdav 的副作用, 用户不需要看到。
3. **curl PROPFIND 是诊断 webdav 兼容性的最直接工具**, 5 秒就能拿到真相, 比看 vfs.rs 猜强 100 倍。
4. **文件夹 mode 不是问题** — 700 和 755 都能正常删除, 不要被 `drwx------` 误导。

### 受影响文件

- ``quarkdrive-webdav/src/vfs.rs`` (read_dir + 新增 is_macos_metadata_file)

## Bug #4: mount_supervisor 重连到错位路径, 用户提示"已挂载"但 Finder 看到空

**日期**: 2026-07-07
**触发场景**: 用户拖放 docker.ipk 上传, webdavfs_agent 短暂断开, mount_supervisor 检测到 MOUNT MISSING 自动重连, macOS 系统提示"LocalQuark 已挂载", 但 Finder 打开 LocalQuark 是空的。

### 故障表现

1. 用户拖放文件到 `/Volumes/LocalQuark`, webdavfs_agent 短暂中断
2. mount_supervisor 检测到 mount missing, 触发自动重连
3. mount_supervisor 日志输出 `RE-MOUNTED via launchctl asuser`, 系统通知提示挂载已恢复
4. **但 Finder 侧栏 LocalQuark 打开是空的 / 实际网盘挂在了别的地方**
5. `mount | grep localquark` 显示挂到了 ``~/Mount/Quark``, 不是 `/Volumes/LocalQuark`
6. 用户从 Finder 看到的 `/Volumes/LocalQuark` 已经不是我们服务的盘

### 原因分析 (诊断步骤)

1. **mount 命令定位错位挂载**:
   ```
   $ mount | grep -iE "(localquark|quark)"
   https://127.0.0.1:8443/ on `~/Mount/Quark` (webdav, ...)
   ```
   不是 `/Volumes/LocalQuark`!

2. **mount_supervisor 日志 field 显示错位**:
   ```
   mount_supervisor: start, url=... mount=`~/Mount/Quark`
   mount_supervisor: MOUNT MISSING ... mount=`~/Mount/Quark`
   mount_supervisor: RE-MOUNTED ... mount=`~/Mount/Quark`
   ```
   而 launcher 日志写 mount=`/Volumes/LocalQuark`.

3. **源码 vs 二进制不一致** — 这是真凶:
   - `main.rs` 当前写: `const MOUNT_POINT: &str = "/Volumes/LocalQuark"` (写死常量, mount_supervisor_loop 单参数)
   - `main.rs` 还写: `mount_point: String default_value = "~/Mount/Quark"` (CLI 默认值)
   - 但 launcher 用 `LOCALQUARK_MOUNT_POINT=/Volumes/LocalQuark` 写死 (局部环境变量覆盖)
   - **当前跑 binary 是某个历史版本**, 那个版本 mount_supervisor_loop 接受 mount_point: String 参数, 拿到的 mount_point 是 `~/Mount/Quark` 展开路径
   - 源码改回单参数 const 后没有重新 build, 老 binary 继续跑

4. **build_deploy_test.sh 弱点** — 第 3 步只清理 `/Volumes/{LocalQuark,127.0.0.1,Quark}`, 没有清理 ``~/Mount/Quark``, 因此老 binary 的错位 mount 在下次部署后还残留。

### 修复

两步修 (binary 已经 build 好的情况下):

```bash
# 1. 杀老进程 + 卸载错位 mount (build_deploy_test.sh 不处理这条路径)
killall -9 quarkdrive-webdav 2>/dev/null || true
diskutil unmount force "`~/Mount/Quark`" 2>&1 || true

# 2. 重 build + 部署 + 启动 (新 binary 才用新 const = /Volumes/LocalQuark)
bash `scripts/build_deploy_test.sh`
```

如果要彻底, 还需要给 `build_deploy_test.sh` 加一条 `diskutil unmount force "`~/Mount/Quark`"`, 但这次场景下手动清就够了。

### 改动结果 (端到端验证)

```
=== mount 命令 (改前 vs 改后) ===
改前: https://127.0.0.1:8443/ on `~/Mount/Quark`
改后: https://127.0.0.1:8443/ on /Volumes/LocalQuark (webdav, ...)  ← 修对了

=== binary mtime (改前 vs 改后) ===
改前: Jul 7 11:45:58 (老 binary, mount_supervisor 接受路径参数)
改后: Jul 7 12:08:55 (新 binary, mount_supervisor 写死 const /Volumes/LocalQuark)

=== PROPFIND / 内容验证 ===
response length: 19687, 26 entries, 含用户的 13 个文件夹 + 多个 .DS_Store conflict + docker.ipk
```

### 教训

1. **源码改了 binary 必须 rebuild + redeploy** — 写完代码一定要走一遍 `build_deploy_test.sh`, 不要只改源码不更新 binary。`mount_supervisor_loop` 函数签名单/双参数这种改动尤其危险。
2. **`mount | grep` 永远是最快的"挂载对了没"检测** — 不要相信 mount_supervisor 自己说 RE-MOUNTED, 它只看到 `Output.status.success()`, 看不到 macOS 内部 mount table。
3. **build_deploy_test.sh 的 unmount 列表要覆盖所有可能的错位路径** — 不只是 `/Volumes/*`, 还要包括用户任意 home 目录下的 mount 点。
4. **launcher 和 mount_supervisor 必须用同一个 mount_point** — 现在 launcher 写死 LOCALQUARK_MOUNT_POINT=/Volumes/LocalQuark, 但 main.rs CLI 默认 ~/Mount/Quark, 这种不一致是定时炸弹。后续应该把 opt.mount_point 也改成默认 `/Volumes/LocalQuark`, 或者 mount_supervisor 写死一个常量 (我已经做了一半)。
5. **macOS mount_webdav 的 launchctl asuser spawn 是同步的** — 返回 status 0 不代表 mount 真的成功了。可以用 `mount | grep target` 再 grep 一次确认。

### 受影响文件

- ``scripts/build_deploy_test.sh`` (待补: 加 unmount ``~/Mount/Quark``)

## Bug #5: deploy 重启顺序错 → webdavfs_agent 热切换 → drag-drop 出现 (1) 文件

**日期**: 2026-07-07
**触发场景**: 用户执行 build_deploy_test.sh 重启 server 后, 拖文件到挂载网盘, Finder 自动生成 (1) 文件, 同时留下 0 byte 占位。

### 故障表现

1. 用户跑 `build_deploy_test.sh` (开发者改完代码 / 重新打包后常规操作)
2. 进程顺序: `killall -9 quarkdrive-webdav` → 等 2s → unmount → install → start
3. webdavfs_agent 客户端**没先收到优雅的 unmount** → server socket 突然关闭
4. macOS webdavfs_agent 自动 retry TLS handshake (几十次, 全是 SNI noise warning)
5. 期间 webdavfs_agent 在 vnode cache 里留下**0 byte 占位**
6. 用户接下来 drag-drop 一个本地文件 (云端无同名文件) → Finder 在 PROPFIND 看到占位"已存在同名文件" → 跳过原名 → 自动加 `(1)` 重试
7. 结果: Finder 显示 2 个文件 (1 个 0 byte 原名 + 1 个新文件 `(1)`), 即使 cloud 上根本没有这个文件

### 原因分析 (有铁证)

日志铁证 (会话日志):
```
04:08:57.258 WARN proxy: TLS handshake failed peer=127.0.0.1:57667 error=tls handshake eof
04:08:57.738 WARN proxy: TLS handshake failed peer=127.0.0.1:57670 error=tls handshake eof
04:08:58 - 04:09:01 100+ 行 rustls::msgs::handshake: Illegal SNI extension ...
```

说明事件顺序:
1. 04:08:?? 老 server 被 kill (PID 43288) → webdavfs_agent TCP 连接 EOF
2. 04:08:57 webdavfs_agent 立即重连新 server (PID 80383) → TLS handshake 失败
3. 04:08:58+ 100+ 次 SNI warning = webdavfs_agent 一次次重试 TLS, 终于成功
4. 拖文件时 vnode cache 半坏, webdavfs_agent 误以为"目标已存在"

**关键**: SNI warning 不是 TLS 错误, 是 rustls 提醒"ServerName 是 IP 不是 hostname", 但握手正常完成。前 2 行 `TLS handshake failed eof` 才是真错误。

### 修复

`scripts/build_deploy_test.sh` step 3 改顺序: **先 unmount 再 kill server**, 让 webdavfs_agent 优雅卸载 (vnode cache 完整清理)。

```bash
# 改前: killall → 等 2s → unmount (webdavfs_agent 半死, vnode cache 残留)
killall -9 quarkdrive-webdav run-localquark.sh
sleep 2
diskutil unmount force /Volumes/LocalQuark  # 太迟了

# 改后: unmount → 等 webdavfs 清理 vnode → killall (客户端完全卸载, vnode cache 全清)
diskutil unmount force /Volumes/LocalQuark                  # 优雅通知 webdavfs_agent 退出
diskutil unmount force "`~/Mount/Quark`"        # Bug #4 残留路径也一起清
diskutil unmount force /Volumes/127.0.0.1
diskutil unmount force /Volumes/Quark
sleep 3   # 让 webdavfs_agent 完整 release 它的 vnode cache
killall -9 webdavfs_agent                                    # 兜底杀残留
killall -9 quarkdrive-webdav run-localquark.sh
sleep 1
```

### 改动结果 (端到端验证)

修复后跑 `build_deploy_test.sh`:

```
[3/6] Unmount gracefully, then kill server
Unmount successful for /Volumes/LocalQuark  ← 干净
[5/6] Start app
[6/6] Wait for mount
  ✓ mounted after 7s

日志行数: 27 行 (之前是 1025 行) ← server 从干净空状态启动
TLS handshake failed 次数: 0 (之前 2 次 + 100+ SNI noise) ← 修复生效
mount: /Volumes/LocalQuark
PROPFIND / : 17 entries, 正常
```

### 教训

1. **重启顺序: 先 unmount, 后 kill server** —— webdavfs_agent 是 kernel extension, 它不能"快速重启", 它的 vnode cache 必须通过 unmount 优雅清理。
2. **`TLS handshake eof` 容易被 100+ SNI noise 掩盖** —— 排查时第一件事就是过滤掉 `rustls::msgs::handshake: Illegal SNI` 再 grep `TLS handshake failed`, 否则找不到真错误。
3. **macOS webdavfs_agent 在 drag-drop 时如果看到"目标文件已存在", 不会问覆盖, 直接 keep both 加 `(N)`** —— 这是 macOS Finder 的默认行为 (Settings → Advanced → When copying duplicate files → Replace), 不是 webdavfs 的 bug。
4. **deploy 脚本里 killall 加 1-2s sleep 是不够的** —— 让 webdavfs_agent 清理 vnode cache 需要更长时间 (3s+)。
5. **一定要把 deploy 路径上的所有可能的"错位挂载点"都加到 unmount 列表** —— 之前漏了 ``~/Mount/Quark``, 导致 Bug #4 残留。这次补上。

### 受影响文件

- ``scripts/build_deploy_test.sh`` (step 3 顺序调整)

## Bug #6: do_flush(P1-2) 把 webdavfs_agent 的 0-byte probe 当真上传, 留下 orphan + 触发 Finder "(1)" 自动重命名

**日期**: 2026-07-07
**触发场景**: 用户拖一个本地文件到 /Volumes/LocalQuark, Finder 显示两个文件: 原文件名 (0 bytes) + 原文件名(1) (1.4MB 真文件)。

### 故障表现

- 用户拖一个 1.4MB 本地文件到 LocalQuark
- Finder 出现两个文件:
  - `cloudflare-cfnew-少年你相信光吗.txt` —— **0 bytes**
  - `cloudflare-cfnew-少年你相信光吗(1).txt` —— **1.4MB** (真文件)
- Finder **不会问** "是否覆盖" / "Keep Both", 直接 keep both 加 `(1)` 后缀

### 原因分析 (铁证证据链)

1. **`do_flush` (vfs.rs:1004) 注释 P1-2 明确说**：empty-file PUT (write_buf was never called because the body is 0 bytes). Set up upload state so do_flush() creates the 0-byte file on the cloud (chunk_count=0 → empty commit)
   - **设计意图**就是让 0-byte PUT 真在云端创建 0-byte file
   - 但是这个设计假设"用户真的传 0-byte 文件"

2. **macOS webdavfs_agent 在 Finder drag-drop 时不是这种行为**：它先做一个 0-byte PUT (准备 target vnode), 然后立即做带 body 的 PUT. 我们用 PROPFIND 看云端就能证实:
   ```
   $ curl PROPFIND /cloudflare-cfnew-少年你相信光吗.txt
   <D:creationdate>2026-07-07T04:28:17Z</D:creationdate>
   <D:getcontentlength>0</D:getcontentlength>
   ```
   这个 0-byte 文件的 creationdate = **04:28:17** = 用户**刚才 drag-drop 的瞬间**!

3. **事件链**:
   a. Finder drag-drop 1.4MB
   b. webdavfs_agent 发 **0-byte PUT /target** (vnode probe)
   c. 我们 do_flush → up_pre 创建 fid_A → commit/finish → **云端有 fid_A (0 bytes)**
   d. webdavfs_agent 发 **1.4MB PUT /target with body**
   e. 我们 open() 找到 fid_A → do_flush(old=A) → up_pre 创建 fid_B → chunks → commit/finish → 注册 active_write
   f. **理论上** remove_file(old=A) 应该删 A, 但 (1) 怎么来的?

4. **findings**:
   - 0-byte file 在云端真存在 (curl PROPFIND 证实, creationdate = drag-drop 时刻)
   - (1) file 也在云端真存在 (curl PROPFIND 证实, 1.4MB MD5 = 7d71e8...)
   - 也就是说**两条 cloudflare 文件都真在云端**, 不是 vnode cache 残留

5. **(1) 怎么来的 — 真正的根因**:
   - webdavfs_agent 在步骤 (c) 看到 server 已经创建了 0-byte fid_A
   - 它把这条结果告诉 Finder
   - Finder vnode layer 检测到 target 已存在
   - **Finder 默认 Keep Both** (Finder → Settings → Advanced → When copying duplicate files: Keep Both 是默认)
   - Finder 决定: 不替换 A, 而是给 client 加 (1) 后缀重试
   - webdavfs_agent PUT (1) 1.4MB → 我们 create-new path → 上传成功 → 云端有 (1)
   - **结果**: cloud A (0 byte) + cloud (1) (1.4MB)

### 修复

`vfs.rs::do_flush` 开头加 guard: 当 `size == 0 && old_fid.is_none()` 时跳过 up_pre.

```rust
// BUG #6 fix: skip up_pre for "create new + size=0" PUTs.
if size == 0 && old_fid.is_none() {
    debug!(file_name = %self.file.file_name,
           "do_flush: 0-byte brand-new PUT — skip up_pre (webdavfs_agent vnode probe)");
    self.upload_state.is_finished = true;
    self.after_flush().await?;
    return Ok(());
}
```

原理: webdavfs_agent 的 0-byte probe 之后**几乎总会**有 body PUT, 当 body PUT 来的时候:
- open() 走 create-new path (因为 fid 不存在)
- do_flush 走正常 up_pre + chunks + commit/finish
- 没有 old_fid, 不需要 remove_file
- 只有 1 个文件 (1.4MB) 落到云端

**trade-off**: 如果用户**真的**拖一个真正的空白文件, 它不会上传到云端。这比"每次拖文件都生成 (1)"好很多, 空白文件场景罕见 (一般文件 > 0 字节)。

### 改动结果 (端到端验证)

```
=== 旧行为 (Bug #6) ===
拖文件 → 云端有 (0 bytes + 1.4MB)

=== 新行为 (修复后) ===
$ curl -X PUT Content-Length: 0 → 201 OK
$ curl PROPFIND target
  <D:status>HTTP/1.1 404 Not Found</D:status>   ← 云端没有这个 0 byte 文件!
  content-length: (absent)
```

修复后 server:
- 接收 0-byte probe PUT → 201 OK (webdavfs_agent 满意,认为 vnode 已 prepare)
- **不**真在云端创建 0-byte file
- 当 webdavfs_agent 后续发 body PUT → 走 create-new path → 上传成功 → 只有 1 个文件

### 教训

1. **不要假设客户端 SDK 的行为**: macOS webdavfs_agent 的 0-byte probe 是它自己的"vnode prepare"机制, 但 server 不该把它当真创建云端文件。要看客户端代码/实测, 不要凭 P1-2 那种"想当然"的设计意图。

2. **PROPFIND 时间戳是金矿**: 当 0-byte 文件 creationdate = 用户 drag-drop 时刻, 说明这就是这次操作产生的, 不是历史遗留。结合 webdav log, 就能完整还原事件链。

3. **macOS Finder 默认 Keep Both**: Finder → Settings → Advanced → When copying duplicate files → 默认是 "Keep Both", 不是 "Replace". 这跟 webdav 无关, 是 macOS 标准行为。要避免 Finder 自动生成 (1), 必须保证客户端提供给 server 的 target 是唯一的 (server 端不能创建 0-byte).

4. **log level 配错了就要小心**: INFO level 把 PROXY request 也 filter 出去, 但 webdav handler 在 debug. 排查 server-side 问题前, 应该用 `RUST_LOG=debug` 跑一次 repro.

5. **bug修复经验要写得让下次遇到同样问题的人能复用**: 这次 P1-2 的注释是个陷阱, 后来人会以为"这是 design choice". 我加了 BUG #6 fix 注释明确写 "0-byte brand-new PUT skip up_pre (webdavfs_agent vnode probe)" 让意图清晰.

### 受影响文件

- ``quarkdrive-webdav/src/vfs.rs`` (`do_flush` 开头加 guard)

## Bug #7: GitHub 发布/本地构建提示依赖 legacy Python bundle（核心 Swift Helper 与 bin 脚本未解耦）

**日期**: 2026-07-07
**触发场景**: 用户尝试发布到 GitHub，或他人本地 clone 仓库后直接执行构建 `./scripts/build-app.sh` 提示找不到 `LocalQuarkHelper` 二进制。

### 故障表现

- 用户尝试将代码发布到 GitHub，提示存在对未提交的 `legacy/` 目录中编译产物的隐性强依赖。
- 干净环境下执行打包脚本时，因为 `.gitignore` 忽略了 Swift 的 `.build` 缓存目录，导致 `LocalQuarkHelper` 缺失，报错：`ERROR: missing .../LocalQuarkHelper`，无法完成本地打包构建。

### 原因分析

1. **构建脚本路径硬编码**：打包脚本 [build-app.sh](./scripts/build-app.sh) 默认硬编码从 `legacy/LocalQuark-python-bundle/LocalQuark/helper/.build/` 缓存目录中拉取编译好的 `LocalQuarkHelper` 辅助程序，以及从 `legacy/LocalQuark-python-bundle/LocalQuark/bin/` 拷贝 app 运行时的引导脚本。
2. **Git 忽略了编译缓存**：`LocalQuarkHelper` 在 `legacy` 目录下的 `.build` 构建缓存目录被 `.gitignore` 忽略，未曾提交到代码库。
3. **结果**：导致全新克隆仓库、或在 CI 服务（如 GitHub Actions）上执行构建的用户因缺少上述文件无法直接完成打包。

### 修复

1. **物理迁移资源**：将 Swift Helper 源码 `legacy/LocalQuark-python-bundle/LocalQuark/helper` 整体移动到项目根目录下的 [helper/](./helper)；将 8 个 app 运行时引导脚本从 `legacy/LocalQuark-python-bundle/LocalQuark/bin` 移动到项目根目录下的 [scripts/bin/](./scripts/bin/)；调试 Cookie 样本移动至 `scripts/bin/cookies.json`。
2. **解耦路径依赖**：
   - 更新 [build-app.sh](./scripts/build-app.sh) 里的 `HELPER_BIN`、`HELPER_PLIST_SRC` 和 `SCRIPTS_SRC` 变量，彻底摆脱 `legacy/` 路径，指向根目录的解耦目录。
   - 更新 [restart_app.sh](./scripts/restart_app.sh) 的调试 Cookie 同步路径。
   - 更新 [.gitignore](./.gitignore) 规则，忽略根目录下新 Swift 模块的编译缓存路径 `helper/.build/`。

### 改动结果 (端到端验证)

1. **独立编译通过**：
   - 清理缓存并在根目录下重新编译：`cd helper && rm -rf .build && swift build -c release`。成功无错编译，生成特权 Swift Helper。
2. **打包部署通过**：
   - 运行 `./scripts/build_deploy_test.sh`：自动完成 Rust 与 Swift 二进制编译，成功打包为 `dist/LocalQuark-rust.app`，并在系统 `/Applications` 下部署和拉起服务，`/Volumes/LocalQuark` 成功挂载。
3. **重启调试通过**：
   - 运行 `./scripts/restart_app.sh`：成功自动提取 `scripts/bin/cookies.json` 作为最新 Cookie，完美拉起应用，挂载成功，且 WebDAV `PROPFIND` 测试返回 `1 entries at root`。

### 教训

1. **彻底解耦重构**：从旧技术栈迁移时，如果涉及系统辅助程序的构建和运行时脚本，必须将其与历史遗留包完全脱钩，全部提升为项目根目录级的原生模块。
2. **不可隐式依赖忽略文件**：构建依赖链上严禁引用被 `.gitignore` 屏蔽的文件或本地特定的编译缓存（例如 Swift 的 `.build` 缓存目录），构建系统必须支持从干净的克隆环境中通过简单命令自行编译所有依赖组件。

### 受影响文件

- ``scripts/build-app.sh`` (修改依赖资源提取路径)
- ``scripts/restart_app.sh`` (修改 Cookie 源路径)
- ``.gitignore`` (追加 `helper/.build/` 忽略)
