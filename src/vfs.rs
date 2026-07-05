use std::fmt::{Debug, Formatter};
use std::io::{SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use bytes::{Buf, Bytes, BytesMut};
use dashmap::DashMap;
use dav_server::{
    davpath::DavPath,
    fs::{
        DavDirEntry, DavFile, DavFileSystem, DavMetaData, FsError, FsFuture, FsStream, OpenOptions,
        ReadDirMeta,
    },
};
use futures_util::future::{ready, FutureExt};
use futures_util::stream::StreamExt;
use tracing::{debug, error, info, trace};
use crate::{
    cache::Cache,
    drive::{QuarkDrive, QuarkFile},
};
use bytes::BufMut;

use md5::Context as Md5Context;
use sha1::Sha1;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex as AsyncMutex;

use sha1::Digest;
use tokio::fs::File;

use crate::drive::model::{Callback, UpAuthAndCommitRequest, UpPartMethodRequest};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Clone, Debug)]
pub struct ActiveWriteInfo {
    pub file_name: String,
    pub size: u64,
    pub updated_at: u64,
    pub body: Vec<u8>,
    pub created_at: std::time::Instant,
}

#[derive(Clone)]
pub struct QuarkDriveFileSystem {
    pub(crate) drive: QuarkDrive,
    pub(crate) dir_cache: Cache,
    pub(crate) uploading: Arc<DashMap<String, Vec<QuarkFile>>>,
    pub(crate) active_writes: Arc<DashMap<String, ActiveWriteInfo>>,
    pub(crate) root: PathBuf,
    no_trash: bool,
    read_only: bool,
    upload_buffer_size: usize,
    skip_upload_same_size: bool,
    prefer_http_download: bool,
    upload_wait_timeout: u64,
    /// Filesystem-wide monotonic counter for unique temp-file names.
    /// Pinned in atomic so concurrent calls to `prepare_for_upload` produce
    /// strictly distinct paths even when the wall-clock ms collides
    /// (fixes C1: temp_file_path ms-collision under concurrent PUTs).
    temp_seq: Arc<AtomicU64>,
    /// Per-path async mutex registry. Each value is an `AsyncMutex<()>` that
    /// is created lazily the first time a write to that path arrives; each
    /// subsequent PUT waits on it before doing any destructive preflight
    /// (`remove_file` of the old fid) so concurrent PUTs to the same path
    /// are serialized end-to-end (fixes C2: same-fid `remove_file` race).
    write_locks: Arc<DashMap<PathBuf, Arc<AsyncMutex<()>>>>,
    /// §2.2 Write Folding generation counter. Each `prepare_for_upload` bumps
    /// the counter for its path; `flush()` checks the counter *after* acquiring
    /// the write lock — if a newer generation exists the stale upload is
    /// skipped (temp file deleted, no network traffic wasted).
    upload_generation: Arc<DashMap<PathBuf, u64>>,
    /// Per-chunk async mutex registry. Each value is an `AsyncMutex<()>` that
    /// prevents concurrent threads from downloading the same chunk from network simultaneously.
    chunk_locks: Arc<DashMap<(String, u64), Arc<AsyncMutex<()>>>>,
}

impl QuarkDriveFileSystem {
    pub async fn register_active_write(&self, parent_path: &str, file_name: &str, size: u64, temp_file_path: &str) {
        // 清理超过 45 秒的过期记录
        self.active_writes.retain(|_, v| v.created_at.elapsed().as_secs() < 45);

        if size > 16 * 1024 * 1024 {
            return;
        }
        let body = if size == 0 {
            Vec::new()
        } else {
            // Under concurrent same-path PUTs (write-coalescing), the previous
            // writer's temp file may have already been `remove_file`'d by the
            // time later writers reach this point. That is expected and not an
            // error — those later writers are simply racing ahead of us, and
            // the cloud-side file they produced will become the truth. So we
            // down-grade "missing temp file" from `error!` to `debug!` and
            // still register an empty-body entry — the directory listing and
            // `metadata()` fallbacks will continue to surface the path so the
            // Finder/proxy doesn't see a ghost 404.
            match tokio::fs::read(temp_file_path).await {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    debug!("register_active_write: 临时文件 {} 已被并发清理，跳过 body 缓存（无错）", temp_file_path);
                    Vec::new()
                }
                Err(e) => {
                    debug!("register_active_write: 读取临时文件 {} 失败: {:?}（降级为空 body 注册）", temp_file_path, e);
                    Vec::new()
                }
            }
        };
        let utc_time = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_secs(),
            Err(_) => 0,
        };
        let info = ActiveWriteInfo {
            file_name: file_name.to_string(),
            size,
            updated_at: utc_time,
            body,
            created_at: std::time::Instant::now(),
        };
        let key = format!("{}/{}", parent_path.trim_end_matches('/'), file_name);
        self.active_writes.insert(key, info);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(drive: QuarkDrive, root: String, cache_size: u64, cache_ttl: u64) -> Result<Self> {
        let dir_cache = Cache::new(cache_size, cache_ttl, drive.clone());
        debug!("dir cache initialized");
        let root = if root.starts_with('/') {
            PathBuf::from(root)
        } else {
            Path::new("/").join(root)
        };
        Ok(Self {
            drive,
            dir_cache,
            uploading: Arc::new(DashMap::new()),
            active_writes: Arc::new(DashMap::new()),
            root,
            no_trash: false,
            read_only: false,
            upload_buffer_size: 16 * 1024 * 1024,
            skip_upload_same_size: false,
            prefer_http_download: false,
            upload_wait_timeout: 280,
            temp_seq: Arc::new(AtomicU64::new(0)),
            write_locks: Arc::new(DashMap::new()),
            upload_generation: Arc::new(DashMap::new()),
            chunk_locks: Arc::new(DashMap::new()),
        })
    }

    /// Resolve (and lazily create) the per-path write mutex.
    /// All writers that touch this exact path will queue on the same
    /// `AsyncMutex<()>` instance end-to-end. Returning the mutex by
    /// `Arc::clone` instead of holding the DashMap guard means the guard
    /// is dropped before the caller awaits `.lock().await`, avoiding the
    /// "lock held across await while holding sharded entry" footgun.
    pub(crate) fn write_lock_for(&self, path: PathBuf) -> Arc<AsyncMutex<()>> {
        if let Some(m) = self.write_locks.get(&path) {
            return m.value().clone();
        }
        let new_lock = Arc::new(AsyncMutex::new(()));
        // Race-safe insert: if someone else won the race we drop ours
        // and return the winning entry.
        self.write_locks
            .entry(path)
            .or_insert(new_lock.clone())
            .clone()
    }

    /// Resolve (and lazily create) the per-chunk download mutex.
    pub(crate) fn chunk_lock_for(&self, path: String, start_align: u64) -> Arc<AsyncMutex<()>> {
        let key = (path, start_align);
        if let Some(m) = self.chunk_locks.get(&key) {
            return m.value().clone();
        }
        let new_lock = Arc::new(AsyncMutex::new(()));
        self.chunk_locks
            .entry(key)
            .or_insert(new_lock.clone())
            .clone()
    }

    pub fn set_read_only(&mut self, read_only: bool) -> &mut Self {
        self.read_only = read_only;
        self
    }

    pub fn set_no_trash(&mut self, no_trash: bool) -> &mut Self {
        self.no_trash = no_trash;
        self
    }

    pub fn set_upload_buffer_size(&mut self, upload_buffer_size: usize) -> &mut Self {
        self.upload_buffer_size = upload_buffer_size;
        self
    }

    pub fn set_skip_upload_same_size(&mut self, skip_upload_same_size: bool) -> &mut Self {
        self.skip_upload_same_size = skip_upload_same_size;
        self
    }

    pub fn set_prefer_http_download(&mut self, prefer_http_download: bool) -> &mut Self {
        self.prefer_http_download = prefer_http_download;
        self
    }

    pub fn set_upload_wait_timeout(&mut self, upload_wait_timeout: u64) -> &mut Self {
        self.upload_wait_timeout = upload_wait_timeout;
        self
    }
    fn list_uploading_files(&self, parent_file_path: &str) -> Vec<QuarkFile> {
        self.uploading
            .get(parent_file_path)
            .map(|val_ref| val_ref.value().clone())
            .unwrap_or_default()
    }

    fn remove_uploading_file(&self, parent_file_path: &str, file_name: &str) {
        if let Some(mut files) = self.uploading.get_mut(parent_file_path) {
            if let Some(index) = files.iter().position(|x| x.file_name == file_name) {
                files.swap_remove(index);
            }
        }
    }

    async fn find_in_cache(&self, path: &Path) -> Result<Option<QuarkFile>, FsError> {
        if let Some(parent) = path.parent() {
            let parent_str = parent.to_string_lossy();
            let file_name = path
                .file_name()
                .ok_or(FsError::NotFound)?
                .to_string_lossy()
                .into_owned();
            let file = self.dir_cache.get_or_insert(&parent_str).await.and_then(|files| {
                for file in &files {
                    if file.file_name == file_name {
                        return Some(file.clone());
                    }
                }
                None
            });
            Ok(file)
        } else {
            let root = QuarkFile::new_root();
            Ok(Some(root))
        }
    }

    async fn get_file(&self, path: PathBuf) -> Result<Option<QuarkFile>, FsError> {
        let file = self.find_in_cache(&path).await?;
        if let Some(file) = file {
            trace!(path = %path.display(), file_id = %file.fid, "file found in cache");
            Ok(Some(file))
        } else {
            // find in drive
            Ok(None)
        }
    }


    pub(crate) async fn get_file_md5_for_path(&self, path: &Path) -> Option<String> {
        let file = self.get_file(path.to_path_buf()).await.ok()??;
        if file.fid.is_empty() {
            return None;
        }
        // Try cached md5 first (populated by get_download_urls during file serving)
        if let Some(md5) = self.drive.get_cached_md5(&file.fid) {
            return Some(md5);
        }
        // Fall back to API call
        self.drive.get_file_md5(&file.fid).await.ok()?
    }

    fn normalize_dav_path(&self, dav_path: &DavPath) -> PathBuf {
        let path = dav_path.as_pathbuf();
        if self.root.parent().is_none() || path.starts_with(&self.root) {
            return path;
        }
        let rel_path = dav_path.as_rel_ospath();
        if rel_path == Path::new("") {
            return self.root.clone();
        }
        self.root.join(rel_path)
    }
}

