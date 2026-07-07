//! Chromium 浏览器 cookie 抓取。
//!
//! 严格对齐 docs/rust-migration.md §4.1:
//!   1. `security find-generic-password` 拿 Chrome Safe Storage 明文
//!   2. PBKDF2-HMAC-SHA1(password, "saltysalt", 1003, 16) 派生 AES key
//!   3. 拷 Cookies 到 tempfile,绕开锁库
//!   4. SQLite 读列,过滤 pan/drive/quark.cn
//!   5. meta.version >= 24:剥前 32 字节 SHA256(host_key)
//!   6. v10/v11 头,AES-128-CBC,IV = " " * 16
//!   7. PKCS#7 去 padding;UTF-8 失败整条丢弃
//!
//! 仅 macOS(security-framework 限定)。其他平台直接返回空 store。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use anyhow::{anyhow, bail, Context, Result};
use dashmap::DashMap;
use pbkdf2::pbkdf2_hmac;
use rusqlite::Connection;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use tracing::{debug, info, warn};

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

/// 浏览器类型按优先级排列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserKind {
    Chrome,
    Brave,
    Edge,
    Arc,
    Chromium,
}

impl BrowserKind {
    /// (display name, keychain service, "Cookies" 在 Application Support 下的子目录)
    fn parts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            BrowserKind::Chrome => ("Chrome", "Chrome Safe Storage", "Google/Chrome"),
            BrowserKind::Brave => ("Brave-Browser", "Brave Safe Storage", "BraveSoftware/Brave-Browser"),
            BrowserKind::Edge => ("Microsoft Edge", "Microsoft Edge Safe Storage", "Microsoft Edge"),
            BrowserKind::Arc => ("Arc", "Arc Safe Storage", "Arc"),
            BrowserKind::Chromium => ("Chromium", "Chromium Safe Storage", "Chromium"),
        }
    }

    pub fn as_str(self) -> &'static str {
        self.parts().0
    }

    fn keychain_service(self) -> &'static str {
        self.parts().1
    }

    fn app_subdir(self) -> &'static str {
        self.parts().2
    }
}

pub const DEFAULT_BROWSERS: &[BrowserKind] = &[
    BrowserKind::Chrome,
    BrowserKind::Brave,
    BrowserKind::Edge,
    BrowserKind::Arc,
    BrowserKind::Chromium,
];

/// 内存 cookie 存储,线程安全。
#[derive(Clone, Default)]
pub struct CookieStore {
    inner: Arc<DashMap<String, String>>,
}

impl CookieStore {
    /// 按优先级顺序尝试;找到第一个有命中 cookie 的浏览器并填充。
    pub async fn from_chromium(priority: &[BrowserKind]) -> Result<Self> {
        let store = Self::default();
        for &browser in priority {
            match load_from_browser(browser).await {
                Ok(map) if !map.is_empty() => {
                    info!(browser = browser.as_str(), n = map.len(), "loaded cookies");
                    for (k, v) in map {
                        store.inner.insert(k, v);
                    }
                    return Ok(store);
                }
                Ok(_) => debug!(browser = browser.as_str(), "no matching cookies, trying next"),
                Err(e) => warn!(browser = browser.as_str(), error = %e, "browser load failed"),
            }
        }
        bail!("no browser yielded pan.quark.cn cookies; please open Chrome once to unlock keychain")
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.inner.get(key).map(|v| v.value().clone())
    }

    pub fn snapshot(&self) -> HashMap<String, String> {
        self.inner.iter().map(|e| (e.key().clone(), e.value().clone())).collect()
    }

    /// 用 snapshot 覆盖现有内容(main loop 定时刷新用)。
    pub fn replace(&self, map: HashMap<String, String>) {
        self.inner.clear();
        for (k, v) in map {
            self.inner.insert(k, v);
        }
    }
}

// ---------------------------------------------------------------------------
// 单浏览器加载流程
// ---------------------------------------------------------------------------

