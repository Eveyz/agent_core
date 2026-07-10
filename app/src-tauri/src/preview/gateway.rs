//! Loopback preview gateway: static serving, reload WebSocket, framework proxy.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use std::convert::Infallible;

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures::StreamExt;
use mime_guess::from_path;
use parking_lot::RwLock;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use super::path_policy::{default_preview_csp, resolve_under_root, PathPolicyOptions};
use super::types::PreviewMode;
use super::watcher::ReloadNotification;

const RELOAD_SCRIPT: &str = r#"<script id="__agverse_reload">(function(){try{var p=location.pathname.split("/");var t=p.length>2?p[2]:"";var ws=new WebSocket((location.protocol==="https:"?"wss:":"ws:")+"//"+location.host+"/p/"+t+"/__agverse/reload");ws.onmessage=function(e){try{var m=JSON.parse(e.data);if(m.type==="reload")location.reload();}catch(_){location.reload();}};}catch(_){}})();</script>"#;

#[derive(Clone)]
pub struct GatewayState {
    pub token: String,
    pub root: PathBuf,
    pub gateway_port: u16,
    pub mode: PreviewMode,
    pub entrypoint: Option<String>,
    pub proxy_target: Arc<RwLock<Option<String>>>,
    pub reload_tx: broadcast::Sender<ReloadNotification>,
}

pub struct PreviewGateway {
    pub port: u16,
    pub token: String,
    pub url: String,
    cancel: CancellationToken,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl PreviewGateway {
    pub async fn start(
        root: PathBuf,
        mode: PreviewMode,
        entrypoint: Option<String>,
        reload_tx: broadcast::Sender<ReloadNotification>,
        proxy_target: Option<String>,
    ) -> anyhow::Result<Self> {
        let token = generate_token();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let port = addr.port();

        let state = GatewayState {
            token: token.clone(),
            root: root.clone(),
            gateway_port: port,
            mode,
            entrypoint,
            proxy_target: Arc::new(RwLock::new(proxy_target)),
            reload_tx,
        };

        let router = build_router(state);
        let cancel = CancellationToken::new();
        let cancel_serve = cancel.clone();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        tauri::async_runtime::spawn(async move {
            let server = axum::serve(listener, router).with_graceful_shutdown(async move {
                cancel_serve.cancelled().await;
                let _ = shutdown_rx.await;
            });
            if let Err(e) = server.await {
                eprintln!("preview gateway stopped: {e}");
            }
        });

        let url = format!("http://127.0.0.1:{port}/p/{token}/");

        Ok(Self {
            port,
            token,
            url,
            cancel,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub fn set_proxy_target(&self, _target: &str) {
        // Updated via shared GatewayState in manager
    }

    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn generate_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        bytes,
    )
}

fn build_router(state: GatewayState) -> Router {
    let state_for_fallback = state.clone();
    Router::new()
        .route("/p/{token}/__agverse/reload", get(reload_ws))
        .fallback_service(tower::service_fn(move |req: Request<Body>| {
            let state = state_for_fallback.clone();
            async move {
                Ok::<_, Infallible>(handle_http_request(state, req).await)
            }
        }))
        .with_state(state)
}

async fn handle_http_request(state: GatewayState, req: Request<Body>) -> Response {
    let method = req.method().clone();
    let headers = req.headers().clone();
    let path = req.uri().path();
    let Some(rest) = path.strip_prefix("/p/") else {
        return not_found();
    };
    if rest.contains("/__agverse/reload") {
        return not_found();
    }
    let mut parts = rest.splitn(2, '/');
    let token = parts.next().unwrap_or("");
    let rel = parts.next().unwrap_or("");
    serve_authed(&state, token, rel, method, &headers).await
}

async fn serve_authed(
    state: &GatewayState,
    token: &str,
    rel: &str,
    method: Method,
    headers: &HeaderMap,
) -> Response {
    if state.token != token {
        return not_found();
    }
    if !validate_host(&headers, state.gateway_port) {
        return not_found();
    }

    if state.mode == PreviewMode::Framework {
        let proxy_target = state.proxy_target.read().clone();
        if let Some(target) = proxy_target {
            return proxy_request(&target, rel, method, headers.clone()).await;
        }
    }

    let rel_path = if rel.is_empty() { None } else { Some(rel) };
    serve_static(state, rel_path, headers, method).await
}

fn validate_host(headers: &HeaderMap, expected_port: u16) -> bool {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    host == format!("127.0.0.1:{expected_port}")
        || host == format!("localhost:{expected_port}")
}

async fn reload_ws(
    ws: WebSocketUpgrade,
    axum::extract::Path(token): axum::extract::Path<String>,
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !validate_host(&headers, state.gateway_port) || state.token != token {
        return StatusCode::NOT_FOUND.into_response();
    }
    ws.on_upgrade(move |socket| handle_reload_socket(socket, state))
}

async fn handle_reload_socket(mut socket: WebSocket, state: GatewayState) {
    let mut rx = state.reload_tx.subscribe();
    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            Ok(note) = rx.recv() => {
                let payload = serde_json::json!({
                    "type": "reload",
                    "revision": note.revision,
                    "paths": note.paths,
                });
                if socket.send(Message::Text(payload.to_string().into())).await.is_err() {
                    break;
                }
            }
        }
    }
}

async fn serve_static(
    state: &GatewayState,
    rel_path: Option<&str>,
    headers: &HeaderMap,
    method: Method,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }

