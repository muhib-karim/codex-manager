use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::models::{ProxyServerConfig, ProxyStatus, RequestContext};
use crate::routing::{is_account_failure, RoutingEngine};

/// Largest request body the proxy accepts (64 MiB).
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Request headers copied from the client to the upstream. `Authorization`
/// is never copied: it carries the proxy key and is replaced by the key of
/// the selected account.
const FORWARDED_REQUEST_HEADERS: [&str; 6] = [
    "content-type",
    "accept",
    "openai-beta",
    "openai-project",
    "idempotency-key",
    "user-agent",
];

/// Shared state for the proxy server
#[derive(Clone)]
pub struct ProxyState {
    pub config: Arc<ProxyServerConfig>,
    pub routing_engine: Arc<RoutingEngine>,
    pub http_client: reqwest::Client,
    pub request_count: Arc<AtomicU64>,
    pub started_at: Instant,
}

impl ProxyState {
    pub fn new(routing_engine: Arc<RoutingEngine>, config: ProxyServerConfig) -> Self {
        // No overall timeout: streamed completions can run for minutes.
        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            config: Arc::new(config),
            routing_engine,
            http_client,
            request_count: Arc::new(AtomicU64::new(0)),
            started_at: Instant::now(),
        }
    }
}

/// Compare two byte strings without stopping at the first difference.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Check the `Authorization: Bearer <proxy key>` header
pub fn is_authorized(headers: &HeaderMap, expected_key: &str) -> bool {
    if expected_key.is_empty() {
        return false;
    }
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    provided.is_some_and(|key| constant_time_eq(key.as_bytes(), expected_key.as_bytes()))
}

/// Join the configured upstream base URL and the request path + query.
/// A base that already ends in `/v1` is not doubled.
pub fn build_upstream_url(base_url: &str, path_and_query: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let base = match base.strip_suffix("/v1") {
        Some(without_version) if path_and_query.starts_with("/v1") => without_version,
        _ => base,
    };
    format!("{base}{path_and_query}")
}

/// Which upstream response headers are passed back to the client
pub fn should_forward_response_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "content-type"
        || name == "cache-control"
        || name == "retry-after"
        || name == "x-request-id"
        || name.starts_with("x-ratelimit-")
        || name.starts_with("openai-")
}

/// Derive a session id from the first message of a chat request, so the
/// sticky strategy can keep a conversation on one account. Returns `None`
/// when the request has no usable first message.
pub fn extract_session_id(body: &Value) -> Option<String> {
    let first_message = body.get("messages")?.as_array()?.first()?;
    let content = first_message.get("content")?.as_str()?;
    if content.is_empty() {
        return None;
    }
    let digest = Sha256::digest(content.as_bytes());
    let mut session = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        session.push_str(&format!("{byte:02x}"));
    }
    Some(session)
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    let body = serde_json::json!({
        "error": {
            "message": message,
            "type": "codex_manager_proxy_error",
            "code": code,
        }
    });
    (status, Json(body)).into_response()
}

/// Health check. Unauthenticated on purpose and limited to non-sensitive
/// information.
async fn health_handler(State(state): State<ProxyState>) -> Json<Value> {
    let stats = state.routing_engine.stats();
    let status = if stats.available_accounts > 0 {
        "ok"
    } else {
        "degraded"
    };
    Json(serde_json::json!({
        "status": status,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": state.started_at.elapsed().as_secs(),
    }))
}

