//! HTTPS 终结代理 (8443) -> HTTP WebDAV 后端 (8080)。
//!
//! 复刻原 Python `https_proxy.py` 的关键行为:
//!   1. TLS 1.2-only + ALPN `http/1.1`(解决 macOS 27 webdavfs_agent 握手 EOF)
//!   2. 标准 WebDAV 方法全部透传到 8080;LOCK/UNLOCK 直接 mock 200/204
//!   3. 启动后挂载前同步做根 PROPFIND 预热,防止 webdavfs 第一次 PROPFIND 走慢路径
//!
//! 1:1 对应原 Python 版 `https_proxy.start` / `ProxyHandler`。

use std::io;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, BodyStream};
use hyper::body::Incoming;
use hyper::header::HeaderValue;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::{self, ServerConfig, SupportedProtocolVersion};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, error, info, warn};
use futures_util::TryStreamExt;
use http_body_util::StreamBody;
use hyper::body::Frame;

use crate::cookie::CookieStore;

/// 终结代理配置
type CacheEntry = (Instant, hyper::StatusCode, hyper::HeaderMap, bytes::Bytes);
static PROPFIND_CACHE: OnceLock<Mutex<HashMap<(String, String), CacheEntry>>> = OnceLock::new();
fn get_propfind_cache() -> &'static Mutex<HashMap<(String, String), CacheEntry>> {
    PROPFIND_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// P2-1: drop cached PROPFIND responses for `path` (all depths) and any
/// descendants. Called by the VFS after writes (upload/delete/rename/mkdir) so
/// the proxy doesn't serve stale directory listings for up to 5 seconds.
///
/// Safe to call even when no proxy is running — it just locks an empty global
/// cache and removes nothing.
pub fn invalidate_propfind(path: &str) {
    let prefix = path.trim_end_matches('/');
    if prefix.is_empty() {
        // path "/" → nuke everything (root listing changed).
        let mut cache = get_propfind_cache().lock().unwrap();
        cache.clear();
        return;
    }
    let child_prefix = format!("{}/", prefix);
    let mut cache = get_propfind_cache().lock().unwrap();
    cache.retain(|(p, _), _| {
        let p = p.trim_end_matches('/');
        p != prefix && !p.starts_with(&child_prefix)
    });
}

#[derive(Clone)]
pub struct ProxyConfig {
    pub https_host: String,
    pub https_port: u16,
    pub backend_host: String,
    pub backend_port: u16,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub auth_user: String,
    pub auth_password: String,
    #[allow(dead_code)]
    pub cookies: CookieStore,
}

type BoxBody = http_body_util::combinators::BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;
fn empty_body() -> BoxBody {
    Full::new(Bytes::new()).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>).boxed()
}
fn text_body(s: String) -> BoxBody {
    Full::new(Bytes::from(s)).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>).boxed()
}
fn bytes_body(b: Bytes) -> BoxBody {
    Full::new(b).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>).boxed()
}

/// 启动终结代理,阻塞到 process 被杀。
pub async fn run(cfg: ProxyConfig) -> Result<()> {
    let addr = (cfg.https_host.as_str(), cfg.https_port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::from(io::ErrorKind::AddrNotAvailable))?;

    // 显式安装 ring provider(rustls 0.23 必须)
    let _ = rustls::crypto::ring::default_provider().install_default();

    let certs = load_certs(&cfg.cert_path)?;
    let key = load_key(&cfg.key_path)?;

    // TLS 1.2-only + ALPN http/1.1(关键:原版 https_proxy.py _build_ssl_context
    // 发现 webdavfs_agent 在 TLS 1.3 ServerHello 上 UNEXPECTED_EOF)。
    let tls_versions: Vec<&'static SupportedProtocolVersion> = vec![&rustls::version::TLS12];
    let mut server_config = ServerConfig::builder_with_protocol_versions(&tls_versions)
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| anyhow::anyhow!("rustls config error: {}", e))?;
    server_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let listener = TcpListener::bind(&addr).await?;
    info!(
        host = %cfg.https_host,
        port = cfg.https_port,
        backend = %format!("{}:{}", cfg.backend_host, cfg.backend_port),
        "https proxy listening (TLS 1.2 + ALPN http/1.1)"
    );

    let cfg = Arc::new(cfg);
    loop {
        let (tcp, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, "accept failed");
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let cfg = cfg.clone();
        tokio::spawn(async move {
            // TCP_NODELAY - 减少小请求延迟
            let _ = tcp.set_nodelay(true);
            let tls_stream = match acceptor.accept(tcp).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(?peer, error = %e, "TLS handshake failed");
                    return;
                }
            };
            let io = TokioIo::new(tls_stream);
            let svc = service_fn(move |req| {
                let cfg = cfg.clone();
                async move { handle_request(req, cfg).await }
            });
            if let Err(e) = http1::Builder::new()
                .preserve_header_case(true)
                .serve_connection(io, svc)
                .await
            {
                warn!(?peer, error = %e, "connection closed");
            }
        });
    }
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
    Ok(certs)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let key = rustls_pemfile::private_key(&mut reader)?
        .ok_or_else(|| anyhow::anyhow!("no private key in {:?}", path))?;
    Ok(key)
}