impl DavFileSystem for QuarkDriveFileSystem {
    fn open<'a>(
        &'a self,
        dav_path: &'a DavPath,
        options: OpenOptions,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        let path = self.normalize_dav_path(dav_path);
        let mode = if options.write { "write" } else { "read" };
        debug!(path = %path.display(), mode = %mode, "fs: open");
        async move {
            if options.append {
                // Can't support open in write-append mode
                error!(path = %path.display(), "unsupported write-append mode");
                return Err(FsError::NotImplemented);
            }
            let parent_path = path.parent().ok_or(FsError::NotFound)?;
            let parent_file = self
                .get_file(parent_path.to_path_buf())
                .await?
                .ok_or(FsError::NotFound)?;
            let sha1 = options.checksum.and_then(|c| {
                if let Some((algo, hash)) = c.split_once(':') {
                    if algo.eq_ignore_ascii_case("sha1") {
                        Some(hash.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            });

            #[cfg(feature = "local_upload_hash")]
            if options.write && path.is_file() && sha1.is_none() {
                if let Ok((_, sha1_val)) = calc_md5_sha1(&path) {
                    sha1 = Some(sha1_val);
                }
            }
            let mut file_opt = self.get_file(path.clone()).await.unwrap_or(None);
            if file_opt.is_none() {
                // 尝试从 active_writes 获取 (自愈)
                let path_str = path.to_string_lossy().to_string();
                if let Some(active_write) = self.active_writes.get(&path_str) {
                    if active_write.created_at.elapsed().as_secs() < 45 {
                        let now = active_write.updated_at * 1000;
                        file_opt = Some(QuarkFile {
                            fid: "".to_string(),
                            file_name: active_write.file_name.clone(),
                            pdir_fid: parent_file.fid.clone(),
                            size: active_write.size,
                            format_type: "application/octet-stream".to_string(),
                            status: 1,
                            dir: false,
                            file: true,
                            content_hash: None,
                            created_at: now,
                            updated_at: now,
                            download_url: None,
                            parent_path: Some(parent_path.to_string_lossy().into_owned()),
                        });
                    }
                }
            }
            if file_opt.is_none() {
                // 尝试从正在上传的列表中匹配 (上传占位匹配，要求文件名完全相同)
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                file_opt = self.list_uploading_files(&parent_path.to_string_lossy())
                    .into_iter()
                    .find(|x| x.file_name == file_name);
            }

            let mut dav_file = if let Some(file) = file_opt {
                if options.write && options.create_new {
                    return Err(FsError::Exists);
                }
                if options.write && self.read_only {
                    return Err(FsError::Forbidden);
                }
                QuarkDavFile::new(
                    self.clone(),
                    file,
                    parent_file.fid,
                    parent_path.to_path_buf(),
                    // Always start at 0: consume_buf() accumulates the actual bytes written
                    0u64,
                    sha1,
                )
            } else if options.write && (options.create || options.create_new) {
                if self.read_only {
                    return Err(FsError::Forbidden);
                }

                let size = options.size;
                let name = dav_path
                    .file_name()
                    .ok_or(FsError::GeneralFailure)?
                    .to_string();

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis();

                let file = QuarkFile {
                    fid: "".to_string(),
                    file_name: name,
                    pdir_fid: parent_file.fid.clone(),
                    size: size.unwrap_or(0),
                    format_type: "application/octet-stream".to_string(),
                    status: 1,
                    dir: false,
                    file: true,
                    content_hash: sha1.clone(),
                    created_at: now as u64,
                    updated_at: now as u64,
                    download_url: None,
                    parent_path: Some(parent_path.to_string_lossy().into_owned()),
                };

                let mut uploading = self.uploading.entry(parent_path.to_str().unwrap().to_string()).or_default();
                uploading.push(file.clone());
                QuarkDavFile::new(
                    self.clone(),
                    file,
                    parent_file.fid,
                    parent_path.to_path_buf(),
                   // size.unwrap_or(0),
                    // The client will not provide the size of large files,
                    // So the size is calculated uniformly by the post program
                    0u64,
                    sha1,
                )
            } else {
                return Err(FsError::NotFound);
            };
            dav_file.http_download = self.prefer_http_download;
            Ok(Box::new(dav_file) as Box<dyn DavFile>)
        }
            .boxed()
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        _meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        let path = self.normalize_dav_path(path);
        debug!(path = %path.display(), "fs: read_dir");
        async move {
            let files = self.dir_cache.get_or_insert(&path.to_string_lossy())
                .await
                .ok_or(FsError::NotFound)?;

            let path_str = path.to_string_lossy().to_string();
            let norm_path = path_str.trim_end_matches('/');

            // 1. 提取 active_writes 中属于当前目录的项
            let mut active_files = Vec::new();
            for entry in self.active_writes.iter() {
                let key = entry.key();
                let info = entry.value();
                if info.created_at.elapsed().as_secs() >= 45 {
                    continue;
                }
                let k_path = Path::new(key);
                if let Some(k_parent) = k_path.parent() {
                    let k_parent_str = k_parent.to_string_lossy().to_string();
                    let k_parent_norm = k_parent_str.trim_end_matches('/');
                    if k_parent_norm == norm_path {
                        let now = info.updated_at * 1000;
                        active_files.push(QuarkFile {
                            fid: "".to_string(),
                            file_name: info.file_name.clone(),
                            pdir_fid: "".to_string(),
                            size: info.size,
                            format_type: "application/octet-stream".to_string(),
                            status: 1,
                            dir: false,
                            file: true,
                            content_hash: None,
                            created_at: now,
                            updated_at: now,
                            download_url: None,
                            parent_path: Some(path_str.clone()),
                        });
                    }
                }
            }

            // 2. 提取正在上传的文件
            let uploading_files = self.list_uploading_files(&path_str);

            // 3. 去重与覆盖合并
            let mut merged_files = std::collections::HashMap::new();
            for file in files {
                merged_files.insert(file.file_name.clone(), file);
            }
            for file in active_files {
                merged_files.insert(file.file_name.clone(), file);
            }
            for file in uploading_files {
                merged_files.insert(file.file_name.clone(), file);
            }

            let mut v: Vec<Result<Box<dyn DavDirEntry>, FsError>> = Vec::with_capacity(merged_files.len());
            for (_, file) in merged_files {
                v.push(Ok(Box::new(file)));
            }

            let stream = futures_util::stream::iter(v);
            Ok(Box::pin(stream) as FsStream<Box<dyn DavDirEntry>>)
        }
            .boxed()
    }

    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        let mut path = self.normalize_dav_path(path);
        if path.as_path().to_str() == Some("0") {
            // root path
            debug!("fs: metadata for root");
            path = PathBuf::from("/");
        }
        debug!(path = %path.display(), "fs: metadata");
        async move {
            // if root return
            if path == self.root {
                debug!("fs: metadata for root");
                let root_file = QuarkFile::new_root();
                return Ok(Box::new(root_file) as Box<dyn DavMetaData>);
            }

            // 1. 尝试从 cache/云端获取
            let mut file = self.get_file(path.clone()).await.unwrap_or_else(|_| Option::None);
            
            // 2. 尝试从 active_writes 获取 (自愈)
            if file.is_none() {
                let path_str = path.to_string_lossy().to_string();
                if let Some(active_write) = self.active_writes.get(&path_str) {
                    if active_write.created_at.elapsed().as_secs() < 45 {
                        let parent_path = path.parent().ok_or(FsError::NotFound)?;
                        let now = active_write.updated_at * 1000;
                        file = Some(QuarkFile {
                            fid: "".to_string(),
                            file_name: active_write.file_name.clone(),
                            pdir_fid: "".to_string(),
                            size: active_write.size,
                            format_type: "application/octet-stream".to_string(),
                            status: 1,
                            dir: false,
                            file: true,
                            content_hash: None,
                            created_at: now,
                            updated_at: now,
                            download_url: None,
                            parent_path: Some(parent_path.to_string_lossy().into_owned()),
                        });
                    }
                }
            }

            // 3. 尝试从正在上传的列表中匹配 (上传占位匹配，要求文件名完全相同)
            if file.is_none() {
                let parent_path = path.parent().ok_or(FsError::NotFound)?;
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                file = self.list_uploading_files(&parent_path.to_string_lossy())
                    .into_iter()
                    .find(|x| x.file_name == file_name);
            };

            let file = file.ok_or(FsError::NotFound)?;

            Ok(Box::new(file) as Box<dyn DavMetaData>)
        }
            .boxed()
    }
    fn have_props<'a>(
        &'a self,
        _path: &'a DavPath,
    ) -> std::pin::Pin<Box<dyn futures_util::Future<Output = bool> + Send + 'a>> {
        Box::pin(ready(true))
    }

    fn get_prop(&self, dav_path: &DavPath, prop: dav_server::fs::DavProp) -> FsFuture<Vec<u8>> {
        let path = self.normalize_dav_path(dav_path);
        let prop_name = match prop.prefix.as_ref() {
            Some(prefix) => format!("{}:{}", prefix, prop.name),
            None => prop.name.to_string(),
        };
        debug!(path = %path.display(), prop = %prop_name, "fs: get_prop");
        async move {
            if prop.namespace.as_deref() == Some("http://owncloud.org/ns")
                && prop.name == "checksums"
            {
                let file = self.get_file(path).await?.ok_or(FsError::NotFound)?;
                if let Some(sha1) = file.content_hash {
                    let xml = format!(
                        r#"<?xml version="1.0"?>
                        <oc:checksums xmlns:d="DAV:" xmlns:nc="http://nextcloud.org/ns" xmlns:oc="http://owncloud.org/ns">
                            <oc:checksum>sha1:{}</oc:checksum>
                        </oc:checksums>
                    "#,
                        sha1
                    );
                    return Ok(xml.into_bytes());
                }
            }
            Err(FsError::NotImplemented)
        }
            .boxed()
    }

    fn get_quota(&self) -> FsFuture<(u64, Option<u64>)> {
        debug!("fs: get_quota");
        async move {
            let (used, total) = self.drive.get_quota().await.map_err(|err| {
                error!(error = %err, "get quota failed");
                FsError::GeneralFailure
            })?;
            Ok((used, Some(total)))
        }
            .boxed()
    }

    fn create_dir<'a>(&'a self, dav_path: &'a DavPath) -> FsFuture<'a, ()> {
        let path = self.normalize_dav_path(dav_path);
        debug!(path = %path.display(), "fs: create_dir");
        async move {
            if self.read_only {
                return Err(FsError::Forbidden);
            }
            let parent_path = path.parent().ok_or(FsError::NotFound)?;
            let parent_file = self
                .get_file(parent_path.to_path_buf())
                .await?
                .ok_or(FsError::NotFound)?;
            if !parent_file.dir {
                return Err(FsError::Forbidden);
            }
            // check if the folder already exists
            if self.get_file(path.clone()).await?.is_some() {
                return Err(FsError::Exists);
            }
            if let Some(name) = path.file_name() {
                self.dir_cache.invalidate(parent_path).await;
                let name = name.to_string_lossy().into_owned();
                self.drive
                    .create_folder(&parent_file.fid, &name)
                    .await
                    .map_err(|err| {
                        error!(path = %path.display(), error = %err, "create folder failed");
                        FsError::GeneralFailure
                    })?;
                self.dir_cache.invalidate(&path).await;
                self.dir_cache.invalidate_parent(&path).await;
                Ok(())
            } else {
                Err(FsError::Forbidden)
            }
        }
            .boxed()
    }


    fn remove_dir<'a>(&'a self, dav_path: &'a DavPath) -> FsFuture<'a, ()> {
        let path = self.normalize_dav_path(dav_path);
        debug!(path = %path.display(), "fs: remove_dir");
        async move {
            if self.read_only {
                return Err(FsError::Forbidden);
            }

            let file = self
                .get_file(path.clone())
                .await?
                .ok_or(FsError::NotFound)?;
            if !file.dir {
                return Err(FsError::Forbidden);
            }
            self.drive
                .remove_file(&file.fid, !self.no_trash)
                .await
                .map_err(|err| {
                    error!(path = %path.display(), error = %err, "remove directory failed");
                    FsError::GeneralFailure
                })?;
            self.dir_cache.invalidate(&path).await;
            self.dir_cache.invalidate_parent(&path).await;
            Ok(())
        }
            .boxed()
    }

    fn remove_file<'a>(&'a self, dav_path: &'a DavPath) -> FsFuture<'a, ()> {
        let path = self.normalize_dav_path(dav_path);
        debug!(path = %path.display(), "fs: remove_file");
        async move {
            if self.read_only {
                return Err(FsError::Forbidden);
            }

            let file = self
                .get_file(path.clone())
                .await?
                .ok_or(FsError::NotFound)?;
            if !file.file {
                return Err(FsError::Forbidden);
            }
            self.drive
                .remove_file(&file.fid, !self.no_trash)
                .await
                .map_err(|err| {
                    error!(path = %path.display(), error = %err, "remove file failed");
                    FsError::GeneralFailure
                })?;
            self.dir_cache.invalidate_parent(&path).await;
            Ok(())
        }
            .boxed()
    }

    fn copy<'a>(&'a self, from_dav: &'a DavPath, to_dav: &'a DavPath) -> FsFuture<'a, ()> {
        // not support by quark api
        async move {
            Err(FsError::NotImplemented)
        }.boxed()
    }

    fn rename<'a>(&'a self, from_dav: &'a DavPath, to_dav: &'a DavPath) -> FsFuture<'a, ()> {
        let from = self.normalize_dav_path(from_dav);
        let to = self.normalize_dav_path(to_dav);
        debug!(from = %from.display(), to = %to.display(), "fs: rename");
        async move {
            if self.read_only {
                return Err(FsError::Forbidden);
            }

            let is_dir;
            if from.parent() == to.parent() {
                // rename
                if let Some(name) = to.file_name() {
                    let file = self
                        .get_file(from.clone())
                        .await?
                        .ok_or(FsError::NotFound)?;
                    is_dir = file.dir;
                    let name = name.to_string_lossy().into_owned();
                    self.drive
                        .rename_file(&file.fid, &name)
                        .await
                        .map_err(|err| {
                            error!(from = %from.display(), to = %to.display(), error = %err, "rename file failed");
                            FsError::GeneralFailure
                        })?;
                    self.dir_cache.invalidate_parent(&from).await;
                } else {
                    return Err(FsError::Forbidden);
                }
            } else {
                // move
                let file = self
                    .get_file(from.clone())
                    .await?
                    .ok_or(FsError::NotFound)?;
                is_dir = file.dir;
                let to_parent_file = self
                    .get_file(to.parent().unwrap().to_path_buf())
                    .await?
                    .ok_or(FsError::NotFound)?;
                let new_name = to_dav.file_name();
                self.drive
                    .move_file(&file.fid, &to_parent_file.fid)
                    // then rename ...
                    .await
                    .map_err(|err| {
                        error!(from = %from.display(), to = %to.display(), error = %err, "move file failed");
                        FsError::GeneralFailure
                    })?;
                if let Some(to_name) = new_name {
                    if let Some(from_name) = from_dav.file_name(){
                        if from_name != to_name {
                            self.drive.rename_file(&file.fid, to_name)
                                .await
                                .map_err(|err| {
                                    error!(from = %from.display(), to = %to.display(), error = %err, "rename file after move failed");
                                    FsError::GeneralFailure
                                })?;
                        }
                    }
                }
                self.dir_cache.invalidate_parent(&from).await;
                self.dir_cache.invalidate_parent(&to).await;

            }


            if is_dir {
                self.dir_cache.invalidate(&from).await;
            }
            self.dir_cache.invalidate_parent(&from).await;
            self.dir_cache.invalidate_parent(&to).await;
            Ok(())
        }
            .boxed()
    }

}