/// Forward any other request to the upstream API with the credentials of
/// the account chosen by the routing engine. Method, path, query string,
/// body and content type are passed through unchanged; the upstream status
/// and body are returned as they are, streamed.
async fn proxy_handler(State(state): State<ProxyState>, request: Request) -> Response {
    if !is_authorized(request.headers(), &state.config.api_key) {
        warn!("Unauthorized request: invalid or missing proxy API key");
        return error_response(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "Missing or invalid proxy API key",
        );
    }
    state.request_count.fetch_add(1, Ordering::Relaxed);

    let (parts, body) = request.into_parts();
    let path_and_query = match parts.uri.path_and_query() {
        Some(value) => value.as_str().to_string(),
        None => parts.uri.path().to_string(),
    };

    let body_bytes = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "Request body could not be read or is too large",
            );
        }
    };

    // The body is only inspected to pick an account; it is forwarded as is.
    let json_body: Option<Value> = serde_json::from_slice(&body_bytes).ok();
    let model = json_body
        .as_ref()
        .and_then(|value| value.get("model"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let session_id = json_body.as_ref().and_then(extract_session_id);

    let mut ctx = RequestContext::new(model);
    if let Some(session) = session_id {
        ctx = ctx.with_session(session);
    }

    let decision = match state.routing_engine.resolve_account(&ctx) {
        Ok(decision) => decision,
        Err(reason) => {
            warn!("Routing failed: {reason}");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "no_available_account",
                &reason,
            );
        }
    };
    debug!(
        "Routing {} {} to account {} ({})",
        parts.method, path_and_query, decision.account_label, decision.reason
    );

    let method = match reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) {
        Ok(method) => method,
        Err(_) => {
            return error_response(
                StatusCode::METHOD_NOT_ALLOWED,
                "unsupported_method",
                "Unsupported HTTP method",
            );
        }
    };
    let url = build_upstream_url(&state.config.openai_base_url, &path_and_query);

    let mut upstream = state
        .http_client
        .request(method, url)
        .bearer_auth(&decision.api_key);
    if let Some(org_id) = decision.org_id.as_deref() {
        if !org_id.is_empty() {
            upstream = upstream.header("OpenAI-Organization", org_id);
        }
    }
    for name in FORWARDED_REQUEST_HEADERS.iter() {
        if let Some(value) = parts.headers.get(*name) {
            if let Ok(text) = value.to_str() {
                upstream = upstream.header(*name, text);
            }
        }
    }
    if !body_bytes.is_empty() {
        upstream = upstream.body(body_bytes.to_vec());
    }

    let upstream_response = match upstream.send().await {
        Ok(response) => response,
        Err(e) => {
            warn!("Upstream request failed: {e}");
            state.routing_engine.report_error(decision.account_id, true);
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_unreachable",
                "The upstream API could not be reached",
            );
        }
    };

    let status_code = upstream_response.status().as_u16();
    if is_account_failure(status_code) {
        warn!(
            "Upstream returned {} for account {}",
            status_code, decision.account_label
        );
        state.routing_engine.report_error(decision.account_id, true);
    } else {
        state.routing_engine.report_success(decision.account_id);
    }

    let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = axum::http::Response::builder().status(status);
    for (name, value) in upstream_response.headers().iter() {
        if !should_forward_response_header(name.as_str()) {
            continue;
        }
        if let Ok(text) = value.to_str() {
            builder = builder.header(name.as_str(), text);
        }
    }

    let stream = upstream_response.bytes_stream();
    match builder.body(Body::from_stream(stream)) {
        Ok(response) => response,
        Err(_) => error_response(
            StatusCode::BAD_GATEWAY,
            "invalid_upstream_response",
            "The upstream response could not be forwarded",
        ),
    }
}

/// Build the Axum router. No CORS layer is installed: browsers must not be
/// able to drive the proxy from arbitrary web pages.
pub fn build_router(state: ProxyState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/healthz", get(health_handler))
        .fallback(proxy_handler)
        .with_state(state)
}

/// A running proxy server
pub struct ProxyServer {
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    bound_addr: SocketAddr,
    request_count: Arc<AtomicU64>,
    started_at: Instant,
}

impl ProxyServer {
    /// Bind the listener and start serving in the background
    pub async fn start(
        routing_engine: Arc<RoutingEngine>,
        config: ProxyServerConfig,
    ) -> Result<Self, String> {
        let addr: SocketAddr = config
            .bind_addr
            .parse::<SocketAddr>()
            .map_err(|e| format!("Invalid bind address {}: {}", config.bind_addr, e))?;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("Failed to bind to {addr}: {e}"))?;
        let bound_addr = listener
            .local_addr()
            .map_err(|e| format!("Failed to read the bound address: {e}"))?;

