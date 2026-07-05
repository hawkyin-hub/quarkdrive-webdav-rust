# 性能基线与优化记录

> 仓库路径：`/Users/HawkSept/myproject/myapp/localquark-rust`
> 最后更新：2026-07-05
>
> 本文档记录性能优化的真实状态、可复现测试命令、已知的"踩坑"。
> docs（AGENTS.md / ARCHITECTURE.md / README.md / CHANGELOG.md）里的 Phase 表格以本文档为准。

---

## 1. 真实状态（2026-07-05）

| 阶段 | 名义目标 | 真实状态 | 锚点源码 |
|------|---------|---------|---------|
| **Phase 1** | `upload_chunk` 改 `buffer_unordered(4)` 并发 | 已回滚为串行。夸克 API 元数据返回 `part_thread:1`，并发上传触发 `PartNotSequential` 500 错误。代码注释明确说明（`vfs.rs:1057`）。 | `quarkdrive-webdav/src/vfs.rs` `do_flush` 中 spawn 的上传循环；`upload_chunk` 函数（line 1341+） |
| **Phase 2** | 删除 7 处 `sleep(1s)` + 清缓存后 `sleep(2s)` | 真实生效。`grep "sleep" vfs.rs` 为空。 | `vfs.rs` 的 `remove_file` / `remove_dir` / 缓存清除路径 |
| **Phase 3** | 移除 `consume_buf` 中的 `file.flush()` | 真实生效。`consume_buf` 函数（line 1305+）无 `file.flush()` 调用，依赖 OS 在 file handle 关闭时自动 flush。 | `vfs.rs::consume_buf` |

> 历史文档（README/AGENTS/ARCHITECTURE/CHANGELOG）中提到的"Phase 1：5MB 上传从 ~30s → ~8s"是一次**实测有效的并发版本**，但因为 Quark API 不允许并发上传，已被回滚。当前基准不能引用那个数字。

---

## 2. 可复现的基准方法

### 2.1 上传吞吐（5MB 文件）

```bash
# 1) 启动 daemon（带 cookie 避免浏览器抓取）
cd /Users/HawkSept/myproject/myapp/localquark-rust/quarkdrive-webdav
cargo build --release
./target/release/quarkdrive-webdav \
  --quark-cookie "$(cat ~/Library/Application\ Support/LocalQuark/cookies.json | jq -r '.quark_cookie')" \
  --mount-point "$HOME/Mount/QuarkPerf" \
  --serve-only &

# 2) 等 backend ready
for i in $(seq 1 30); do
  curl -fs -k -X OPTIONS -u quasar:"$(cat ~/Library/Application\ Support/QuarkDrive/webdav.passwd)" \
    https://127.0.0.1:8443/ >/dev/null 2>&1 && break
  sleep 1
done

# 3) 准备测试文件
dd if=/dev/urandom of=/tmp/perf-5mb.bin bs=1M count=5 status=none

# 4) PUT 一次，记录耗时
PASS="$(cat ~/Library/Application\ Support/QuarkDrive/webdav.passwd)"
START=$(date +%s.%N)
curl -fs -k -X PUT -u "quasar:$PASS" \
  --data-binary @/tmp/perf-5mb.bin \
  https://127.0.0.1:8443/tmp/perf-5mb.bin
END=$(date +%s.%N)
echo "upload_5MB=$(echo "$END - $START" | bc)s"
```

### 2.2 目录列表延迟

```bash
PASS="$(cat ~/Library/Application\ Support/QuarkDrive/webdav.passwd)"
START=$(date +%s.%N)
curl -fs -k -X PROPFIND -H "Depth: 1" -u "quasar:$PASS" \
  https://127.0.0.1:8443/ >/dev/null
END=$(date +%s.%N)
echo "propfind_root=$(echo "$END - $START" | bc)s"
```

### 2.3 RSS 占用

```bash
ps -o pid,rss,command -p $(pgrep -f quarkdrive-webdav)
```

---

## 3. 当前实测数据（2026-07-05，待补完）

下面数据需要 5MB 上传 + 目录 PROPFIND + 5 分钟 idle 后采集，**不要直接引用历史数字**：

| 指标 | Phase 2+3 之后（实测） | 历史"优化前"（已不可复现） |
|------|------------------------|----------------------------|
| 5MB PUT 耗时 | _TBD_ | ~30s（含并发版短暂有效时） |
| 根 PROPFIND 耗时 | _TBD_ | ~2s |
| idle RSS | _TBD_ | ~50MB |

### 测试条件

- macOS 26.6+（webdavfs_agent 强 HTTPS）
- bundled `.app` 走 `/Volumes/LocalQuark`
- CLI 直跑走 `$HOME/Mount/Quark`
- 局域网 Wi-Fi（不模拟高延迟）
- 不在 macOS App Nap 抑制窗口

---

## 4. 已知性能陷阱（不要重蹈覆辙）

1. **并发上传** - Quark 上传 API 元数据返回 `part_thread:1`，并发 chunk 会触发 `PartNotSequential` 错误并返回 500。
   - 改进空间：未来若 Quark 支持并发（返回 `part_thread>1`），再启用 `futures::stream::iter().buffer_unordered(N)`。

2. **每 chunk flush** - `file.flush()` 每写一次就触发一次 syscall + page cache flush。
   - 改进空间：依赖 OS 在 file handle drop 时自动 flush（tokio `File` 走 RAII）。

3. **`sleep` 同步缓存** - `remove_file` / `remove_dir` 后 `sleep(1s)` 等 API 生效是反 pattern。
   - 改进空间：API 成功后立刻 `cache.invalidate()`，不 sleep。

4. **mount_webdav 上传大文件** - macOS 自带 `mount_webdav` 对大文件 PUT 有超时。建议 <100MB 用 Finder，>100MB 用 CyberDuck / rclone。

5. **`tokio::io::copy` 而不是 `body.collect()`** - 当前 `webdav.rs` 的 PUT handler 已经走 streaming（`Body::into_data_stream`），不要回退到 `to_bytes()`。

---

## 5. 未来优化候选（按风险/收益排序）

| 候选 | 预期收益 | 风险 | 前置条件 |
|------|---------|------|---------|
| 客户端并发分片（≥4） | 上传吞吐 ×2~3 | 中（必须先验证 API 支持） | Quark API 返回 `part_thread>1` |
| `moka` 预热策略 | 目录列表 -30% | 低 | 已在 root 启动时做一次 PROPFIND，可推广到常用路径 |
| `quark` 目录列表走 `if-modified-since` | 大目录 -50% | 中 | Quark API 必须支持 ETag |
| tokio runtime 调优 (`max_blocking_threads`) | 突发上传 +20% | 低 | 已在 main.rs 显式配置 |

---

## 6. 引用

- 架构与模块职责：[ARCHITECTURE.md](../ARCHITECTURE.md)
- 仓库根 AGENTS：[AGENTS.md](../AGENTS.md)
- 排错手册：[troubleshooting.md](troubleshooting.md)
- 部署：[DEPLOYMENT.md](DEPLOYMENT.md)
- 版本变更：[CHANGELOG.md](../CHANGELOG.md)