#[derive(Debug, Clone)]
struct UploadState {
    size: u64,
    buffer: BytesMut,
    chunk_count: u64,
    chunk_size: u64,
    chunk: u64,
    upload_id: String,
    upload_url: String,
    sha1: Option<String>,
    task_id: String,
    temp_file_path: String,
    is_finished: bool,
    bucket: String,
    obj_key: String,
    mime_type: String,
    auth_info: String,
    callback: Option<Callback>,
    is_uploading: bool,
    flush_count: u32,
    /// §2.2 Write Folding generation counter — monotonically increasing per
    /// dav path; `flush()` checks after acquiring the write lock and skips
    /// the upload when a newer generation has arrived while it waited.
    generation: u64,
}

impl Default for UploadState {
    fn default() -> Self {
        Self {
            size: 0,
            buffer: BytesMut::new(),
            chunk_count: 0,
            chunk_size: 0,
            chunk: 1,
            upload_id: String::new(),
            upload_url: "".to_string(),
            sha1: None,
            task_id: "".to_string(),
            temp_file_path: "".to_string(),
            is_finished: false,
            bucket: "".to_string(),
            obj_key: "".to_string(),
            mime_type: "application/octet-stream".to_string(),
            auth_info: "".to_string(),
            callback: None,
            is_uploading: false,
            flush_count: 0,
            generation: 0,
        }
    }
}

