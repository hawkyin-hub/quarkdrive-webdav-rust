use std::path::{Path, PathBuf};
use std::time::Duration;
use moka::future::Cache as MokaCache;
use tracing::{debug, warn};
use crate::drive::{QuarkDrive};
use crate::drive::model::QuarkFile;
use bytes::Bytes;

#[derive(Clone)]
pub struct Cache {
    inner: MokaCache<String, Vec<QuarkFile>>,
    drive: QuarkDrive,
    disk_root: PathBuf,
}
const ONE_PAGE: u32 = 500;

impl Cache {
    pub fn new(max_capacity: u64, ttl: u64, drive: QuarkDrive) -> Self {
        let inner = MokaCache::builder()
            .max_capacity(max_capacity)
            .time_to_live(Duration::from_secs(ttl))
            .build();
        // 磁盘持久化路径：模拟 legacy Python DiskCache 的
        // ~/Library/Caches/LocalQuark/propfind/<sha256(path)>.json。
        // 签名不能改（webdav.rs 硬编码 (drive, 100, 60)），
        // 内部从 HOME 环境变量推导，fallback /tmp。
        let disk_root = std::env::var("HOME")
            .map(|h| PathBuf::from(h).join("Library/Caches/LocalQuark/propfind"))
            .unwrap_or_else(|_| PathBuf::from("/tmp/LocalQuark/propfind"));
        if let Err(e) = std::fs::create_dir_all(&disk_root) {
            warn!(path = %disk_root.display(), error = %e,
                  "Cache::new: failed to create disk root, fallback to no-disk");
        }
        Self { inner, drive, disk_root }
    }

    fn disk_path(&self, key: &str) -> PathBuf {
        // SHA-256(key) -> hex。key 唯一确定性，目录遍历稳定性靠 moka。
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // 使用 std::hash 默认 hasher 即可，不需要密码学强度，跨进程稳定。
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        let key_hash = format!("{:016x}", h.finish());
        self.disk_root.join(format!("{}.json", key_hash))
    }

    async fn read_disk(&self, key: &str) -> Option<Vec<QuarkFile>> {
        let p = self.disk_path(key);
        match tokio::fs::read(&p).await {
            Ok(bytes) => match serde_json::from_slice::<Vec<QuarkFile>>(&bytes) {
                Ok(v) => Some(v),
                Err(e) => {
                    warn!(path = %p.display(), error = %e,
                          "disk cache: deserialize failed, ignoring");
                    None
                }
            },
            Err(_) => None, // 文件不存在直接返回 None
        }
    }

    fn write_disk(&self, key: String, value: Vec<QuarkFile>) {
        let disk_root = self.disk_root.clone();
        let path = self.disk_path(&key);
        // 异步落盘：spawn 后立刻返回，不阻塞调用方。
        tokio::spawn(async move {
            match serde_json::to_vec(&value) {
                Ok(bytes) => {
                    if let Err(e) = tokio::fs::write(&path, &bytes).await {
                        warn!(path = %path.display(), error = %e,
                              "disk cache: write failed");
                    } else {
                        debug!(path = %path.display(), key = %key,
                               "disk cache: write ok");
                    }
                }
                Err(e) => warn!(path = %path.display(), error = %e,
                                "disk cache: serialize failed"),
            }
            let _ = disk_root; // 保留字段，避免未来 use 后未用警告
        });
    }