        let state = ProxyState::new(routing_engine, config);
        let request_count = state.request_count.clone();
        let started_at = state.started_at;
        let app = build_router(state);

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
            if let Err(e) = result {
                warn!("Proxy server stopped with an error: {e}");
            }
        });

        info!("Proxy server listening on http://{bound_addr}");
        Ok(Self {
            shutdown_tx: Some(shutdown_tx),
            bound_addr,
            request_count,
            started_at,
        })
    }

    /// Address the server is listening on
    pub fn bound_addr(&self) -> SocketAddr {
        self.bound_addr
    }

    /// Stop the server
    pub fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }

    /// Current status
    pub fn status(&self) -> ProxyStatus {
        ProxyStatus {
            running: self.shutdown_tx.is_some(),
            bind_addr: self.bound_addr.to_string(),
            request_count: self.request_count.load(Ordering::Relaxed),
            uptime_seconds: self.started_at.elapsed().as_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Account, RoutingStrategy};
    use std::collections::HashMap;

    const PROXY_KEY: &str = "proxy-test-key";
    const UPSTREAM_KEY: &str = "sk-upstream-0123456789abcdef";

    fn header_text(headers: &HeaderMap, name: &str) -> String {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    /// Mock upstream API: echoes what it received, with a few special paths.
    async fn mock_upstream(request: Request) -> Response {
        let method = request.method().as_str().to_string();
        let path = request.uri().path().to_string();
        let query = request.uri().query().unwrap_or("").to_string();
        let authorization = header_text(request.headers(), "authorization");
        let organization = header_text(request.headers(), "openai-organization");
        let content_type = header_text(request.headers(), "content-type");

        if path == "/v1/stream" {
            return axum::http::Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .body(Body::from("data: one\n\ndata: [DONE]\n\n"))
                .unwrap();
        }
        if path == "/v1/limited" {
            return axum::http::Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header("content-type", "application/json")
                .header("retry-after", "7")
                .header("set-cookie", "upstream=secret")
                .body(Body::from("{\"error\":{\"message\":\"slow down\"}}"))
                .unwrap();
        }
        if path == "/v1/missing" {
            return axum::http::Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-type", "application/json")
                .body(Body::from("{\"error\":{\"message\":\"no such thing\"}}"))
                .unwrap();
        }

        let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let echoed = serde_json::json!({
            "method": method,
            "path": path,
            "query": query,
            "authorization": authorization,
            "organization": organization,
            "content_type": content_type,
            "body": String::from_utf8_lossy(&body).to_string(),
        });
        let status = if path == "/v1/created" {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        axum::http::Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .header("x-request-id", "req-test")
            .body(Body::from(echoed.to_string()))
            .unwrap()
    }

    async fn spawn_upstream() -> SocketAddr {
        let app = Router::new().fallback(mock_upstream);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        addr
    }

    /// Start a mock upstream and a proxy in front of it. Returns the proxy
    /// and its base URL.
    async fn spawn_proxy() -> (ProxyServer, String) {
        let upstream = spawn_upstream().await;
        let engine = Arc::new(RoutingEngine::new(RoutingStrategy::RoundRobin));
        let account = Account::new("primary".to_string(), UPSTREAM_KEY.to_string())
            .with_org_id("org-test".to_string());
        engine.update_accounts(vec![account], HashMap::new());

        let config = ProxyServerConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            api_key: PROXY_KEY.to_string(),
            openai_base_url: format!("http://{upstream}"),
        };
        let server = ProxyServer::start(engine, config).await.unwrap();
        let base = format!("http://{}", server.bound_addr());
        (server, base)
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    // Regression: every upstream call used to be a POST and the query
    // string was dropped.
    #[tokio::test]
    async fn get_requests_stay_get_and_keep_their_query() {
        let (server, base) = spawn_proxy().await;
        let response = client()
            .get(format!("{base}/v1/models?limit=5&after=abc"))
            .bearer_auth(PROXY_KEY)
            .send()
            .await
            .unwrap();

        assert_eq!(response.status().as_u16(), 200);
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert_eq!(request_id, "req-test");

        let echoed: Value = response.json().await.unwrap();
        assert_eq!(echoed["method"].as_str(), Some("GET"));
        assert_eq!(echoed["path"].as_str(), Some("/v1/models"));
        assert_eq!(echoed["query"].as_str(), Some("limit=5&after=abc"));
        server.stop();
    }

    #[tokio::test]
    async fn post_body_is_forwarded_and_credentials_are_swapped() {
        let (server, base) = spawn_proxy().await;
        let payload = "{\"model\":\"gpt-4o\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}";
        let response = client()
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth(PROXY_KEY)
            .header("content-type", "application/json")
            .body(payload)
            .send()
            .await
            .unwrap();

        assert_eq!(response.status().as_u16(), 200);
        let echoed: Value = response.json().await.unwrap();
        assert_eq!(echoed["method"].as_str(), Some("POST"));
        assert_eq!(echoed["body"].as_str(), Some(payload));
        assert_eq!(echoed["content_type"].as_str(), Some("application/json"));
        // The upstream sees the account key, never the proxy key.
        let expected_auth = format!("Bearer {UPSTREAM_KEY}");
        assert_eq!(echoed["authorization"].as_str(), Some(expected_auth.as_str()));
        assert_eq!(echoed["organization"].as_str(), Some("org-test"));
        assert_eq!(server.status().request_count, 1);
        server.stop();
    }

    #[tokio::test]
    async fn other_methods_are_forwarded() {
        let (server, base) = spawn_proxy().await;
        let response = client()
            .delete(format!("{base}/v1/files/file-123"))
            .bearer_auth(PROXY_KEY)
            .send()
            .await
            .unwrap();
        let echoed: Value = response.json().await.unwrap();
        assert_eq!(echoed["method"].as_str(), Some("DELETE"));
        assert_eq!(echoed["path"].as_str(), Some("/v1/files/file-123"));
        server.stop();
    }

    // Regression: success was always reported as 200 with a fixed content
    // type, and upstream errors lost their headers.
    #[tokio::test]
    async fn upstream_status_and_headers_pass_through() {
        let (server, base) = spawn_proxy().await;

        let created = client()
            .post(format!("{base}/v1/created"))
            .bearer_auth(PROXY_KEY)
            .send()
            .await
            .unwrap();
        assert_eq!(created.status().as_u16(), 201);

        let missing = client()
            .get(format!("{base}/v1/missing"))
            .bearer_auth(PROXY_KEY)
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status().as_u16(), 404);
        let text = missing.text().await.unwrap();
        assert!(text.contains("no such thing"));

        let limited = client()
            .get(format!("{base}/v1/limited"))
            .bearer_auth(PROXY_KEY)
            .send()
            .await
            .unwrap();
        assert_eq!(limited.status().as_u16(), 429);
        assert!(limited.headers().get("retry-after").is_some());
        // Headers outside the allow-list are not passed back.
        assert!(limited.headers().get("set-cookie").is_none());
        server.stop();
    }

    #[tokio::test]
    async fn event_streams_keep_their_content_type_and_body() {
        let (server, base) = spawn_proxy().await;
        let response = client()
            .post(format!("{base}/v1/stream"))
            .bearer_auth(PROXY_KEY)
            .send()
            .await
            .unwrap();

        assert_eq!(response.status().as_u16(), 200);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(content_type.starts_with("text/event-stream"));
        let text = response.text().await.unwrap();
        assert_eq!(text, "data: one\n\ndata: [DONE]\n\n");
        server.stop();
    }

    // Regression: /health used to sit behind the bearer check.
    #[tokio::test]
    async fn health_needs_no_key_and_everything_else_does() {
        let (server, base) = spawn_proxy().await;

        let health = client().get(format!("{base}/health")).send().await.unwrap();
        assert_eq!(health.status().as_u16(), 200);
        let body: Value = health.json().await.unwrap();
        assert_eq!(body["status"].as_str(), Some("ok"));

        let no_key = client()
            .get(format!("{base}/v1/models"))
            .send()
            .await
            .unwrap();
        assert_eq!(no_key.status().as_u16(), 401);

        let wrong_key = client()
            .get(format!("{base}/v1/models"))
            .bearer_auth("not-the-key")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong_key.status().as_u16(), 401);
        assert_eq!(server.status().request_count, 0);
        server.stop();
    }

    // Regression: the proxy answered any origin with permissive CORS headers.
    #[tokio::test]
    async fn cross_origin_preflight_gets_no_cors_headers() {
        let (server, base) = spawn_proxy().await;
        let response = client()
            .request(
                reqwest::Method::OPTIONS,
                format!("{base}/v1/chat/completions"),
            )
            .header("origin", "https://example.invalid")
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "authorization")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status().as_u16(), 401);
        assert!(response
            .headers()
            .get("access-control-allow-origin")
            .is_none());
        server.stop();
    }

    // Regression: 401/403/429 never counted against the account.
    #[tokio::test]
    async fn repeated_rate_limits_take_the_account_out_of_rotation() {
        let (server, base) = spawn_proxy().await;
        for _ in 0..3 {
            let limited = client()
                .get(format!("{base}/v1/limited"))
                .bearer_auth(PROXY_KEY)
                .send()
                .await
                .unwrap();
            assert_eq!(limited.status().as_u16(), 429);
        }

        let after = client()
            .get(format!("{base}/v1/models"))
            .bearer_auth(PROXY_KEY)
            .send()
            .await
            .unwrap();
        assert_eq!(after.status().as_u16(), 503);

        let health = client().get(format!("{base}/health")).send().await.unwrap();
        let body: Value = health.json().await.unwrap();
        assert_eq!(body["status"].as_str(), Some("degraded"));
        server.stop();
    }

    #[tokio::test]
    async fn invalid_bind_address_is_reported() {
        let engine = Arc::new(RoutingEngine::new(RoutingStrategy::RoundRobin));
        let config = ProxyServerConfig {
            bind_addr: "not-an-address".to_string(),
            api_key: PROXY_KEY.to_string(),
            openai_base_url: "http://127.0.0.1:1".to_string(),
        };
        assert!(ProxyServer::start(engine, config).await.is_err());
    }

    #[test]
    fn authorization_requires_the_exact_bearer_key() {
        let mut headers = HeaderMap::new();
        assert!(!is_authorized(&headers, PROXY_KEY));

        headers.insert(header::AUTHORIZATION, "Bearer proxy-test-key".parse().unwrap());
        assert!(is_authorized(&headers, PROXY_KEY));
        assert!(!is_authorized(&headers, "proxy-test-kez"));
        assert!(!is_authorized(&headers, "proxy-test-key-longer"));
        // An empty configured key never authorizes anything.
        assert!(!is_authorized(&headers, ""));

        headers.insert(header::AUTHORIZATION, "Basic proxy-test-key".parse().unwrap());
        assert!(!is_authorized(&headers, PROXY_KEY));
    }

    #[test]
    fn constant_time_eq_compares_content_and_length() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn upstream_url_keeps_the_query_and_never_doubles_v1() {
        assert_eq!(
            build_upstream_url("https://api.openai.com", "/v1/models?limit=5"),
            "https://api.openai.com/v1/models?limit=5"
        );
        assert_eq!(
            build_upstream_url("https://api.openai.com/", "/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            build_upstream_url("https://gateway.example/v1", "/v1/embeddings"),
            "https://gateway.example/v1/embeddings"
        );
        assert_eq!(
            build_upstream_url("https://gateway.example/v1/", "/v1/embeddings"),
            "https://gateway.example/v1/embeddings"
        );
    }

    #[test]
    fn response_header_allow_list() {
        assert!(should_forward_response_header("Content-Type"));
        assert!(should_forward_response_header("retry-after"));
        assert!(should_forward_response_header("x-request-id"));
        assert!(should_forward_response_header("x-ratelimit-remaining-tokens"));
        assert!(should_forward_response_header("openai-processing-ms"));
        assert!(!should_forward_response_header("set-cookie"));
        assert!(!should_forward_response_header("content-length"));
        assert!(!should_forward_response_header("transfer-encoding"));
        assert!(!should_forward_response_header("content-encoding"));
    }

    #[test]
    fn session_id_is_a_stable_hash_of_the_first_message() {
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "You are helpful." },
                { "role": "user", "content": "Hello" }
            ]
        });
        let first = extract_session_id(&body).unwrap();
        let second = extract_session_id(&body).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 16);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));

        let other = serde_json::json!({
            "messages": [{ "role": "system", "content": "Different prompt." }]
        });
        assert_ne!(extract_session_id(&other).unwrap(), first);
    }

    // Regression: a request without messages used to get the session id "".
    #[test]
    fn requests_without_a_first_message_have_no_session() {
        assert!(extract_session_id(&serde_json::json!({})).is_none());
        assert!(extract_session_id(&serde_json::json!({ "messages": [] })).is_none());
        assert!(extract_session_id(&serde_json::json!({ "input": "embed me" })).is_none());
        let empty = serde_json::json!({ "messages": [{ "role": "user", "content": "" }] });
        assert!(extract_session_id(&empty).is_none());
    }
}