struct QuarkDavFile {
    fs: QuarkDriveFileSystem,
    file: QuarkFile,
    parent_file_id: String,
    parent_dir: PathBuf,
    current_pos: u64,
    upload_state: UploadState,
    http_download: bool,
    md5_ctx: Md5Context,
    sha1_ctx: Sha1,
}

impl Debug for QuarkDavFile {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuarkDavFile")
            .field("file", &self.file)
            .field("parent_file_id", &self.parent_file_id)
            .field("current_pos", &self.current_pos)
            .field("upload_state", &self.upload_state)
            .finish()
    }
}

impl QuarkDavFile {

    fn new(
        fs: QuarkDriveFileSystem,
        file: QuarkFile,
        parent_file_id: String,
        parent_dir: PathBuf,
        size: u64,
        sha1: Option<String>,
    ) -> Self {
        Self {
            fs,
            file,
            parent_file_id,
            parent_dir,
            current_pos: 0,
            upload_state: UploadState {
                size,
                sha1,
                ..Default::default()
            },
            http_download: false,
            md5_ctx: Md5Context::new(),
            sha1_ctx: Sha1::default(),
        }
    }

    async fn prepare_for_upload(&mut self) -> Result<bool, FsError> {
        if self.upload_state.is_finished {
            return Ok(false);
        }
        if !self.upload_state.is_uploading {
            self.upload_state.is_uploading = true;
            // §2.2 Write Folding: bump per-path generation counter and record
            // our generation so flush() can detect if a newer PUT arrived while
            // we waited on the write lock.
            let dav_path = self.parent_dir.join(&self.file.file_name);
            let upload_gen = self.fs.upload_generation
                .entry(dav_path)
                .and_modify(|g| *g += 1)
                .or_insert(1);
            self.upload_state.generation = *upload_gen;
            // Combine wall-clock ms with a filesystem-wide atomic counter so
            // two concurrent PUTs that arrive in the same millisecond cannot
            // produce the same temp path. AtomicU64::fetch_add is the only
            // ordering that survives across await cancellations.
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let seq = self.fs.temp_seq.fetch_add(1, Ordering::Relaxed);
            self.upload_state.temp_file_path = format!(
                "/tmp/{}_{}_{}",
                timestamp, seq, self.file.file_name
            );
        }
        Ok(true)
    }