    async fn delete_disk(&self, key: &str) {
        let p = self.disk_path(key);
        let _ = tokio::fs::remove_file(&p).await;
    }
    pub async fn get_or_insert(&self, key: &str) -> Option<Vec<QuarkFile>> {
        debug!(key = %key, "cache: get_or_insert");
        if let Some(files) = self.get(key).await {
            return Some(files);
        }
        if key == "/" {
            self.dfs(QuarkFile::new_root(), key, "/").await;
        }else {
            let mut path = Path::new(key);
            let mut dsf_root_file = None;
            // 修复：父目录缓存命中但找不到子目录时，强制刷新父目录缓存。
            // 旧逻辑只 path = parent 跳走，导致陈旧缓存持续污染，
            // 切换目录假死 2-3s。
            let mut stale_parent_refreshed = false;
            while let Some(parent) = path.parent() {
                if let Some(c_files) = self.get(parent.to_str().unwrap()).await {
                    let file_name = path.file_name().and_then(|os_str| os_str.to_str());
                    let found = c_files.iter().find(|quark_file| {
                        Some(quark_file.file_name.as_str()) == file_name
                    }).cloned();
                    if found.is_none() {
                        debug!(key = %key, "cache: no file found for path: {}", path.to_str().unwrap());
                        if !stale_parent_refreshed {
                            // 第一次发现父缓存里没目标文件，强制 invalidate 父目录缓存并从根目录重走
                            debug!(key = %key, "cache: stale parent detected, invalidating {} and re-dfs from root", parent.to_str().unwrap());
                            self.inner.invalidate(parent.to_str().unwrap()).await;
                            stale_parent_refreshed = true;
                            path = Path::new(key);
                            dsf_root_file = None;
                            continue;
                        }
                        path = parent;
                        continue;
                    }
                    dsf_root_file = found;
                    break;
                }

                path = parent;

                if path.to_str() == Some("/") {
                    break;
                }

            }
            if path.to_str() == Some("/") {
                self.dfs(QuarkFile::new_root(), key, "/").await;
            }else {
                match dsf_root_file { 
                    Some(dsf_root_fil) => {
                        debug!(key = %key, "cache: found root file: {}", dsf_root_fil.file_name);
                        self.dfs(dsf_root_fil, key, path.to_str().unwrap()).await;
                    },
                    None => {
                        debug!(key = %key, "cache: no root file found for path: {}", path.to_str().unwrap());
                        return None;
                    }
                }
            }

        }
        if let Some(files) = self.get(key).await {
            Some(files)
        }else {
            debug!(key = %key, "cache: no files found for key");
            None
        }
    }

