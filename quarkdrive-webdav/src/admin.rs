//! Local 设置面板：127.0.0.1:8444 提供 HTTP API + 简易 HTML 页面。
//!
//! API endpoints (JSON):
//!   GET  /api/status       健康状态（health level + cookies/webdav/mount 标志）
//!   POST /api/repair       触发 repair()
//!   POST /api/refresh      刷新 cookies（立即拉浏览器）
//!   GET  /api/logs/tail?n= 最近 n 行日志（默认 100）
//!   GET  /api/cache/info   缓存目录大小 / 文件数
//!   POST /api/cache/clear  清空 propfind + chunks 缓存
//!
//! 页面:
//!   GET  /                 简易 HTML 设置面板（状态 + 操作按钮 + 日志 tail）

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

use crate::cookie::{CookieStore, DEFAULT_BROWSERS};
use crate::health::HealthChecker;
use crate::mount::MountConfig;

#[derive(Clone)]
pub struct AdminState {
    pub health: Arc<HealthChecker>,
    pub mount_cfg: MountConfig,
    pub webdav_url: String,
    pub log_path: PathBuf,
    pub cache_dir: PathBuf,
    pub cookies: CookieStore,
}

pub async fn run(addr: SocketAddr, state: AdminState) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(%addr, "admin HTTP server listening");

    loop {
        let (tcp, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                warn!(error = %e, "admin accept failed");
                continue;
            }
        };
        let state = state.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(tcp);
            let svc = service_fn(move |req| handle(req, state.clone()));
            if let Err(e) = auto::Builder::new(TokioExecutor::new())
                .http1()
                .keep_alive(false)
                .serve_connection(io, svc)
                .await
            {
                // 客户端断连不算错
                if !e.to_string().contains("connection closed") {
                    error!(peer = %peer, error = %e, "admin serve error");
                }
            }
        });
    }
}