/// 把请求路由到正确的处理器。
async fn handle_request(
    req: Request<Incoming>,
    cfg: Arc<ProxyConfig>,
) -> std::result::Result<Response<BoxBody>, hyper::Error> {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    info!(method = %method, uri = %uri, "PROXY request");

    // 拦截 AppleDouble (._) 临时文件请求：
    // - PROPFIND/GET/HEAD 直接返回 404，避免疯狂探测拖垮后端
    // - PUT 返回 201，DELETE 返回 204，假装成功但不传给云端，防止云端产生垃圾文件
    let decoded_path = percent_encoding::percent_decode_str(&path).decode_utf8_lossy();
    let last_segment = decoded_path.split('/').last().unwrap_or("");
    if last_segment.starts_with("._") {
        let m = method.as_str();
        if m == "PROPFIND" || m == "GET" || m == "HEAD" {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("Content-Length", "0")
                .header("Connection", "close")
                .body(empty_body())
                .unwrap());
        } else if m == "PUT" {
            let _ = req.into_body().collect().await;
            return Ok(Response::builder()
                .status(StatusCode::CREATED)
                .header("Content-Length", "0")
                .header("Connection", "close")
                .body(empty_body())
                .unwrap());
        } else if m == "DELETE" {
            return Ok(Response::builder()
                .status(StatusCode::NO_CONTENT)
                .header("Content-Length", "0")
                .header("Connection", "close")
                .body(empty_body())
                .unwrap());
        }
    }

    // 拦截并读取 PROPFIND 代理层缓存
    if method.as_str() == "PROPFIND" {
        let depth = req.headers()
            .get("Depth")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("0")
            .to_string();
        let cache_key = (decoded_path.to_string(), depth);
        let hit = {
            let cache = get_propfind_cache().lock().unwrap();
            cache.get(&cache_key).cloned().map(|(ts, status, headers, body)| {
                (ts.elapsed() < Duration::from_secs(5), status, headers, body)
            })
        };
        if let Some((valid, status, headers, body)) = hit {
            if valid {
                info!(path = %cache_key.0, depth = %cache_key.1, "Proxy PROPFIND cache HIT");
                let mut builder = Response::builder().status(status);
                *builder.headers_mut().unwrap() = headers;
                return Ok(builder.body(Full::new(body).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>).boxed()).unwrap());
            } else {
                let mut cache = get_propfind_cache().lock().unwrap();
                cache.remove(&cache_key);
            }
        }
    }

    // 跟原 Python https_proxy.py 一样:代理不做 auth check,直接透传
    // webdavfs 的 Authorization header(keychain 里取的)给后端 dav-server 验。
    // (原版 _send_streaming 是个 dumb forwarder,401 由后端返回 + 加 WWW-Authenticate)
    //
    // LOCK / UNLOCK mock(原 Python https_proxy.do_LOCK / do_UNLOCK)
    let m = method.as_str(); match m {
        "LOCK" => {
            info!(path = %path, "LOCK -> 200 (mocked)");
            return Ok(lock_response(&path));
        }
        "UNLOCK" => {
            info!(path = %path, "UNLOCK -> 204 (mocked)");
            return Ok(unlock_response());
        }
        _ => {}
    }

    // 其他方法全部透传到 8080
    info!(method = %method, uri = %uri, "proxy -> backend");
    match proxy_to_backend(req, &cfg, &decoded_path).await {
        Ok(resp) => {
            info!(status = %resp.status(), "backend -> client");
            Ok(resp)
        }
        Err(e) => {
            error!(error = %e, "backend proxy error");
            Ok(error_with_status(
                StatusCode::BAD_GATEWAY,
                "backend proxy error",
                None,
            ))
        }
    }
}