    async fn do_flush(&mut self) -> Result<(), FsError> {
        let size = self.upload_state.size;

        // Compute final SHA-1 and MD5 (all data has been written)
        let sha1 = format!("{:x}", self.sha1_ctx.clone().finalize());
        let md5 = format!("{:x}", self.md5_ctx.clone().compute());

        // If old file exists, compare hash before deleting
        if !self.file.fid.is_empty() {
            // Fetch the cloud file's MD5 via download API and compare
            match self.fs.drive.get_file_md5(&self.file.fid).await {
                Ok(Some(cloud_md5)) if cloud_md5.eq_ignore_ascii_case(&md5) => {
                    debug!(file_name = %self.file.file_name, md5 = %md5,
                           "skip uploading: content hash unchanged");
                    self.upload_state.is_finished = true;
                    self.after_flush().await?;
                    return Ok(());
                }
                Ok(_) => {
                    // MD5 differs or not available, proceed with upload
                }
                Err(err) => {
                    // Failed to get MD5, proceed with upload anyway
                    debug!(file_name = %self.file.file_name, error = %err,
                           "failed to get cloud file md5, proceeding with upload");
                }
            }
            if self.fs.skip_upload_same_size && self.file.size == size {
                debug!(file_name = %self.file.file_name, size = size,
                       "skip uploading: same size");
                self.upload_state.is_finished = true;
                self.after_flush().await?;
                return Ok(());
            }
            // Content is different, now delete old file before uploading
            if let Err(err) = self.fs.drive
                .remove_file(&self.file.fid, !self.fs.no_trash).await
            {
                error!(file_name = %self.file.file_name, error = %err,
                       "delete file before upload failed");
            }
        }

        // up_pre
        let res = self
            .fs
            .drive
            .up_pre(&self.file.file_name, size, &self.parent_file_id)
            .await
            .map_err(|err| {
                error!(file_name = %self.file.file_name, error = %err, "create file with proof failed");
                FsError::GeneralFailure
            })?;

        if res.data.finish {
            // 秒传
            self.upload_state.is_finished = true;
            self.after_flush().await?;
            self.fs.register_active_write(&self.file.parent_path.as_ref().unwrap(), &self.file.file_name, self.upload_state.size, &self.upload_state.temp_file_path).await;
            return Ok(());
        }
        self.upload_state.auth_info = res.data.auth_info;
        self.upload_state.callback = Some(res.data.callback.clone());
        self.upload_state.task_id = res.data.task_id.clone();
        self.upload_state.upload_url =
            res.data.upload_url
                .strip_prefix("https://")
                .or_else(|| res.data.upload_url.strip_prefix("http://"))
                .unwrap_or(&res.data.upload_url)
                .to_string();
        self.upload_state.bucket = res.data.bucket;
        self.upload_state.obj_key = res.data.obj_key;
        if res.data.format_type != "" {
            self.upload_state.mime_type = res.data.format_type;
        }

        self.file.fid = res.data.fid.clone();

        self.upload_state.chunk_size = res.metadata.part_size;
        let chunk_count =
            size / res.metadata.part_size + if size % res.metadata.part_size != 0 { 1 } else { 0 };
        self.upload_state.chunk_count = chunk_count;
        let Some(upload_id) = res.data.upload_id else {
            error!("create file with proof failed: missing upload_id");
            return Err(FsError::GeneralFailure);
        };
        self.upload_state.upload_id = upload_id;

        // up_hash (reuse already-computed md5 and sha1)
        let task_id = self.upload_state.task_id.clone();
        let res = self.fs.drive.up_hash(&md5, &sha1, &task_id).await.map_err(|err| {
            error!(file_id = %self.file.fid, file_name = %self.file.file_name, error = %err, "hash file failed");
            FsError::GeneralFailure
        })?;
        if res.data.finish {
            self.upload_state.is_finished = true;
            self.after_flush().await?;
            self.fs.register_active_write(&self.file.parent_path.as_ref().unwrap(), &self.file.file_name, self.upload_state.size, &self.upload_state.temp_file_path).await;
            return Ok(());
        }
        // Spawn upload task so it won't be cancelled if client disconnects.
        // We still await the result — if the client stays connected, it gets the real result.
        // If the client disconnects (e.g. timeout), the spawned task continues uploading.
        let drive = self.fs.drive.clone();
        let upload_state = self.upload_state.clone();
        let file_name = self.file.file_name.clone();
        let parent_path = self.file.parent_path.as_ref().unwrap().clone();
        let parent_dir = self.parent_dir.clone();
        let fs = self.fs.clone();

        let handle = tokio::spawn(async move {
            // upload chunks (concurrent, buffered(4))
            let chunk_size = upload_state.chunk_size as usize;
            let temp_path = upload_state.temp_file_path.clone();
            let chunk_count = upload_state.chunk_count as usize;
            let total_size = upload_state.size as usize;
            let mime_type = upload_state.mime_type.clone();
            let obj_key = upload_state.obj_key.clone();
            let bucket = upload_state.bucket.clone();
            let task_id = upload_state.task_id.clone();
            let upload_id_ref = &upload_state.upload_id;
            let upload_url = upload_state.upload_url.clone();
            let auth_info = upload_state.auth_info.clone();
            let file_name_inner = file_name.clone();
            let drive_inner = drive.clone();

            // Bug fix: Quark API metadata returns part_thread:1, requiring
            // SEQUENTIAL uploads only. The previous buffered(4) concurrent
            // implementation triggered `PartNotSequential` errors and 500s.
            use tokio::io::AsyncReadExt;
            let mut etags: Vec<String> = Vec::with_capacity(chunk_count);
            let mut early_finish = false;
            'upload_loop: for chunk_idx in 1u32..=chunk_count as u32 {
                let bytes_to_read = if chunk_idx as usize == chunk_count {
                    let remaining_bytes = total_size - ((chunk_idx as usize - 1) * chunk_size);
                    std::cmp::min(remaining_bytes, chunk_size)
                } else {
                    chunk_size
                };
                let offset = (chunk_idx as usize - 1) * chunk_size;
                let mut f = tokio::fs::File::open(&temp_path).await.map_err(|e| {
                    error!(file_name = %file_name_inner, error = %e, "open temp file failed");
                    FsError::GeneralFailure
                })?;
                f.seek(SeekFrom::Start(offset as u64)).await.map_err(|e| {
                    error!(file_name = %file_name_inner, error = %e, "seek temp file failed");
                    FsError::GeneralFailure
                })?;
                let mut buf = vec![0u8; bytes_to_read];
                f.read_exact(&mut buf).await.map_err(|e| {
                    error!(file_name = %file_name_inner, error = %e, "read temp file failed");
                    FsError::GeneralFailure
                })?;
                drop(f);
                let now: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
                let utc_time = now.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
                let auth_meta = drive_inner.up_part_auth_meta(&mime_type, &utc_time, &bucket, &obj_key, chunk_idx, &upload_state.upload_id).await.map_err(|err| {
                    error!(file_name = %file_name_inner, error = %err, "get upload part auth meta failed");
                    FsError::GeneralFailure
                })?;
                let auth_res = drive_inner.auth(&auth_info, &auth_meta, &task_id).await.map_err(|err| {
                    error!(file_name = %file_name_inner, error = %err, "auth upload part failed");
                    FsError::GeneralFailure
                })?;
                let up_req = UpPartMethodRequest {
                    auth_key: auth_res.data.auth_key,
                    mime_type: mime_type.clone(),
                    utc_time,
                    bucket: bucket.clone(),
                    upload_url: upload_url.clone(),
                    obj_key: obj_key.clone(),
                    part_number: chunk_idx,
                    upload_id: upload_state.upload_id.clone(),
                    part_bytes: buf,
                };
                let res = drive_inner.up_part(up_req).await.map_err(|err| {
                    error!(file_name = %file_name_inner, error = %err, "upload chunk failed");
                    FsError::GeneralFailure
                })?;
                let etag = res.ok_or_else(|| {
                    error!(file_name = %file_name_inner, "up_part returned None");
                    FsError::GeneralFailure
                })?;
                if etag == "finish" {
                    early_finish = true;
                    break 'upload_loop;
                }
                etags.push(etag);
            }
            if early_finish {
                fs.register_active_write(&parent_path, &file_name_inner, upload_state.size, &temp_path).await;
                if tokio::fs::metadata(&temp_path).await.is_ok() {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                }
                fs.remove_uploading_file(&parent_path, &file_name_inner);
                let full_path = parent_dir.join(&file_name_inner);
                fs.dir_cache.invalidate(full_path.as_path()).await;
                fs.dir_cache.invalidate_chunks(&full_path.to_string_lossy());
                fs.dir_cache.invalidate(parent_dir.as_path()).await;
                return Ok(());
            }

            // commit
            let callback = upload_state.callback.clone().ok_or_else(|| {
                error!(file_name = %file_name, "upload_state.callback is None");
                FsError::GeneralFailure
            })?;
            let commit_req = UpAuthAndCommitRequest {
                md5s: etags,
                callback,
                bucket: bucket.clone(),
                obj_key: obj_key.clone(),
                upload_id: upload_id_ref.to_string(),
                auth_info: upload_state.auth_info.clone(),
                task_id: task_id.clone(),
                upload_url: upload_url.clone(),
            };
            drive.up_auth_and_commit(commit_req).await.map_err(|err| {
                error!(file_name = %file_name, error = %err, "commit upload failed");
                FsError::GeneralFailure
            })?;
            drive.finish(&obj_key, &task_id).await.map_err(|err| {
                error!(file_name = %file_name, error = %err, "finish upload failed");
                FsError::GeneralFailure
            })?;

            fs.register_active_write(&parent_path, &file_name, upload_state.size, &temp_path).await;
            // cleanup
            if tokio::fs::metadata(&temp_path).await.is_ok() {
                let _ = tokio::fs::remove_file(&temp_path).await;
            }
            fs.remove_uploading_file(&parent_path, &file_name);
            let full_path = parent_dir.join(&file_name);
            fs.dir_cache.invalidate(full_path.as_path()).await;
            fs.dir_cache.invalidate_chunks(&full_path.to_string_lossy());
            fs.dir_cache.invalidate(parent_dir.as_path()).await;

            Ok::<(), FsError>(())
        });

        // Wait for upload to complete, but return early if upload_wait_timeout is reached
        // to avoid client timeout. The spawned task continues uploading in the background.
        let upload_wait_timeout = self.fs.upload_wait_timeout;
        if upload_wait_timeout > 0 {
            match tokio::time::timeout(
                std::time::Duration::from_secs(upload_wait_timeout),
                handle,
            ).await {
                Ok(result) => {
                    // Upload finished within timeout, return real result
                    result.map_err(|err| {
                        error!(file_name = %self.file.file_name, error = %err, "upload task join failed");
                        FsError::GeneralFailure
                    })??;
                }
                Err(_) => {
                    // Timeout reached, upload continues in background
                    info!(file_name = %self.file.file_name, timeout_secs = upload_wait_timeout,
                          "upload still in progress, returning early to avoid client timeout");
                }
            }
        } else {
            // Wait indefinitely
            handle.await.map_err(|err| {
                error!(file_name = %self.file.file_name, error = %err, "upload task join failed");
                FsError::GeneralFailure
            })??;
        }