async fn load_from_browser(browser: BrowserKind) -> Result<HashMap<String, String>> {
    let password = keychain_password(browser)?;
    let key = derive_key(&password);

    let cookies_path = locate_cookies_db(browser)
        .ok_or_else(|| anyhow!("Cookies db not found for {}", browser.as_str()))?;
    let CookieRows { rows, .. } = read_and_decrypt(&cookies_path, &key).await?;
    Ok(filter_quark_cookies(rows))
}

fn keychain_password(browser: BrowserKind) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        use security_framework::passwords::get_generic_password;
        let service = browser.keychain_service();
        let account = browser.as_str();
        let bytes = get_generic_password(service, account)
            .with_context(|| format!("keychain get {service}/{account}"))?;
        Ok(String::from_utf8(bytes.to_vec())?)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = browser;
        bail!("cookie extraction is macOS-only in this build")
    }
}

fn derive_key(password: &str) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2_hmac::<Sha1>(password.as_bytes(), b"saltysalt", 1003, &mut key);
    key
}

fn locate_cookies_db(browser: BrowserKind) -> Option<PathBuf> {
    let home = dirs_home()?;
    let base: PathBuf = [
        home.as_path(),
        Path::new("Library/Application Support"),
        Path::new(browser.app_subdir()),
    ]
    .iter()
    .collect();

    // Default + Profile *; 取第一个存在的 Cookies
    let candidates = ["Default/Cookies", "Profile 1/Cookies", "Profile 2/Cookies"];
    for rel in candidates {
        let p = base.join(rel);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

struct CookieRows {
    rows: Vec<(String, String, Vec<u8>)>,
    meta_version: i64,
}

async fn read_and_decrypt(cookies: &Path, key: &[u8; 16]) -> Result<CookieRows> {
    // 拷到 tempfile 避免锁库
    let tmp = tempdir()?;
    let tmp_db = tmp.path().join("Cookies");
    tokio::fs::copy(cookies, &tmp_db).await
        .with_context(|| format!("copy {} to {}", cookies.display(), tmp_db.display()))?;

    // rusqlite 是同步的;放 spawn_blocking
    let tmp_db_clone = tmp_db.clone();
    let parsed = tokio::task::spawn_blocking(move || -> Result<CookieRows> {
        let conn = Connection::open(&tmp_db_clone)
            .with_context(|| format!("open sqlite {}", tmp_db_clone.display()))?;

        let mut stmt = conn.prepare(
            "SELECT host_key, name, encrypted_value FROM cookies",
        )?;
        let mut rows = Vec::new();
        let mapped = stmt.query_map([], |row| {
            let host: String = row.get(0)?;
            let name: String = row.get(1)?;
            let value: Vec<u8> = row.get(2)?;
            Ok((host, name, value))
        })?;
        for r in mapped {
            let (host, name, value) = r?;
            rows.push((host, name, value));
        }

        let meta_version: i64 = conn
            .query_row("SELECT value FROM meta WHERE key='version'", [], |row| {
                row.get::<_, String>(0)
            })
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        Ok(CookieRows { rows, meta_version })
    })
    .await??;

    let iv = [b' '; 16];
    let mut out = Vec::with_capacity(parsed.rows.len());
    for (host, name, enc) in parsed.rows {
        if let Some(plain) = decrypt_value(&host, &enc, key, &iv, parsed.meta_version) {
            out.push((host, name, plain));
        }
    }
    Ok(CookieRows { rows: out, meta_version: parsed.meta_version })
}

fn decrypt_value(
    host: &str,
    enc: &[u8],
    key: &[u8; 16],
    iv: &[u8; 16],
    meta_version: i64,
) -> Option<Vec<u8>> {
    // v10 / v11 头;空值原样
    if enc.len() < 3 {
        return None;
    }
    if &enc[..3] != b"v10" && &enc[..3] != b"v11" {
        return None;
    }
    let cipher = &enc[3..];
    let mut buf = cipher.to_vec();
    let plain = Aes128CbcDec::new(key.into(), iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .ok()?;
    let mut dec: Vec<u8> = plain.to_vec();
    if meta_version >= 24 {
        // 剥前 32 字节 SHA256(host_key)
        if dec.len() < 32 {
            return None;
        }
        let prefix = &dec[..32];
        let expected = Sha256::digest(host.as_bytes());
        if prefix != expected.as_slice() {
            return None;
        }
        dec = dec[32..].to_vec();
    }

    // UTF-8 校验
    if std::str::from_utf8(&dec).is_err() {
        return None;
    }
    Some(dec)
}

fn filter_quark_cookies(rows: Vec<(String, String, Vec<u8>)>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (host, name, value) in rows {
        if !is_quark_host(&host) {
            continue;
        }
        if let Ok(s) = String::from_utf8(value) {
            out.insert(name, s);
        }
    }
    out
}

fn is_quark_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h.contains("pan.quark.cn") || h.contains("drive.quark.cn") || h == "quark.cn" || h.ends_with(".quark.cn")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_roundtrip_v10_no_prefix() {
        let key: [u8; 16] = [
            0xfd, 0x39, 0x4d, 0x10, 0x6c, 0x4c, 0x6f, 0x2c,
            0x55, 0x5b, 0xfa, 0x97, 0x9c, 0x21, 0x12, 0x27,
        ];
        let iv = [b' '; 16];
        let plaintext = b"sl-session=abc123;dummy";

        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
        let mut buf = vec![0u8; plaintext.len() + 16];
        buf[..plaintext.len()].copy_from_slice(plaintext);
        let n: usize = Aes128CbcEnc::new(&key.into(), &iv.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plaintext.len())
            .expect("encrypt")
            .len();
        let cipher = &buf[..n];

        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(cipher);

        let dec = decrypt_value("pan.quark.cn", &blob, &key, &iv, 0).expect("decrypt");
        assert_eq!(dec, plaintext);
    }

    #[test]
    fn aes_roundtrip_v11_with_sha256_prefix() {
        let key: [u8; 16] = [0x42; 16];
        let iv = [b' '; 16];
        let host = "drive.quark.cn";
        let plaintext = b"__pus=hello-world";

        let prefix: [u8; 32] = Sha256::digest(host.as_bytes()).into();
        let mut with_prefix = Vec::new();
        with_prefix.extend_from_slice(&prefix);
        with_prefix.extend_from_slice(plaintext);

        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
        let mut buf = vec![0u8; with_prefix.len() + 16];
        buf[..with_prefix.len()].copy_from_slice(&with_prefix);
        let n: usize = Aes128CbcEnc::new(&key.into(), &iv.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buf, with_prefix.len())
            .expect("encrypt")
            .len();
        let cipher = &buf[..n];

        let mut blob = b"v11".to_vec();
        blob.extend_from_slice(cipher);

        let dec = decrypt_value(host, &blob, &key, &iv, 24).expect("decrypt");
        assert_eq!(dec, plaintext);
    }

    #[test]
    fn aes_rejects_wrong_sha256_prefix() {
        let key: [u8; 16] = [0x42; 16];
        let iv = [b' '; 16];
        let real_host = "drive.quark.cn";
        let wrong_host = "evil.example.com";
        let plaintext = b"__pus=x";

        let prefix: [u8; 32] = Sha256::digest(real_host.as_bytes()).into();
        let mut with_prefix = Vec::new();
        with_prefix.extend_from_slice(&prefix);
        with_prefix.extend_from_slice(plaintext);

        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
        let mut buf = vec![0u8; with_prefix.len() + 16];
        buf[..with_prefix.len()].copy_from_slice(&with_prefix);
        let n: usize = Aes128CbcEnc::new(&key.into(), &iv.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buf, with_prefix.len())
            .expect("encrypt")
            .len();
        let cipher = &buf[..n];

        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(cipher);

        assert!(decrypt_value(wrong_host, &blob, &key, &iv, 24).is_none());
    }

    #[test]
    fn filter_quark_cookies_only_quark() {
        let rows = vec![
            ("pan.quark.cn".into(), "__pus".into(), b"a".to_vec()),
            ("drive.quark.cn".into(), "sl-session".into(), b"b".to_vec()),
            ("example.com".into(), "evil".into(), b"c".to_vec()),
        ];
        let m = filter_quark_cookies(rows);
        assert_eq!(m.len(), 2);
        assert_eq!(m.get("__pus").unwrap(), "a");
        assert_eq!(m.get("sl-session").unwrap(), "b");
    }
}