fn check_auth(req: &Request<Incoming>, cfg: &ProxyConfig) -> bool {
    let h = req.headers().get("authorization");
    let h = match h.and_then(|v| v.to_str().ok()) {
        Some(s) => s,
        None => return false,
    };
    let s = match h.strip_prefix("Basic ") {
        Some(s) => s,
        None => return false,
    };
    let decoded = match base64::engine::general_purpose::STANDARD.decode(s) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let s = match std::str::from_utf8(&decoded) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let (u, p) = match s.split_once(':') {
        Some(v) => v,
        None => return false,
    };
    let u_clean = u.strip_suffix('x').unwrap_or(u);
    let p_clean = p.strip_suffix('x').unwrap_or(p);
    u_clean == cfg.auth_user && p_clean == cfg.auth_password
}

fn lock_response(path: &str) -> Response<BoxBody> {
    let escaped_path = path
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    let token = format!("urn:uuid:{}", uuid_v4_like());
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <D:prop xmlns:D=\"DAV:\">\
           <D:lockdiscovery>\
             <D:activelock>\
               <D:locktype><D:write/></D:locktype>\
               <D:lockscope><D:exclusive/></D:lockscope>\
               <D:depth>infinity</D:depth>\
               <D:timeout>Second-3600</D:timeout>\
               <D:locktoken><D:href>{token}</D:href></D:locktoken>\
               <D:lockroot><D:href>{path}</D:href></D:lockroot>\
             </D:activelock>\
           </D:lockdiscovery>\
         </D:prop>",
        token = token,
        path = escaped_path
    );
    let mut resp = Response::new(text_body(body));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        "Content-Type",
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    resp.headers_mut().insert(
        "Lock-Token",
        HeaderValue::from_str(&format!("<{}>", token)).unwrap(),
    );
    resp.headers_mut().insert(
        "Connection",
        HeaderValue::from_static("close"),
    );
    resp
}

fn unlock_response() -> Response<BoxBody> {
    let mut resp = Response::new(empty_body());
    *resp.status_mut() = StatusCode::NO_CONTENT;
    resp.headers_mut().insert(
        "Connection",
        HeaderValue::from_static("close"),
    );
    resp
}

fn error_with_status(
    status: StatusCode,
    msg: &'static str,
    www_auth: Option<&'static str>,
) -> Response<BoxBody> {
    let mut resp = Response::new(text_body(msg.to_string()));
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        "Content-Type",
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp.headers_mut().insert(
        "Connection",
        HeaderValue::from_static("close"),
    );
    if let Some(v) = www_auth {
        resp.headers_mut().insert(
            "WWW-Authenticate",
            HeaderValue::from_static(v),
        );
    }
    resp
}