        self.upload_state = UploadState::default();
        Ok(())
    }


    async fn upload_mini_byte_file(&mut self) -> Result<(), FsError> {
        // Empty file MD5
        let empty_md5 = "d41d8cd98f00b204e9800998ecf8427e";

        // If old file exists, compare hash before deleting
        if !self.file.fid.is_empty() {
            match self.fs.drive.get_file_md5(&self.file.fid).await {
                Ok(Some(cloud_md5)) if cloud_md5.eq_ignore_ascii_case(empty_md5) => {
                    debug!(file_name = %self.file.file_name,
                           "skip uploading: empty file content hash unchanged");
                    self.upload_state.is_finished = true;
                    self.after_flush().await?;
                    return Ok(());
                }
                Ok(_) => {}
                Err(err) => {
                    debug!(file_name = %self.file.file_name, error = %err,
                           "failed to get cloud file md5, proceeding with upload");
                }
            }
            // Content is different, now delete old file before uploading
            if let Err(err) = self.fs.drive
                .remove_file(&self.file.fid, !self.fs.no_trash).await
            {
                error!(file_name = %self.file.file_name, error = %err,
                       "delete file before upload failed");
            }
        }

        // pre -> hash -> commit -> finish
        // up_pre
        let res = self
            .fs
            .drive
            .up_pre(&self.file.file_name, 0, &self.parent_file_id)
            .await
            .map_err(|err| {
                error!(file_name = %self.file.file_name, error = %err, "create file with proof failed");
                FsError::GeneralFailure
            })?;

        if res.data.finish {
            // 秒传
            self.upload_state.is_finished = true;
            self.after_flush().await?;
            self.fs.register_active_write(&self.file.parent_path.as_ref().unwrap(), &self.file.file_name, self.upload_state.size, &self.upload_state.temp_file_path).await;
            return Ok(());
        }
        self.upload_state.auth_info = res.data.auth_info;
        self.upload_state.callback = Some(res.data.callback.clone());
        self.upload_state.task_id = res.data.task_id.clone();
        self.upload_state.upload_url =
            res.data.upload_url
                .strip_prefix("https://")
                .or_else(|| res.data.upload_url.strip_prefix("http://"))
                .unwrap_or(&res.data.upload_url)
                .to_string();
        self.upload_state.bucket = res.data.bucket;
        self.upload_state.obj_key = res.data.obj_key;
        if res.data.format_type != "" {
            self.upload_state.mime_type = res.data.format_type;
        }

        self.file.fid = res.data.fid.clone();

        self.upload_state.chunk_size = 0;
        let chunk_count = 1 ;
        self.upload_state.chunk_count = chunk_count;
        let Some(upload_id) = res.data.upload_id else {
            error!("create file with proof failed: missing upload_id");
            return Err(FsError::GeneralFailure);
        };
        self.upload_state.upload_id = upload_id;

        // unHash
        let md5 = "d41d8cd98f00b204e9800998ecf8427e";
        let sha1 = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
        let task_id = self.upload_state.task_id.clone();
        let res = self.fs.drive.up_hash(&md5, &sha1, &task_id).await.map_err(|err| {
            error!(file_id = %self.file.fid, file_name = %self.file.file_name, error = %err, "hash file failed");
            FsError::GeneralFailure
        })?;
        if res.data.finish {
            self.upload_state.is_finished = true;
            self.after_flush().await?;
            return Ok(());
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let seq = self.fs.temp_seq.fetch_add(1, Ordering::Relaxed);
        self.upload_state.temp_file_path = format!(
            "./temp/{}_{}_{}",
            timestamp, seq, self.file.file_name
        );

        // 创建一个空白文件txt
        let empty_file_content = b"";
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&self.upload_state.temp_file_path)
            .await
            .map_err(|e| {
                error!(file_name = %self.file.file_name, error = %e, "failed to create temp file");
                FsError::GeneralFailure
            })?;
        file.write_all(empty_file_content).await.map_err(|e| {
            error!(file_name = %self.file.file_name, error = %e, "write to temp file failed");
            FsError::GeneralFailure
        })?;
        // Bug fix: actual upload runs in flush() -> do_flush() after all body
        // bytes are written. Calling upload_chunk() here ran on an empty file
        // and produced duplicate uploads.

        Ok(())
    }


    async fn consume_buf(&mut self) -> Result<(), FsError> {
        let temp_path = self.upload_state.temp_file_path.clone();
        let mut md5_ctx = self.md5_ctx.clone();
        let mut sha1_ctx = self.sha1_ctx.clone();
        let bytes = self.upload_state.buffer.split().freeze().to_vec();
        // 写入临时文件
        self.upload_state.size = self.upload_state.size + bytes.len() as u64;
        if let Some(parent) = std::path::Path::new(&temp_path).parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                error!("create_dir_all failed: {}, path: {:?}", e, parent);
            }
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open(&temp_path)
            .await
            .map_err(|e| {
                error!("failed to open file: {}, {}", temp_path, e);
                FsError::GeneralFailure
            })?;
        file.write_all(&bytes).await.map_err(|e| {
            error!(file_name = %self.file.file_name, error = %e, "write to temp file failed");
            FsError::GeneralFailure
        })?;
        // 更新哈希
        md5_ctx.consume(&bytes);
        sha1_ctx.update(&bytes);
        // 保存回结构体
        self.md5_ctx = md5_ctx;
        self.sha1_ctx = sha1_ctx;
        Ok(())
    }

    async fn upload_chunk(&mut self) -> Result<(), FsError> {

        let chunk_size = self.upload_state.chunk_size as usize;
        let temp_path = &self.upload_state.temp_file_path;
        let file = File::open(temp_path).await.map_err(|err| {
            error!(file_name = %self.file.file_name, error = %err, "open temp file failed");
            FsError::GeneralFailure
        })?;
        let mut file = tokio::io::BufReader::new(file);
        let chunk_count = self.upload_state.chunk_count;
        // 定义一个字符串数组，size = chunk_count
        let mut etags = vec![String::new(); chunk_count as usize];
        // 分块上传文件,将temp_path目录所在文件,切成chunk_count块，每块大小 chunk_size，分块上传文件到夸克网盘
        // auth
        let mime_type = &self.upload_state.mime_type;
        let obj_key = &self.upload_state.obj_key;
        let bucket = &self.upload_state.bucket;
        let task_id = &self.upload_state.task_id;
        let upload_id = &self.upload_state.upload_id;
        let upload_url = &self.upload_state.upload_url;

        for chunk_idx in 1..= chunk_count {

            let bytes_to_read = if chunk_idx == chunk_count {
                // 最后一块可能小于 chunk_size
                let remaining_bytes = self.upload_state.size as usize - ((chunk_idx - 1) as usize * chunk_size);
                std::cmp::min(remaining_bytes, chunk_size)
            } else {
                chunk_size
            };
            let mut buf = vec![0u8; bytes_to_read]; // 创建指定大小的缓冲区
            file.read_exact(&mut buf).await.map_err(|e| {
                error!(file_name = %self.file.file_name, error = %e, "read temp file failed");
                FsError::GeneralFailure
            })?;
            let now: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
            // RFC1123 格式
            let utc_time = now.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
            let auth_meta = self.fs.drive.up_part_auth_meta(mime_type, &utc_time, bucket, obj_key, chunk_idx as u32, upload_id).await.map_err(|err| {
                error!(file_name = %self.file.file_name, error = %err, "get upload part auth meta failed");
                FsError::GeneralFailure
            })?;
            let auth_info = &self.upload_state.auth_info;

            let auth_res = self.fs.drive.auth(auth_info, &auth_meta, task_id).await.map_err(|err| {
                error!(file_name = %self.file.file_name, error = %err, "auth upload part failed");
                FsError::GeneralFailure
            })?;


            let auth_key = auth_res.data.auth_key;

            let up_req = UpPartMethodRequest {
                auth_key: auth_key.clone(),
                mime_type: self.upload_state.mime_type.clone(),
                utc_time: utc_time.clone(),
                bucket: bucket.clone(),
                upload_url: upload_url.clone(),
                obj_key: obj_key.clone(),
                part_number: chunk_idx as u32,
                upload_id: upload_id.to_string(),
                part_bytes: buf,
            };

            let res = self.fs.drive.up_part(up_req).await.map_err(|err| {
                error!(file_name = %self.file.file_name, error = %err, "upload chunk failed");
                FsError::GeneralFailure
            })?;
            let etag_from_up_part = res.ok_or_else(|| {
                error!(file_name = %self.file.file_name, "up_part returned None");
                FsError::GeneralFailure
            })?;
            // 检查是否提前完成
            if etag_from_up_part == "finish" {
                return Ok(());
            }
            etags[(chunk_idx - 1) as usize] = etag_from_up_part;
            // self.upload_state.chunk += 1;
        }
        let callback = self.upload_state.callback.clone().ok_or_else(|| {
            error!(file_name = %self.file.file_name, "upload_state.callback is None");
            FsError::GeneralFailure
        })?;

        let auth_info = &self.upload_state.auth_info;
        let commit_req = UpAuthAndCommitRequest{
            md5s: etags.clone(),
            callback: callback,
            bucket: bucket.clone(),
            obj_key: obj_key.clone(),
            upload_id: upload_id.to_string(),
            auth_info: auth_info.clone(),
            task_id: task_id.clone(),
            upload_url: upload_url.clone(),
        };
        // commit
        self.fs.drive.up_auth_and_commit(commit_req).await.map_err(|err| {
            error!(file_name = %self.file.file_name, error = %err, "commit upload failed");
            FsError::GeneralFailure
        })?;
        // finish upload
        self.fs.drive.finish(&obj_key, &task_id).await.map_err(|err| {
            error!(file_name = %self.file.file_name, error = %err, "finish upload failed");
            FsError::GeneralFailure
        })?;

        Ok(())
    }

    async fn delete_temp_file(&self) -> Result<(), FsError> {
        let temp_path = &self.upload_state.temp_file_path;
        if tokio::fs::metadata(&temp_path).await.is_ok() {
            if let Err(err) = tokio::fs::remove_file(&temp_path).await {
                error!(file_id = %self.file.fid, file_name = %self.file.file_name, error = %err, "remove temp file failed");
            }
        }
        Ok(())
    }

    async fn after_flush(&mut self) -> Result<(), FsError> {
        self.delete_temp_file().await?;
        let parent_path = self.file.parent_path.as_ref().unwrap().as_str();
        self.fs.remove_uploading_file(parent_path, &self.file.file_name);
        self.upload_state = UploadState::default();
        let full_path = self.parent_dir.join(&self.file.file_name);
        self.fs.dir_cache.invalidate(full_path.as_path()).await;
        self.fs.dir_cache.invalidate_chunks(&full_path.to_string_lossy());
        self.fs.dir_cache.invalidate(self.parent_dir.as_path()).await;
        Ok(())
    }

    async fn get_download_url(&self) -> Result<String, FsError> {
        self.fs.drive.get_download_url(&self.file.fid).await.map_err(|err| {
            error!(file_id = %self.file.fid, file_name = %self.file.file_name, error = %err, "get download url failed");
            FsError::GeneralFailure
        })
    }

}