    let rel = rel_path
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .or_else(|| state.entrypoint.clone())
        .unwrap_or_else(|| "index.html".to_string());

    let resolved = match resolve_under_root(&state.root, &rel, PathPolicyOptions::default()) {
        Ok(p) => p,
        Err(_) => {
            if is_spa_navigation(headers) {
                if let Ok(p) = resolve_under_root(&state.root, "index.html", PathPolicyOptions::default()) {
                    return serve_file(&p, true).await;
                }
            }
            return not_found();
        }
    };

    if resolved.is_dir() {
        let index = resolved.join("index.html");
        if index.exists() {
            return serve_file(&index, true).await;
        }
        return not_found();
    }

    let is_html = resolved
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("html"))
        .unwrap_or(false);

    serve_file(&resolved, is_html).await
}

fn is_spa_navigation(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|a| a.contains("text/html"))
        .unwrap_or(false)
}

async fn serve_file(path: &Path, inject_reload: bool) -> Response {
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return not_found(),
    };
    if !meta.is_file() {
        return not_found();
    }

    let mime = from_path(path).first_or_octet_stream();
    let is_html = mime.type_() == "text" && mime.subtype() == "html";
    let mut resp = if inject_reload && is_html {
        match tokio::fs::read_to_string(path).await {
            Ok(mut html) => {
                if !html.contains("__agverse_reload") {
                    if let Some(idx) = html.to_lowercase().rfind("</body>") {
                        html.insert_str(idx, RELOAD_SCRIPT);
                    } else {
                        html.push_str(RELOAD_SCRIPT);
                    }
                }
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(html))
                    .unwrap()
            }
            Err(_) => file_response(path).await,
        }
    } else {
        file_response(path).await
    };

    apply_security_headers(&mut resp, &[]);
    if let Ok(ct) = HeaderValue::from_str(mime.as_ref()) {
        resp.headers_mut().insert(header::CONTENT_TYPE, ct);
    }
    if is_html || path.extension().and_then(|e| e.to_str()) == Some("map") {
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        );
    }
    resp
}

async fn file_response(path: &Path) -> Response {
    match tokio::fs::read(path).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(bytes))
            .unwrap(),
        Err(_) => not_found(),
    }
}

async fn proxy_request(
    target_base: &str,
    suffix: &str,
    method: Method,
    _headers: HeaderMap,
) -> Response {
    let uri_str = if suffix.is_empty() {
        format!("{target_base}/")
    } else {
        format!("{target_base}/{suffix}")
    };

    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };

    let body = bytes::Bytes::new();

    let rb = match method {
        Method::GET => client.get(&uri_str),
        Method::POST => client.post(&uri_str).body(body.to_vec()),
        Method::PUT => client.put(&uri_str).body(body.to_vec()),
        Method::DELETE => client.delete(&uri_str),
        Method::HEAD => client.head(&uri_str),
        _ => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
    };

    match rb.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut builder = Response::builder().status(status);
            for (k, v) in resp.headers().iter() {
                if k != header::SET_COOKIE && k != header::TRANSFER_ENCODING {
                    if let Ok(val) = HeaderValue::from_bytes(v.as_bytes()) {
                        builder = builder.header(k, val);
                    }
                }
            }
            let bytes = resp.bytes().await.unwrap_or_default();
            let mut response = builder.body(Body::from(bytes)).unwrap_or_else(|_| not_found());
            apply_security_headers(&mut response, &[]);
            response
        }
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

fn apply_security_headers(resp: &mut Response, extra_connect: &[&str]) {
    let csp = default_preview_csp(extra_connect);
    resp.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(&csp).unwrap_or_else(|_| HeaderValue::from_static("default-src 'none'")),
    );
    resp.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    resp.headers_mut().insert(
        header::HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    resp.headers_mut().insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
}

fn not_found() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::empty())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_url_safe() {
        let t = generate_token();
        assert!(!t.contains('+'));
        assert!(!t.contains('/'));
    }

    #[test]
    fn host_validation() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("127.0.0.1:9999"));
        assert!(validate_host(&h, 9999));
        assert!(!validate_host(&h, 8888));
    }
}
