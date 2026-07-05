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
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
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

use crate::cookie::CookieStore;

/// 终结代理配置
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

type BoxBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;
fn empty_body() -> BoxBody {
    Full::new(Bytes::new()).map_err(|never| match never {}).boxed()
}
fn text_body(s: String) -> BoxBody {
    Full::new(Bytes::from(s)).map_err(|never| match never {}).boxed()
}
fn bytes_body(b: Bytes) -> BoxBody {
    Full::new(b).map_err(|never| match never {}).boxed()
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

    // 跟原 Python https_proxy.py 一样:代理不做 auth check,直接透传
    // webdavfs 的 Authorization header(keychain 里取的)给后端 dav-server 验。
    // (原版 _send_streaming 是个 dumb forwarder,401 由后端返回 + 加 WWW-Authenticate)
    //
    // LOCK / UNLOCK mock(原 Python https_proxy.do_LOCK / do_UNLOCK)
    let m = method.as_str(); match m {
        "LOCK" => {
            info!(path = %path, "LOCK -> 200 (mocked)");
            return Ok(lock_response());
        }
        "UNLOCK" => {
            info!(path = %path, "UNLOCK -> 204 (mocked)");
            return Ok(unlock_response());
        }
        _ => {}
    }

    // 其他方法全部透传到 8080
    info!(method = %method, uri = %uri, "proxy -> backend");
    match proxy_to_backend(req, &cfg).await {
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
    u == cfg.auth_user && p == cfg.auth_password
}

fn lock_response() -> Response<BoxBody> {
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
             </D:activelock>\
           </D:lockdiscovery>\
         </D:prop>"
    );
    let mut resp = Response::new(text_body(body));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        "Content-Type",
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    resp.headers_mut().insert(
        "Lock-Token",
        HeaderValue::from_str(&token).unwrap(),
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

    // 读取 body
    let body_bytes = match req.into_body().collect().await {
        Ok(c) => c.to_bytes().to_vec(),
        Err(e) => {
            return Err(anyhow::anyhow!("read request body error: {}", e));
        }
    };

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
        .request(method, &backend_url)
        .header(
            "Host",
            format!("{}:{}", cfg.backend_host, cfg.backend_port),
        )
        .header("Authorization", format!("Basic {}", auth_b64))
        .header("Connection", "close");

    for (k, v) in fwd_headers {
        let lk = k.to_lowercase();
        if matches!(
            lk.as_str(),
            "host" | "connection" | "keep-alive" | "transfer-encoding" | "te" | "trailers" | "upgrade" | "authorization"
        ) {
            continue;
        }
        fwd = fwd.header(&k, &v);
    }

    let res = if body_bytes.is_empty() {
        fwd.send().await
    } else {
        fwd.body(body_bytes).send().await
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

    // 一次性 read body(简化版;大文件 streaming 可后续优化)
    let body = match resp.bytes().await {
        Ok(b) => bytes_body(b),
        Err(e) => return Err(anyhow::anyhow!("read backend body: {}", e)),
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