async fn handle(
    req: Request<hyper::body::Incoming>,
    state: AdminState,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let resp = match (method, path.as_str()) {
        (Method::GET, "/") => html_index(&state),
        (Method::GET, "/api/status") => api_status(&state).await,
        (Method::POST, "/api/repair") => api_repair(&state).await,
        (Method::POST, "/api/refresh") => api_refresh(&state).await,
        (Method::GET, "/api/logs/tail") => {
            let n: usize = req.uri().query()
                .and_then(|q| {
                    q.split('&').find_map(|kv| {
                        kv.strip_prefix("n=").and_then(|v| v.parse().ok())
                    })
                })
                .unwrap_or(100);
            api_logs_tail(&state, n)
        }
        (Method::GET, "/api/cookies") => api_cookies_list(&state),
        (Method::GET, "/api/cache/info") => api_cache_info(&state),
        (Method::POST, "/api/cache/clear") => api_cache_clear(&state),
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(r#"{"error":"not found"}"#)))
            .unwrap(),
    };
    Ok(resp)
}

fn json_response(status: StatusCode, body: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json; charset=utf-8")
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

async fn api_status(state: &AdminState) -> Response<Full<Bytes>> {
    let report = state.health.check().await;
    let body = serde_json::json!({
        "level": format!("{:?}", report.level),
        "healthy": report.is_healthy(),
        "cookies_ok": report.cookies_ok,
        "webdav_ok": report.webdav_ok,
        "mounted_ok": report.mounted_ok,
        "webdav_url": state.webdav_url,
        "mount_point": state.mount_cfg.mount_point.display().to_string(),
    });
    json_response(StatusCode::OK, body.to_string())
}

async fn api_repair(state: &AdminState) -> Response<Full<Bytes>> {
    let report = state.health.check().await;
    let body = match state.health.repair(&report, &state.mount_cfg).await {
        Ok(()) => serde_json::json!({"ok": true, "level_before": format!("{:?}", report.level)}),
        Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
    };
    json_response(StatusCode::OK, body.to_string())
}

async fn api_refresh(state: &AdminState) -> Response<Full<Bytes>> {
    let body = match CookieStore::from_chromium(DEFAULT_BROWSERS).await {
        Ok(new_store) => {
            let count = new_store.snapshot().len();
            state.cookies.replace(new_store.snapshot());
            serde_json::json!({"ok": true, "loaded_count": count})
        }
        Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
    };
    json_response(StatusCode::OK, body.to_string())
}

fn api_logs_tail(state: &AdminState, n: usize) -> Response<Full<Bytes>> {
    use std::io::Read;
    let body = match std::fs::File::open(&state.log_path) {
        Ok(mut f) => {
            // 简化：读全部行然后取最后 n 行
            let mut buf = String::new();
            if let Err(e) = f.read_to_string(&mut buf) {
                serde_json::json!({"error": e.to_string()})
            } else {
                let lines: Vec<&str> = buf.lines().collect();
                let start = lines.len().saturating_sub(n);
                let tail: Vec<&str> = lines[start..].to_vec();
                serde_json::json!({"lines": tail})
            }
        }
        Err(e) => serde_json::json!({"error": e.to_string()}),
    };
    json_response(StatusCode::OK, body.to_string())
}

fn api_cookies_list(state: &AdminState) -> Response<Full<Bytes>> {
    // 只暴露 cookie key + 是否有值,不暴露实际值(避免泄露 session token)。
    let snap = state.cookies.snapshot();
    let keys: Vec<serde_json::Value> = snap.iter().map(|(k, v)| {
        serde_json::json!({"key": k, "has_value": !v.is_empty(), "len": v.len()})
    }).collect();
    let body = serde_json::json!({"count": snap.len(), "items": keys});
    json_response(StatusCode::OK, body.to_string())
}

fn api_cache_info(state: &AdminState) -> Response<Full<Bytes>> {
    let dir = &state.cache_dir;
    let (files, total_bytes) = count_dir(dir);
    let body = serde_json::json!({
        "cache_dir": dir.display().to_string(),
        "files": files,
        "bytes": total_bytes,
    });
    json_response(StatusCode::OK, body.to_string())
}

fn api_cache_clear(state: &AdminState) -> Response<Full<Bytes>> {
    let dir = &state.cache_dir;
    let body = match std::fs::remove_dir_all(dir).or_else(|_| Ok::<(), std::io::Error>(())) {
        Ok(_) => {
            let _ = std::fs::create_dir_all(dir);
            serde_json::json!({"ok": true})
        }
        Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
    };
    json_response(StatusCode::OK, body.to_string())
}

fn count_dir(dir: &std::path::Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut total = 0u64;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for ent in rd.flatten() {
            if let Ok(meta) = ent.metadata() {
                if meta.is_file() {
                    files += 1;
                    total += meta.len();
                }
            }
        }
    }
    (files, total)
}

fn html_index(state: &AdminState) -> Response<Full<Bytes>> {
    let html = format!(r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>LocalQuark 设置面板</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
body {{ font: 14px/1.4 -apple-system, system-ui, sans-serif; margin: 0; background: #f4f5f7; }}
.wrap {{ max-width: 920px; margin: 24px auto; padding: 0 16px; }}
h1 {{ font-size: 20px; margin: 0 0 16px; }}
.card {{ background: #fff; border-radius: 8px; padding: 16px; margin-bottom: 16px;
        box-shadow: 0 1px 2px rgba(0,0,0,.06); }}
.kv {{ display: grid; grid-template-columns: 160px 1fr; gap: 6px 12px; }}
.kv div:nth-child(odd) {{ color: #6b7280; }}
.btn {{ display: inline-block; padding: 6px 12px; margin-right: 8px; margin-bottom: 4px;
        background: #2563eb; color: #fff; border: 0; border-radius: 6px; cursor: pointer; font-size: 13px; }}
.btn:hover {{ background: #1d4ed8; }}
.btn:disabled {{ background: #9ca3af; cursor: not-allowed; }}
.log {{ font: 12px/1.45 ui-monospace, Menlo, monospace; background: #111827; color: #d1d5db;
       padding: 10px; border-radius: 6px; max-height: 360px; overflow: auto; white-space: pre-wrap; }}
.tag {{ display: inline-block; padding: 2px 8px; border-radius: 999px; font-size: 12px; }}
.ok {{ background: #d1fae5; color: #065f46; }}
.warn {{ background: #fef3c7; color: #92400e; }}
.err {{ background: #fee2e2; color: #991b1b; }}
</style>
</head>
<body>
<div class="wrap">
  <h1>LocalQuark 设置面板</h1>

  <div class="card">
    <h2 style="font-size:15px;margin:0 0 10px">运行状态</h2>
    <div class="kv">
      <div>挂载点</div><div id="mount">{mp}</div>
      <div>WebDAV</div><div id="webdav">{url}</div>
      <div>健康等级</div><div><span id="level" class="tag">加载中…</span></div>
      <div>Cookies</div><div><span id="cookies">—</span> <span id="cookies_count" style="color:#9ca3af"></span></div>
      <div>WebDAV 可达</div><div id="webdav_ok">—</div>
      <div>挂载状态</div><div id="mounted">—</div>
    </div>
  </div>

  <div class="card">
    <h2 style="font-size:15px;margin:0 0 10px">操作</h2>
    <button class="btn" onclick="repair()">触发修复</button>
    <button class="btn" onclick="refresh()">刷新 Cookie</button>
    <button class="btn" onclick="cacheInfo()">查看缓存</button>
    <button class="btn" onclick="cacheClear()">清理缓存</button>
    <button class="btn" onclick="reload()">刷新状态</button>
    <span id="op_msg" style="margin-left:8px;color:#374151"></span>
  </div>

  <div class="card">
    <h2 style="font-size:15px;margin:0 0 10px">最近日志 <span style="color:#9ca3af;font-weight:normal">(tail 100)</span></h2>
    <div id="log" class="log">加载中…</div>
  </div>
</div>

<script>
async function get(path) {{ const r = await fetch(path); return r.json(); }}
async function post(path) {{ const r = await fetch(path, {{method:'POST'}}); return r.json(); }}

function tag(level) {{
  if (level === 'Healthy') return '<span class="tag ok">Healthy</span>';
  return '<span class="tag warn">' + level + '</span>';
}}

async function reload() {{
  const s = await get('/api/status');
  document.getElementById('level').innerHTML = tag(s.level);
  document.getElementById('cookies').textContent = s.cookies_ok ? 'OK' : '缺失';
  try {{
    const c = await get('/api/cookies');
    document.getElementById('cookies_count').textContent = '(' + c.count + ' 项)';
  }} catch(e) {{}}
  document.getElementById('webdav_ok').textContent = s.webdav_ok ? 'OK' : '不可达';
  document.getElementById('mounted').textContent = s.mounted_ok ? '已挂载' : '未挂载';
  const log = await get('/api/logs/tail?n=100');
  document.getElementById('log').textContent = (log.lines || []).join('\n') || '(空)';
}}

async function repair() {{ btn(this, () => post('/api/repair')); }}
async function refresh() {{ btn(this, () => post('/api/refresh')); }}
async function cacheInfo() {{ btn(this, async () => get('/api/cache/info')); }}
async function cacheClear() {{
  if (!confirm('确认清理缓存？')) return;
  btn(document.querySelectorAll('.btn')[3], () => post('/api/cache/clear'));
}}

async function btn(el, fn) {{
  el.disabled = true; el.textContent = el.textContent + ' …';
  try {{
    const r = await fn();
    document.getElementById('op_msg').textContent = JSON.stringify(r);
    setTimeout(() => reload(), 500);
  }} catch(e) {{
    document.getElementById('op_msg').textContent = '错误: ' + e;
  }} finally {{
    setTimeout(() => {{ el.disabled = false; el.textContent = el.textContent.replace(' …',''); }}, 600);
  }}
}}

reload();
setInterval(reload, 5000);
</script>
</body>
</html>"#,
        mp = state.mount_cfg.mount_point.display(),
        url = state.webdav_url,
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Full::new(Bytes::from(html)))
        .unwrap()
}