impl DavFile for QuarkDavFile {
    fn metadata(&'_ mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: metadata");
        async move {
            let file = self.file.clone();
            Ok(Box::new(file) as Box<dyn DavMetaData>)
        }
            .boxed()
    }

    fn redirect_url(&mut self) -> FsFuture<Option<String>> {
        debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: redirect_url");
        async move {
            // 修复：禁用直连 CDN 的 302 redirect。
            // webdavfs_agent 拿到 redirect URL 后会直连 Quark CDN，绕过我们的代理，
            // 没有任何超时控制。大文件传输途中 CDN URL 过期或 CDN 抖动时，
            // Finder 会无限 hang 住（卡死表现）。
            // 返回 None 强制 webdavfs 走 read_bytes() 路径，由我们的 proxy 管控，
            // 每个 chunk 都有 30s 超时保护。
            Ok(None)
        }
            .boxed()
    }



    fn seek(&mut self, pos: SeekFrom) -> FsFuture<u64> {
        debug!(
            file_id = %self.file.fid,
            file_name = %self.file.file_name,
            pos = ?pos,
            "file: seek"
        );
        async move {
            let new_pos = match pos {
                SeekFrom::Start(pos) => pos,
                SeekFrom::End(pos) => (self.file.size as i64 + pos) as u64,
                SeekFrom::Current(size) => self.current_pos + size as u64,
            };
            self.current_pos = new_pos;
            Ok(new_pos)
        }
            .boxed()
    }

    /// write file : open -> metadata -> flush -> write_buf/write_byte -> flush
    fn write_buf(&mut self, buf: Box<dyn bytes::Buf + Send>) -> FsFuture<()>{
        debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: write_buf");
        async move {
            if self.prepare_for_upload().await? {
                self.upload_state.buffer.put(buf);
                self.consume_buf().await?;
            }
            Ok(())
        }
            .boxed()
    }


    fn write_bytes(&mut self, buf: bytes::Bytes) -> FsFuture<()> {
        let buf: Box<dyn Buf + Send> = Box::new(buf);
        self.write_buf(buf)
    }

    fn read_bytes(&mut self, count: usize) -> FsFuture<Bytes> {
        debug!(
            file_id = %self.file.fid,
            file_name = %self.file.file_name,
            pos = self.current_pos,
            count = count,
            size = self.file.size,
            "file: read_bytes",
        );
        async move {
            if self.file.fid.is_empty() {
                // 1. 尝试自愈：如果在 active_writes 里有记录，返回内存缓存 data
                let full_path = self.parent_dir.join(&self.file.file_name);
                let path_str = full_path.to_string_lossy().to_string();
                if let Some(info) = self.fs.active_writes.get(&path_str) {
                    if info.created_at.elapsed().as_secs() < 45 {
                        let start = self.current_pos as usize;
                        if start >= info.body.len() {
                            return Ok(Bytes::new());
                        }
                        let end = std::cmp::min(start + count, info.body.len());
                        let bytes = Bytes::copy_from_slice(&info.body[start..end]);
                        self.current_pos = end as u64;
                        return Ok(bytes);
                    }
                }

                // 2. 嗅探占位防御：正在上传且大小为0
                if self.file.size == 0 {
                    return Ok(Bytes::new());
                }

                // upload in progress
                return Err(FsError::NotFound);
            }

            let read_start = self.current_pos;
            let read_len = count as u64;
            let file_size = self.file.size;
            if file_size == 0 {
                return Ok(Bytes::new());
            }
            if read_start >= file_size {
                return Ok(Bytes::new());
            }
            let read_end = std::cmp::min(read_start + read_len, file_size);
            if read_start >= read_end {
                return Ok(Bytes::new());
            }

            let chunk_cache_path = self.parent_dir
                .join(&self.file.file_name)
                .to_string_lossy()
                .to_string();

            const CHUNK_SIZE: u64 = 4 * 1024 * 1024; // 4MB
            let start_chunk_idx = read_start / CHUNK_SIZE;
            let end_chunk_idx = (read_end - 1) / CHUNK_SIZE;

            let mut result_buf = vec![0u8; (read_end - read_start) as usize];
            const CHUNK_DOWNLOAD_TIMEOUT_SECS: u64 = 300;

            for chunk_idx in start_chunk_idx..=end_chunk_idx {
                let chunk_align_start = chunk_idx * CHUNK_SIZE;
                let chunk_len = if chunk_align_start + CHUNK_SIZE > file_size {
                    file_size - chunk_align_start
                } else {
                    CHUNK_SIZE
                } as usize;
                let chunk_align_end = chunk_align_start + chunk_len as u64;

                // 1. 获取该分块的锁，保证同一时间只有一个线程在下载该文件的这个 chunk
                let chunk_lock = self.fs.chunk_lock_for(chunk_cache_path.clone(), chunk_align_start);
                let _guard = chunk_lock.lock().await;

                // 2. 尝试从磁盘缓存中读取
                let mut data = None;
                if let Some(cached) = self.fs.dir_cache.read_chunk(&chunk_cache_path, chunk_align_start, chunk_len).await {
                    debug!(
                        path = %chunk_cache_path, pos = chunk_align_start, count = chunk_len,
                        "chunk cache: hit, skipping network download"
                    );
                    data = Some(cached);
                } else {
                    // Cache miss: 从网络下载
                    let mut retries = 0;
                    let max_retries = 2;
                    while retries < max_retries {
                        let is_valid = self.file.download_url.as_ref()
                            .map(|url| !is_url_expired(url))
                            .unwrap_or(false);

                        if !is_valid {
                            let new_url = match tokio::time::timeout(
                                std::time::Duration::from_secs(8),
                                self.get_download_url(),
                            ).await {
                                Ok(Ok(url)) => url,
                                Ok(Err(e)) => {
                                    error!(file_name = %self.file.file_name, error = %e, "get_download_url failed");
                                    self.file.download_url = None;
                                    break;
                                }
                                Err(_) => {
                                    error!(file_name = %self.file.file_name, "get_download_url timeout 8s");
                                    self.file.download_url = None;
                                    break;
                                }
                            };
                            self.file.download_url = Some(new_url);
                        }

                        let download_url = match self.file.download_url.as_ref() {
                            Some(url) => url,
                            None => {
                                error!(file_name = %self.file.file_name, "download_url is None");
                                break;
                            }
                        };

                        if download_url.is_empty() {
                            error!(file_name = %self.file.file_name, "download_url is empty");
                            break;
                        }

                        let download_result = tokio::time::timeout(
                            std::time::Duration::from_secs(CHUNK_DOWNLOAD_TIMEOUT_SECS),
                            self.fs.drive.download(download_url.clone(), Some((chunk_align_start, chunk_len))),
                        ).await;

                        match download_result {
                            Ok(Ok(downloaded_data)) => {
                                if downloaded_data.len() == chunk_len {
                                    // 异步写入磁盘缓存
                                    self.fs.dir_cache.write_chunk(
                                        chunk_cache_path.clone(),
                                        chunk_align_start,
                                        downloaded_data.to_vec(),
                                    );
                                    data = Some(downloaded_data);
                                    break;
                                } else {
                                    error!(
                                        file_name = %self.file.file_name,
                                        expected = chunk_len,
                                        actual = downloaded_data.len(),
                                        "download chunk size mismatch, retrying..."
                                    );
                                    self.file.download_url = None;
                                    retries += 1;
                                }
                            }
                            Ok(Err(err)) => {
                                error!(file_name = %self.file.file_name, error = %err, "download chunk failed, resetting URL to retry...");
                                self.file.download_url = None;
                                retries += 1;
                            }
                            Err(_) => {
                                error!(file_name = %self.file.file_name, pos = chunk_align_start, count = chunk_len,
                                    "download chunk timed out after {}s, resetting URL to retry...", CHUNK_DOWNLOAD_TIMEOUT_SECS);
                                self.file.download_url = None;
                                retries += 1;
                            }
                        }
                    }
                }

                // 3. 提取我们需要的部分并复制到 result_buf
                if let Some(chunk_data) = data {
                    let overlap_start = std::cmp::max(read_start, chunk_align_start);
                    let overlap_end = std::cmp::min(read_end, chunk_align_end);
                    if overlap_start < overlap_end {
                        let chunk_offset = (overlap_start - chunk_align_start) as usize;
                        let chunk_end_offset = (overlap_end - chunk_align_start) as usize;
                        let result_offset = (overlap_start - read_start) as usize;
                        let result_end_offset = (overlap_end - read_start) as usize;
                        
                        result_buf[result_offset..result_end_offset].copy_from_slice(&chunk_data[chunk_offset..chunk_end_offset]);
                    }
                } else {
                    return Err(FsError::GeneralFailure);
                }
            }

            self.current_pos = read_end;
            Ok(Bytes::from(result_buf))
        }
            .boxed()
    }