    async fn dfs(&self, file: QuarkFile, target_path: &str, dfs_path: &str) {
        if file.dir {
            let mut current_files = Vec::<QuarkFile>::new();
            // dfs() 并发化（SVIP 调优）：第 1 页同步拿 total + 处理根级 None 分支；
            // 剩余页用 futures_util buffered(7) 并发拉取。
            // 总并发上限 = 1 (sync) + 7 (concurrent) = 8，留余量。
            let page1 = self.drive.get_files_by_pdir_fid(&file.fid, 1u32, ONE_PAGE).await;
            let (mut files, total) = match page1 {
                Ok((k, v)) => (k, v),
                Err(e) => {
                    debug!(error = %e, file_id = &file.fid, file_name = &file.file_name,
                           page_no = 1u32,
                        "Failed to get files from drive");
                    return;
                }
            };
            // 修复：get_files_by_pdir_fid 返回 Ok((None, _)) 表示 Quark 返回 404
            // （fid 已失效 / 父目录被删 / 权限不足）。保留原"invalidate + 从根重 dfs"路径。
            let mut files = match files {
                Some(f) => f,
                None => {
                    debug!(file_id = %file.fid, file_name = &file.file_name,
                           page_no = 1u32, dfs_path = %dfs_path,
                        "dfs: get_files_by_pdir_fid returned None, invalidating and re-dfs from root");
                    self.inner.invalidate(dfs_path).await;
                    Box::pin(self.dfs(QuarkFile::new_root(), target_path, "/")).await;
                    return;
                }
            };
            // add dfs_path to each file (page 1)
            for f in files.list.iter_mut() {
                f.parent_path = Some(dfs_path.to_string());
            }
            let size = files.list.len();
            current_files.extend(files.list);
            // guess: es limit is 10000
            let total_pages = total / ONE_PAGE + 1;
            let mut last_page: u32 = total_pages.min(20) as u32;
            if size < ONE_PAGE as usize || last_page < 2 {
                last_page = 1;
            }
            // 2..=last_page 用 futures_util buffered(7) 并发拉取
            if last_page >= 2 {
                use futures_util::stream::{self, StreamExt};
                let fid_owned = file.fid.clone();
                let path_owned = dfs_path.to_string();
                let pages = stream::iter(2u32..=last_page)
                    .map(|p| {
                        let fid = fid_owned.clone();
                        let path = path_owned.clone();
                        async move {
                            // P2-4: retry each page a couple of times with
                            // backoff so a single transient 5xx/limit doesn't
                            // drop the rest of the directory listing.
                            let mut r = self.drive.get_files_by_pdir_fid(&fid, p, ONE_PAGE).await;
                            for attempt in 1u32..=2 {
                                if r.is_ok() { break; }
                                tokio::time::sleep(Duration::from_millis(400 * attempt as u64)).await;
                                r = self.drive.get_files_by_pdir_fid(&fid, p, ONE_PAGE).await;
                            }
                            (p, path, r)
                        }
                    })
                    .buffered(7);
                futures_util::pin_mut!(pages);
                while let Some((p, path, res)) = pages.next().await {
                    match res {
                        Ok((Some(mut f), _)) => {
                            for x in f.list.iter_mut() {
                                x.parent_path = Some(path.clone());
                            }
                            current_files.extend(f.list);
                        }
                        Ok((None, _)) => {
                            debug!(page_no = p, dfs_path = %path,
                                "dfs: concurrent page returned None; keep what we have");
                        }
                        Err(e) => {
                            // P2-4: don't break — a single persistent page
                            // failure shouldn't drop all remaining pages.
                            warn!(error = %e, page_no = p, dfs_path = %path,
                                "dfs: page failed after retries; skipping (listing may be partial)");
                        }
                    }
                }
            }

            self.insert(dfs_path.to_string(), current_files.clone()).await;
            debug!("{} in cache", &dfs_path);
            if dfs_path == target_path {
                return;
            }
            for curr_f in current_files {
                let file_path = if dfs_path == "/" {
                    format!("{}{}", dfs_path, curr_f.file_name)
                }else {
                    format!("{}/{}", dfs_path, curr_f.file_name)
                };
                if target_path.starts_with(&file_path) {
                    Box::pin(self.dfs(curr_f, target_path, &file_path)).await;
                }
            }

        }
    }

    async fn get(&self, key: &str) -> Option<Vec<QuarkFile>> {
        debug!(key = %key, "cache: get");
        // 第一层：moka（热数据秒返回）。
        if let Some(v) = self.inner.get(key).await {
            return Some(v);
        }
        // 第二层：磁盘（冷启动 / 重启后秒加载）。
        if let Some(v) = self.read_disk(key).await {
            debug!(key = %key, "cache: get -> disk hit, promoting to moka");
            // promote 回 moka 让 TTL 计时重启。
            self.inner.insert(key.to_string(), v.clone()).await;
            return Some(v);
        }
        None
    }

    async fn insert(&self, key: String, value: Vec<QuarkFile>) {
        debug!(key = %key, "cache: insert");
        // 同步写 moka，异步落盘——spawn 后立即返回。
        let disk_value = value.clone();
        self.write_disk(key.clone(), disk_value);
        self.inner.insert(key, value).await;
    }

    pub async fn invalidate(&self, path: &Path) {
        let key = path.to_string_lossy().into_owned();
        debug!(path = %path.display(), key = %key, "cache: invalidate");
        self.inner.invalidate(&key).await;
        self.delete_disk(&key).await;
    }