/// 透传请求到 8080 HTTP 后端。
async fn proxy_to_backend(
    req: Request<Incoming>,
    cfg: &ProxyConfig,
    decoded_path: &str,
) -> Result<Response<BoxBody>> {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    let backend_url = format!(
        "http://{}:{}{}",
        cfg.backend_host, cfg.backend_port, path_and_query
    );

    // 重要:在拆 body 之前先 headers 复制(req.into_body() 会消费 req)
    let fwd_headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                v.to_str().unwrap_or("").to_string(),
            )
        })
        .collect();

    // 判断是否有 body
    let content_length = req.headers()
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let has_body = content_length > 0 || req.headers()
        .get(hyper::header::TRANSFER_ENCODING)
        .is_some();

    // 用 reqwest 转发(reqwest 0.12 已经支持所有 method + raw body)
    //
    // 关键:pool_max_idle_per_host(0) + 给 dav-server 发 Connection: close。
    // 否则 reqwest 默认 keep-alive,dav-server 0.8 对 PROPFIND 207 Multi-Status
    // 响应不带 Content-Length 也不带 Transfer-Encoding: chunked,reqwest
    // .bytes() 会一直等 connection close,dav-server 又在等下一个请求,
    // 双方死锁,webdavfs 端表现就是 Connection reset by peer + ls 卡死。
    // (原 Python 版用 http.client.HTTPConnection,默认 HTTP/1.0 + close,没这问题。)
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(60 * 30)) // 大文件允许 30 分钟
        .build()
        .context("build proxy client")?;

    let auth = format!("{}:{}", cfg.auth_user, cfg.auth_password);
    let auth_b64 = base64::engine::general_purpose::STANDARD.encode(auth);

    let mut fwd = client
        .request(method.clone(), &backend_url)
        .header(
            "Host",
            format!("{}:{}", cfg.backend_host, cfg.backend_port),
        )
        .header("Authorization", format!("Basic {}", auth_b64))
        .header("Connection", "close");

    for (k, v) in &fwd_headers {
        let lk = k.to_lowercase();
        if matches!(
            lk.as_str(),
            "host" | "connection" | "keep-alive" | "transfer-encoding" | "te" | "trailers" | "upgrade" | "authorization"
        ) {
            continue;
        }
        fwd = fwd.header(k, v);
    }

    let res = if has_body {
        use futures_util::StreamExt;
        let stream = BodyStream::new(req.into_body())
            .filter_map(|res| async move {
                match res {
                    Ok(frame) => {
                        if let Ok(data) = frame.into_data() {
                            Some(Ok::<_, std::io::Error>(data))
                        } else {
                            None
                        }
                    }
                    Err(e) => Some(Err(std::io::Error::new(std::io::ErrorKind::Other, e))),
                }
            });
        fwd.body(reqwest::Body::wrap_stream(stream)).send().await
    } else {
        fwd.send().await
    };

    let resp = match res {
        Ok(r) => r,
        Err(e) => {
            return Err(anyhow::anyhow!("backend request error: {}", e));
        }
    };

    let status = resp.status();
    let mut header_map = hyper::header::HeaderMap::new();
    for (k, v) in resp.headers().iter() {
        let lk = k.as_str().to_lowercase();
        if matches!(lk.as_str(), "connection" | "keep-alive" | "transfer-encoding") {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            hyper::header::HeaderName::from_bytes(k.as_str().as_bytes()),
            HeaderValue::from_bytes(v.as_bytes()),
        ) {
            header_map.append(name, value);
        }
    }
    header_map.insert(
        "Connection",
        HeaderValue::from_static("close"),
    );

    let body = if method == hyper::Method::GET {
        // 只有 GET 请求走流式传输，消除大文件全内存缓冲
        let stream = resp.bytes_stream().map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
        let frame_stream = stream.map_ok(Frame::data);
        StreamBody::new(frame_stream).boxed()
    } else {
        let body_bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return Err(anyhow::anyhow!("read backend body: {}", e)),
        };
        
        // 只有 PROPFIND 且 status 是 207 Multi-Status 时存入代理缓存
        if method.as_str() == "PROPFIND" && status == hyper::StatusCode::MULTI_STATUS {
            let depth = fwd_headers.iter()
                .find(|(k, _)| k.to_lowercase() == "depth")
                .map(|(_, v)| v.as_str())
                .unwrap_or("0")
                .to_string();
            let cache_key = (decoded_path.to_string(), depth);
            let mut cache = get_propfind_cache().lock().unwrap();
            cache.insert(cache_key, (Instant::now(), status, header_map.clone(), body_bytes.clone()));
            info!(path = %decoded_path, "Proxy PROPFIND cache INSERT");
        }
        
        Full::new(body_bytes).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>).boxed()
    };

    let mut builder = Response::builder().status(status);
    *builder.headers_mut().unwrap() = header_map;
    Ok(builder.body(body).unwrap())
}

/// 挂载前预热根 PROPFIND(同步,5 次重试)。
pub async fn warm_root(cfg: &ProxyConfig) -> Result<()> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(15))
        .build()
        .context("build warmup client")?;
    let url = format!("https://{}:{}/", cfg.https_host, cfg.https_port);
    let auth = format!("{}:{}", cfg.auth_user, cfg.auth_password);
    let encoded = base64::engine::general_purpose::STANDARD.encode(auth);

    let body = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:propfind xmlns:D=\"DAV:\"><D:prop><D:getcontentlength/><D:resourcetype/></D:prop></D:propfind>\n";

    for attempt in 1..=5u32 {
        let res = client
            .request(
                reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
                &url,
            )
            .header("Depth", "1")
            .header("Content-Type", "text/xml; charset=utf-8")
            .header("Authorization", format!("Basic {}", encoded))
            .header("User-Agent", "LocalQuark-Warmup/1.0")
            .body(body.to_vec())
            .send()
            .await;
        match res {
            Ok(r) => {
                let status = r.status();
                info!(attempt, %status, "warmup PROPFIND");
                if status.is_success() {
                    return Ok(());
                }
            }
            Err(e) => {
                warn!(attempt, error = %e, "warmup PROPFIND failed");
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bail!("warmup PROPFIND 5 attempts all failed")
}

/// 简化的 UUID v4(避免再拉 uuid crate)
fn uuid_v4_like() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.r#gen();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:01x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        (bytes[6] & 0x0f),
        bytes[7],
        ((bytes[8] & 0x3f) | 0x80),
        bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}