    fn flush(&mut self) -> FsFuture<()> {
        debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: flush");
        // Compute the full dav path *before* moving into the async block, so
        // we don't have to borrow `self.parent_dir`/`self.file.file_name`
        // across an await while the same `&mut self` is already mutably
        // borrowed.
        let full_path = self.parent_dir.join(&self.file.file_name);
        let full_path_for_gen = full_path.clone();  // §2.2: clone before write_lock_for consumes it
        let write_lock = self.fs.write_lock_for(full_path);
        let fs_for_errpath = self.fs.clone();
        async move {
            // if self.upload_state.flush_count >=1 {
            //     // maybe zero byte file, try to upload again
            //     // TODO :
            //     // How to judge if a file is zero byte?
            //     // now it is not working
            //     // self.upload_mini_byte_file().await?;
            //     // return Ok(());
            // }

            if !self.upload_state.is_uploading {
                debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: flush - no temp file path");
                self.upload_state.flush_count = self.upload_state.flush_count + 1;
                return Ok(());
            }

            if self.upload_state.is_finished {
                debug!(file_id = %self.file.fid, file_name = %self.file.file_name, "file: flush - already finished");
                return Ok(());
            }

            // Serialize concurrent same-path PUTs end-to-end (agent.md §2.1
            // "路径独占重入写锁"). The lock is keyed by the full dav path
            // so unrelated files don't queue on the same mutex.
            let _guard = write_lock.lock().await;

            // §2.2 Write Folding: if a newer PUT for the same path has been
            // queued while we waited, our upload is obsolete — skip the
            // network round-trip and just clean up the local temp file.
            if let Some(current_gen) = self.fs.upload_generation.get(&full_path_for_gen) {
                if *current_gen > self.upload_state.generation {
                    debug!(
                        generation = self.upload_state.generation,
                        current_generation = *current_gen,
                        file_name = %self.file.file_name,
                        "§2.2 write folding: obsolete upload, skipping"
                    );
                    let _ = self.delete_temp_file().await;
                    // Remove from uploading list so it doesn't stale the directory listing
                    let parent_path = self.file.parent_path.as_ref()
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    self.fs.remove_uploading_file(parent_path, &self.file.file_name);
                    self.upload_state.is_finished = true;
                    drop(_guard);
                    return Ok(());
                }
            }

            let res = self.do_flush().await;
            drop(_guard);
            if let Err(err) = res {
                error!(file_id = %self.file.fid, file_name = %self.file.file_name, error = %err, "file: flush failed");
                self.after_flush().await?;
                let _ = fs_for_errpath;
                return Err(err);
            }
            Ok(())
        }.boxed()

    }
}



fn is_url_expired(url: &str) -> bool {
    if let Ok(oss_url) = ::url::Url::parse(url) {
        let expires = oss_url.query_pairs().find_map(|(k, v)| {
            if k == "Expires" {
                if let Ok(expires) = v.parse::<u64>() {
                    return Some(expires);
                }
            }
            None
        });
        if let Some(expires) = expires {
            let current_ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_secs();
            // 修复：预留 5 分钟，大文件下载途中有足够时间刷新 URL，
            // 避免传输中途 URL 过期导致 CDN 拒绝请求（Finder 卡死根因之一）。
            return current_ts + 300 >= expires;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_url_expired_with_past_timestamp() {
        // Expires=0 is definitely in the past
        let url = "https://example.com/file?Expires=0";
        assert!(is_url_expired(url));
    }

    #[test]
    fn test_is_url_expired_with_future_timestamp() {
        // Use a timestamp far in the future (year ~2100)
        let url = "https://example.com/file?Expires=4102444800";
        assert!(!is_url_expired(url));
    }

    #[test]
    fn test_is_url_expired_no_expires_param() {
        let url = "https://example.com/file?key=value";
        // No Expires param => not expired (returns false)
        assert!(!is_url_expired(url));
    }

    #[test]
    fn test_is_url_expired_invalid_url() {
        let url = "not a valid url";
        // Invalid URL => not expired (returns false)
        assert!(!is_url_expired(url));
    }

    #[test]
    fn test_is_url_expired_within_300s_buffer() {
        // Get current time + 150 seconds (within the 300s buffer)
        let expires = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() + 150;
        let url = format!("https://example.com/file?Expires={}", expires);
        // Should be considered expired (within 300s buffer)
        assert!(is_url_expired(&url));
    }

    #[test]
    fn test_is_url_expired_beyond_300s_buffer() {
        // Get current time + 400 seconds (beyond the 300s buffer)
        let expires = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() + 400;
        let url = format!("https://example.com/file?Expires={}", expires);
        assert!(!is_url_expired(&url));
    }

    #[test]
    fn test_is_url_expired_empty_string() {
        assert!(!is_url_expired(""));
    }

    #[test]
    fn test_is_url_expired_with_multiple_params() {
        // URL with multiple params, Expires in the middle
        let url = "https://example.com/file?OSSAccessKeyId=xxx&Expires=0&Signature=yyy";
        assert!(is_url_expired(url));
    }

    #[test]
    fn test_is_url_expired_exactly_at_boundary() {
        // Get current time + exactly 300 seconds (at boundary)
        let expires = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() + 300;
        let url = format!("https://example.com/file?Expires={}", expires);
        // current_ts + 300 >= expires → should be expired at boundary
        assert!(is_url_expired(&url));
    }

    #[test]
    fn test_is_url_expired_non_numeric_expires() {
        let url = "https://example.com/file?Expires=not_a_number";
        // Non-numeric Expires should not cause a panic, returns false
        assert!(!is_url_expired(url));
    }
}