    pub async fn invalidate_parent(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            self.invalidate(parent).await;
        }
    }

    pub fn invalidate_all(&self) {
        debug!("cache: invalidate all");
        self.inner.invalidate_all();
    }

    // ---------------- Chunk cache for large files ----------------
    // 对齐 legacy Python DiskCache：把每个 (path, start, end) 的 range
    // 落盘到 ~/Library/Caches/LocalQuark/chunks/<sha256(path)>/<start>-<end>.bin。
    // vfs.rs::read_bytes 在拉网络之前先 read_chunk，命中就直接返回，
    // 不命中走原流程，落盘到下次复用。
    // TTL 由文件 mtime 决定（24h 过期），不维护单独的 meta。

    fn chunk_dir(&self, path: &str) -> PathBuf {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // disk_root = .../propfind，chunks 是其兄弟目录。
        let chunks_root = self.disk_root.parent()
            .map(|p| p.join("chunks"))
            .unwrap_or_else(|| PathBuf::from("/tmp/LocalQuark/chunks"));
        let mut h = DefaultHasher::new();
        path.hash(&mut h);
        let key = format!("{:016x}", h.finish());
        chunks_root.join(key)
    }

    fn chunk_path(&self, path: &str, start: u64, end: u64) -> PathBuf {
        self.chunk_dir(path).join(format!("{}-{}.bin", start, end))
    }

    /// 检查是否有完全匹配 (path, start, length) 的缓存。
    /// 命中且 24h 内，直接返回 Bytes；否则 None。
    pub async fn read_chunk(&self, path: &str, start: u64, length: usize) -> Option<Bytes> {
        let end = start + length as u64;
        let p = self.chunk_path(path, start, end);
        let bytes = match tokio::fs::read(&p).await {
            Ok(b) => b,
            Err(_) => return None,
        };
        if bytes.len() != length {
            return None;
        }
        // TTL 通过 mtime 检查。
        if let Ok(meta) = tokio::fs::metadata(&p).await {
            if let Ok(modified) = meta.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed.as_secs() > 24 * 3600 {
                        let _ = tokio::fs::remove_file(&p).await;
                        return None;
                    }
                }
            }
        }
        Some(Bytes::from(bytes))
    }

    /// 删除某个路径的所有 chunk 缓存（同步 spawn 一个清盘任务）。
    /// 在上传完成 / 删除文件 / 重命名后调用，避免命中陈旧字节。
    /// 这里不开 async fn 是因为调用方（vfs.rs 几个 invalidate 触点）
    /// 多数是 async 块中间调，开 tokio::spawn 即可立即返回。
    pub fn invalidate_chunks(&self, path: &str) {
        let chunks_root = self.disk_root.parent()
            .map(|p| p.join("chunks"))
            .unwrap_or_else(|| PathBuf::from("/tmp/LocalQuark/chunks"));
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        path.hash(&mut h);
        let key = format!("{:016x}", h.finish());
        let dir = chunks_root.join(&key);
        tokio::spawn(async move {
            match tokio::fs::remove_dir_all(&dir).await {
                Ok(_) => debug!(path = %dir.display(), "chunk cache: invalidated all chunks for key"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => warn!(path = %dir.display(), error = %e,
                                  "chunk cache: invalidate failed"),
            }
        });
    }

    /// 把一个 (path, start..end) 字节块异步写到磁盘，不阻塞调用方。
    pub fn write_chunk(&self, path: String, start: u64, data: Vec<u8>) {
        let dir = self.chunk_dir(&path);
        let end = start + data.len() as u64;
        let p = self.chunk_path(&path, start, end);
        tokio::spawn(async move {
            if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                warn!(path = %dir.display(), error = %e, "chunk cache: mkdir failed");
                return;
            }
            if let Err(e) = tokio::fs::write(&p, &data).await {
                warn!(path = %p.display(), error = %e, "chunk cache: write failed");
            } else {
                debug!(path = %p.display(), start = start, end = end,
                       "chunk cache: write ok");
            }
        });
    }
}

