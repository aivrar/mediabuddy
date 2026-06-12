use std::cmp::Reverse;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, OriginalUri, Path as AxumPath, Query, State},
    http::{header, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use uuid::Uuid;

// ---- Request-bound safety limits (REST boundary is untrusted) ----
/// Max request body for any route (bounds batch JSON arrays too).
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
/// Max items accepted in one /download/batch call.
const MAX_BATCH_ITEMS: usize = 1000;
/// Max combo search-download result count.
const MAX_COMBO_LIMIT: usize = 200;
/// Max images accepted or selected for one combo analyze task.
const MAX_ANALYZE_ITEMS: usize = 1000;
/// Max per-source result count (guards provider-quota amplification).
const MAX_PER_SOURCE: u32 = 200;
/// Hard cap on download fan-out (keeps `Semaphore::new` from panicking).
const MAX_CONCURRENCY: usize = 64;

use crate::api_keys::{self, Provider as ApiKeyProvider};
use crate::config::Settings;
use crate::downloader;
use crate::error::panic_message;
use crate::image_manager::ImageManager;
use crate::logbuf::LogBuffer;
use crate::paths::AppPaths;
use crate::quota::QuotaTracker;
use crate::search::{self, Kind as SearchKind, SearchFilters, SearchResult};
use crate::system_monitor::SystemMonitor;
use crate::topics::{self, TopicStore};
use crate::types::Image;
use crate::vision::{
    caption_task_from_name, parse_caption_write_mode, should_write_caption, CaptionWriteMode,
    DetectedObject, Precision, VisionAnalysisOptions, VisionExecutionMode, VisionLoadOptions,
    VisionRegistry,
};

// ---------- Public surface ----------

#[derive(Clone)]
pub struct ApiContext {
    pub paths: Arc<AppPaths>,
    pub settings: Arc<RwLock<Settings>>,
    pub image_manager: Arc<ImageManager>,
    pub http: reqwest::Client,
    /// Redirect-disabled client for caller-supplied media downloads (SSRF).
    pub download_http: reqwest::Client,
    pub system: Arc<SystemMonitor>,
    pub tasks: Arc<TaskTracker>,
    pub started_at: Arc<RwLock<Option<u64>>>,
    pub vision: Arc<VisionRegistry>,
    pub quota: Arc<QuotaTracker>,
    pub topics: Arc<TopicStore>,
    pub log_buffer: LogBuffer,
    /// Per-client-IP request limiter (created per server run).
    pub rate_limiter: Arc<RateLimiter>,
}

/// Simple per-IP token-bucket limiter for the REST boundary. Guards against
/// request floods and provider-quota amplification. Generous by default so
/// the UI status poller and normal tool use are unaffected.
pub struct RateLimiter {
    inner: std::sync::Mutex<HashMap<IpAddr, Bucket>>,
    capacity: f64,
    refill_per_sec: f64,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
            capacity: 120.0,
            refill_per_sec: 20.0,
        }
    }

    /// Returns true if a request from `ip` is allowed (and consumes a token).
    fn allow(&self, ip: IpAddr) -> bool {
        // Poison-tolerant: a panic elsewhere must not wedge the limiter.
        let mut map = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // Bound memory against a flood of distinct source IPs.
        if map.len() > 10_000 {
            map.clear();
        }
        let now = Instant::now();
        let bucket = map.entry(ip).or_insert(Bucket {
            tokens: self.capacity,
            last: now,
        });
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ApiServer {
    handle: Mutex<Option<RunningHandle>>,
    pub tasks: Arc<TaskTracker>,
    pub started_at: Arc<RwLock<Option<u64>>>,
}

struct RunningHandle {
    shutdown: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
    bound_port: u16,
    bound_host: String,
}

#[derive(Serialize, Clone)]
pub struct ApiStatus {
    pub running: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub uptime_seconds: Option<u64>,
}

impl ApiServer {
    pub fn new() -> Self {
        Self {
            handle: Mutex::new(None),
            tasks: Arc::new(TaskTracker::new()),
            started_at: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn status(&self) -> ApiStatus {
        let guard = self.handle.lock().await;
        match guard.as_ref() {
            Some(h) => {
                let started = *self.started_at.read().await;
                let uptime = started.map(|s| now_secs().saturating_sub(s));
                ApiStatus {
                    running: true,
                    host: Some(h.bound_host.clone()),
                    port: Some(h.bound_port),
                    uptime_seconds: uptime,
                }
            }
            None => ApiStatus {
                running: false,
                host: None,
                port: None,
                uptime_seconds: None,
            },
        }
    }

    pub async fn start(
        &self,
        host: String,
        port: u16,
        cors_enabled: bool,
        ctx: ApiContext,
    ) -> Result<u16, String> {
        let mut guard = self.handle.lock().await;
        if guard.is_some() {
            return Err("API server already running".into());
        }

        let addr: SocketAddr = format!("{host}:{port}")
            .parse()
            .map_err(|e: std::net::AddrParseError| e.to_string())?;

        // Refuse to expose the control API to the network without a token.
        if !addr.ip().is_loopback() && ctx.settings.read().await.api_token.trim().is_empty() {
            return Err(
                "Refusing to bind a non-loopback address with an empty API token. \
                 Set an API token in Settings before exposing the server."
                    .into(),
            );
        }
        let bound_loopback = addr.ip().is_loopback();

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind {addr}: {e}"))?;
        let bound_addr = listener.local_addr().map_err(|e| e.to_string())?;
        let bound_port = bound_addr.port();

        let router = build_router(ctx, cors_enabled, bound_loopback);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let started_at = self.started_at.clone();
        *started_at.write().await = Some(now_secs());

        let join = tokio::spawn(async move {
            let server = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            });
            if let Err(err) = server.await {
                tracing::error!("API server error: {err}");
            }
            *started_at.write().await = None;
        });

        *guard = Some(RunningHandle {
            shutdown: shutdown_tx,
            join,
            bound_port,
            bound_host: host,
        });

        Ok(bound_port)
    }

    pub async fn stop(&self) -> Result<(), String> {
        let mut guard = self.handle.lock().await;
        let Some(handle) = guard.take() else {
            return Err("API server not running".into());
        };
        let _ = handle.shutdown.send(());
        let _ = handle.join.await;
        Ok(())
    }
}

// ---------- Task tracker ----------

#[derive(Debug, Clone, Serialize)]
pub struct TaskInfo {
    pub id: String,
    pub task_type: String,
    pub status: String, // queued / running / completed / failed
    pub progress: u32,
    pub total: u32,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub error: Option<String>,
    pub result: Option<Value>,
}

pub struct TaskTracker {
    inner: tokio::sync::Mutex<HashMap<String, TaskInfo>>,
}

impl TaskTracker {
    pub fn new() -> Self {
        Self {
            inner: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub async fn create(&self, task_type: &str, total: u32) -> String {
        let id = Uuid::new_v4().to_string();
        let info = TaskInfo {
            id: id.clone(),
            task_type: task_type.to_string(),
            status: "queued".into(),
            progress: 0,
            total,
            started_at: now_secs(),
            completed_at: None,
            error: None,
            result: None,
        };
        self.inner.lock().await.insert(id.clone(), info);
        id
    }

    pub async fn set_running(&self, id: &str) {
        if let Some(t) = self.inner.lock().await.get_mut(id) {
            t.status = "running".into();
        }
    }

    pub async fn bump(&self, id: &str, by: u32) {
        if let Some(t) = self.inner.lock().await.get_mut(id) {
            t.progress = t.progress.saturating_add(by);
        }
    }

    pub async fn complete(&self, id: &str, result: Option<Value>) {
        if let Some(t) = self.inner.lock().await.get_mut(id) {
            t.status = "completed".into();
            t.completed_at = Some(now_secs());
            t.result = result;
        }
    }

    pub async fn fail(&self, id: &str, error: String) {
        if let Some(t) = self.inner.lock().await.get_mut(id) {
            t.status = "failed".into();
            t.completed_at = Some(now_secs());
            t.error = Some(error);
        }
    }

    pub async fn get(&self, id: &str) -> Option<TaskInfo> {
        self.inner.lock().await.get(id).cloned()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------- Response helpers ----------

fn ok<T: Serialize>(data: T) -> Response {
    Json(json!({ "success": true, "data": data })).into_response()
}

fn err(code: StatusCode, message: impl Into<String>) -> Response {
    (
        code,
        Json(json!({ "success": false, "error": message.into() })),
    )
        .into_response()
}

/// Log full error detail server-side and return a generic message to the
/// client, so internal paths / SQL / provider URLs aren't leaked over the
/// (potentially network-exposed) REST boundary.
fn internal(detail: impl std::fmt::Display) -> Response {
    tracing::error!("API internal error: {detail}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}

/// Add baseline security headers to every response.
async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp
}

/// Reject requests whose `Host` header is not a loopback name. Only applied
/// when the server is bound to loopback, to defeat DNS-rebinding (a rebinding
/// attack sends the attacker's own domain in `Host`).
async fn host_guard(req: Request<Body>, next: Next) -> Response {
    if host_is_loopback(&req) {
        next.run(req).await
    } else {
        err(StatusCode::FORBIDDEN, "Invalid Host header")
    }
}

fn host_is_loopback(req: &Request<Body>) -> bool {
    let Some(host) = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
    else {
        // No Host header (e.g. HTTP/2 :authority) — don't block.
        return true;
    };
    let host_only = strip_port(host.trim());
    host_only.is_empty() || matches!(host_only, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// Strip an optional `:port` suffix, correctly handling bracketed IPv6.
fn strip_port(host: &str) -> &str {
    if host.starts_with('[') {
        return match host.find(']') {
            Some(end) => &host[..=end], // keep "[..]"
            None => host,
        };
    }
    // Unbracketed IPv6 literal (e.g. "::1") has multiple colons and, per spec,
    // cannot carry a port without brackets — so never treat its tail as one.
    if host.matches(':').count() > 1 {
        return host;
    }
    match host.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    }
}

/// True if a CORS `Origin` header points at a loopback host.
fn origin_is_local(origin: &HeaderValue) -> bool {
    let Ok(s) = origin.to_str() else {
        return false;
    };
    let after_scheme = match s.split_once("://") {
        Some((_, rest)) => rest,
        None => return false,
    };
    let host_port = after_scheme.split('/').next().unwrap_or("");
    matches!(
        strip_port(host_port),
        "127.0.0.1" | "localhost" | "::1" | "[::1]"
    )
}

// ---------- Router ----------

/// Per-IP rate-limit middleware. Applied to the whole router.
async fn rate_limit_middleware(
    State(ctx): State<ApiContext>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if ctx.rate_limiter.allow(addr.ip()) {
        next.run(req).await
    } else {
        err(StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded")
    }
}

async fn auth_middleware(
    State(ctx): State<ApiContext>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let token = ctx.settings.read().await.api_token.trim().to_string();
    if token.is_empty() {
        // No token configured. This mode is only appropriate for local-only
        // development; `start` rejects non-loopback binds without a token.
        return next.run(req).await;
    }
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .or_else(|| {
            req.headers()
                .get("x-api-token")
                .and_then(|v| v.to_str().ok())
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if !provided.is_empty() && constant_time_eq(provided.as_bytes(), token.as_bytes()) {
        return next.run(req).await;
    }
    err(StatusCode::UNAUTHORIZED, "Missing or invalid API token")
}

/// Constant-time comparison so a remote attacker can't time the prefix.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn build_router(ctx: ApiContext, cors_enabled: bool, bound_loopback: bool) -> Router {
    let rl_ctx = ctx.clone();
    // Status endpoint is intentionally outside auth so the UI's status
    // poller can probe a server-with-token-set without hitting 401.
    let public = Router::new()
        .route("/api/v1/status", get(handle_status))
        .with_state(ctx.clone());

    let router = Router::new()
        // Stats (auth'd; reveals library size)
        .route("/api/v1/stats", get(handle_stats))
        .route("/api/v1/docs", get(handle_api_docs))
        .route("/api/v1/openapi.json", get(handle_openapi))
        .route(
            "/api/v1/settings",
            get(handle_get_settings).put(handle_update_settings),
        )
        .route("/api/v1/api-keys/validate", post(handle_validate_api_key))
        .route("/api/v1/quota", get(handle_quota))
        .route("/api/v1/logs", get(handle_logs).delete(handle_clear_logs))
        .route("/api/v1/app/shutdown", post(handle_shutdown))
        // Images CRUD
        .route("/api/v1/images", get(handle_list_images))
        .route(
            "/api/v1/images/{image_id}",
            get(handle_get_image)
                .delete(handle_delete_image)
                .put(handle_update_image),
        )
        .route("/api/v1/images/delete", post(handle_batch_delete))
        .route("/api/v1/images/{image_id}/file", get(handle_image_file))
        .route("/api/v1/images/{image_id}/thumb", get(handle_image_thumb))
        .route("/api/v1/images/query", post(handle_query_images))
        // Search
        .route("/api/v1/search", post(handle_search_all))
        .route("/api/v1/search/pixabay", get(handle_search_one_source))
        .route("/api/v1/search/pexels", get(handle_search_one_source))
        .route("/api/v1/search/unsplash", get(handle_search_one_source))
        // Download
        .route("/api/v1/download", post(handle_download_single))
        .route("/api/v1/download/batch", post(handle_download_batch))
        .route("/api/v1/tasks/{task_id}", get(handle_task_status))
        // Topics: persistent search cursors for repeat asset harvesting
        .route(
            "/api/v1/topics",
            get(handle_list_topics).post(handle_create_topic),
        )
        .route(
            "/api/v1/topics/{topic_id}",
            get(handle_topic_status)
                .delete(handle_topic_delete)
                .put(handle_topic_rename),
        )
        .route(
            "/api/v1/topics/{topic_id}/more",
            post(handle_topic_get_more),
        )
        .route("/api/v1/topics/{topic_id}/reset", post(handle_topic_reset))
        .route(
            "/api/v1/topics/{topic_id}/images",
            get(handle_topic_image_ids),
        )
        // Vision
        .route("/api/v1/vision/status", get(handle_vision_status))
        .route("/api/v1/vision/load", post(handle_vision_load))
        .route("/api/v1/vision/unload", post(handle_vision_unload))
        .route(
            "/api/v1/vision/analyze/{image_id}",
            post(handle_vision_analyze_image),
        )
        .route("/api/v1/vision/analyze", post(handle_vision_analyze_path))
        // Combo
        .route(
            "/api/v1/combo/search-download",
            post(handle_combo_search_download),
        )
        .route(
            "/api/v1/combo/download-analyze",
            post(handle_combo_download_analyze),
        )
        .route(
            "/api/v1/combo/analyze-unprocessed",
            post(handle_combo_analyze_unprocessed),
        )
        .route(
            "/api/v1/combo/smart-analyze",
            post(handle_combo_smart_analyze),
        )
        .route(
            "/api/v1/combo/search-download-analyze",
            post(handle_combo_search_download_analyze),
        )
        .layer(middleware::from_fn_with_state(ctx.clone(), auth_middleware))
        .with_state(ctx);

    let mut merged = public.merge(router);

    // Bound request body size (also caps batch JSON arrays).
    merged = merged.layer(DefaultBodyLimit::max(MAX_BODY_BYTES));

    // Per-IP rate limiting.
    merged = merged.layer(middleware::from_fn_with_state(
        rl_ctx,
        rate_limit_middleware,
    ));

    // Baseline security headers on every response.
    merged = merged.layer(middleware::from_fn(security_headers));

    // Anti DNS-rebinding: when bound to loopback, require a loopback Host.
    if bound_loopback {
        merged = merged.layer(middleware::from_fn(host_guard));
    }

    if cors_enabled {
        // Never `Any` — only reflect loopback origins so an arbitrary web
        // page can't read responses cross-origin.
        merged = merged.layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::predicate(|origin, _parts| {
                    origin_is_local(origin)
                }))
                .allow_methods(Any)
                .allow_headers(Any),
        );
    }
    merged
}

// ---------- Handlers ----------

async fn handle_status(State(ctx): State<ApiContext>) -> Response {
    let started = *ctx.started_at.read().await;
    let uptime = started.map(|s| now_secs().saturating_sub(s)).unwrap_or(0);
    ok(json!({
        "status": "running",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": uptime,
    }))
}

async fn handle_stats(State(ctx): State<ApiContext>) -> Response {
    let mgr = ctx.image_manager.clone();
    let images_res = tokio::task::spawn_blocking(move || mgr.get_all_images()).await;
    let images = match images_res {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return internal(e),
        Err(e) => return internal(e),
    };

    let mut by_source: HashMap<String, u32> = HashMap::new();
    let mut by_query: HashMap<String, u32> = HashMap::new();
    let mut vision_count: u32 = 0;
    for img in &images {
        *by_source.entry(img.source.clone()).or_default() += 1;
        *by_query.entry(img.query.clone()).or_default() += 1;
        if img.vision_processed {
            vision_count += 1;
        }
    }

    let originals_size = dir_total_size(&ctx.paths.originals).await;
    let thumbs_size = dir_total_size(&ctx.paths.thumbs).await;

    let mut top_queries: Vec<(String, u32)> = by_query.into_iter().collect();
    top_queries.sort_by_key(|item| Reverse(item.1));
    top_queries.truncate(20);

    ok(json!({
        "total_images": images.len(),
        "by_source": by_source,
        "by_query": top_queries.into_iter().collect::<HashMap<_,_>>(),
        "vision_processed": vision_count,
        "disk_usage": {
            "originals_mb": (originals_size as f64) / (1024.0 * 1024.0),
            "thumbs_mb": (thumbs_size as f64) / (1024.0 * 1024.0),
            "total_mb": ((originals_size + thumbs_size) as f64) / (1024.0 * 1024.0),
        }
    }))
}

#[derive(Debug, Clone, Serialize)]
struct ApiRouteDoc {
    method: &'static str,
    path: &'static str,
    description: &'static str,
    auth_required: bool,
    example_body: Option<Value>,
}

fn api_route_docs() -> Vec<ApiRouteDoc> {
    vec![
        ApiRouteDoc {
            method: "GET",
            path: "/api/v1/status",
            description: "Health check. The only unauthenticated endpoint.",
            auth_required: false,
            example_body: None,
        },
        ApiRouteDoc {
            method: "GET",
            path: "/api/v1/stats",
            description: "Library counts and disk usage.",
            auth_required: true,
            example_body: None,
        },
        ApiRouteDoc {
            method: "GET",
            path: "/api/v1/docs",
            description: "Machine-readable API route summary and examples.",
            auth_required: true,
            example_body: None,
        },
        ApiRouteDoc {
            method: "GET",
            path: "/api/v1/openapi.json",
            description: "Minimal OpenAPI 3 path document.",
            auth_required: true,
            example_body: None,
        },
        ApiRouteDoc {
            method: "GET",
            path: "/api/v1/settings",
            description: "Read redacted app settings and key configured state.",
            auth_required: true,
            example_body: None,
        },
        ApiRouteDoc {
            method: "PUT",
            path: "/api/v1/settings",
            description: "Patch settings, including provider keys when fields are supplied.",
            auth_required: true,
            example_body: Some(json!({"api_auto_start": true, "unsplash_detail_threshold": 30})),
        },
        ApiRouteDoc {
            method: "POST",
            path: "/api/v1/api-keys/validate",
            description: "Validate a provider key, optionally saving it when valid.",
            auth_required: true,
            example_body: Some(json!({"provider": "pexels", "key": "YOUR_KEY", "save": true})),
        },
        ApiRouteDoc {
            method: "GET",
            path: "/api/v1/quota",
            description: "Provider quota snapshot from observed provider responses.",
            auth_required: true,
            example_body: None,
        },
        ApiRouteDoc {
            method: "GET",
            path: "/api/v1/logs?level=INFO",
            description: "Read in-memory app logs.",
            auth_required: true,
            example_body: None,
        },
        ApiRouteDoc {
            method: "DELETE",
            path: "/api/v1/logs",
            description: "Clear in-memory app logs.",
            auth_required: true,
            example_body: None,
        },
        ApiRouteDoc {
            method: "POST",
            path: "/api/v1/app/shutdown",
            description: "Shut down the desktop app process.",
            auth_required: true,
            example_body: Some(json!({})),
        },
        ApiRouteDoc {
            method: "GET",
            path: "/api/v1/images",
            description: "List media with pagination and simple filters.",
            auth_required: true,
            example_body: None,
        },
        ApiRouteDoc {
            method: "POST",
            path: "/api/v1/images/query",
            description: "Advanced library query with filters, sort, and pagination.",
            auth_required: true,
            example_body: Some(
                json!({"filters": {"kind": "photo"}, "sort": {"field": "downloaded_at", "order": "desc"}, "pagination": {"page": 1, "per_page": 50}}),
            ),
        },
        ApiRouteDoc {
            method: "POST",
            path: "/api/v1/search",
            description: "Search all enabled providers.",
            auth_required: true,
            example_body: Some(
                json!({"query": "manta ray", "kind": "photo", "sources": {"pixabay": 1, "pexels": 1, "unsplash": 1}}),
            ),
        },
        ApiRouteDoc {
            method: "POST",
            path: "/api/v1/download/batch",
            description: "Download normalized search results as an async task.",
            auth_required: true,
            example_body: Some(json!({"items": [], "concurrency": 8})),
        },
        ApiRouteDoc {
            method: "GET",
            path: "/api/v1/tasks/{task_id}",
            description: "Read async task progress and results.",
            auth_required: true,
            example_body: None,
        },
        ApiRouteDoc {
            method: "GET",
            path: "/api/v1/topics",
            description: "List persistent search topics.",
            auth_required: true,
            example_body: None,
        },
        ApiRouteDoc {
            method: "POST",
            path: "/api/v1/vision/load",
            description: "Load Florence-2 workers.",
            auth_required: true,
            example_body: Some(
                json!({"mode": "auto", "gpu_instances_per_gpu": 1, "max_total_instances": 2}),
            ),
        },
        ApiRouteDoc {
            method: "POST",
            path: "/api/v1/vision/analyze/{image_id}",
            description: "Analyze one library image using caption policy and object detection.",
            auth_required: true,
            example_body: Some(
                json!({"detect_objects": true, "caption_mode": "missing", "caption_task": "detailed", "caption_min_chars": 80}),
            ),
        },
        ApiRouteDoc {
            method: "POST",
            path: "/api/v1/combo/search-download-analyze",
            description: "Search, download originals, then analyze saved images.",
            auth_required: true,
            example_body: Some(
                json!({"query": "london", "kind": "photo", "limit": 10, "detect_objects": true, "caption_mode": "short"}),
            ),
        },
    ]
}

async fn handle_api_docs() -> Response {
    ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "auth": {
            "required": "All /api/v1 endpoints except /api/v1/status require the REST API token when one is configured.",
            "headers": [
                "Authorization: Bearer <token>",
                "x-api-token: <token>"
            ]
        },
        "response_shape": {
            "success": true,
            "data": {},
            "error": null
        },
        "limits": {
            "body_bytes": MAX_BODY_BYTES,
            "batch_items": MAX_BATCH_ITEMS,
            "combo_limit": MAX_COMBO_LIMIT,
            "analyze_items": MAX_ANALYZE_ITEMS,
            "concurrency": MAX_CONCURRENCY,
            "per_source": MAX_PER_SOURCE
        },
        "routes": api_route_docs(),
    }))
}

async fn handle_openapi() -> Response {
    let mut paths = serde_json::Map::new();
    for route in api_route_docs() {
        let path = paths
            .entry(route.path.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(methods) = path.as_object_mut() {
            methods.insert(
                route.method.to_ascii_lowercase(),
                json!({
                    "summary": route.description,
                    "security": if route.auth_required { json!([{ "bearerAuth": [] }, { "apiToken": [] }]) } else { json!([]) },
                    "responses": {
                        "200": { "description": "OK" },
                        "400": { "description": "Bad request" },
                        "401": { "description": "Missing or invalid API token" },
                        "500": { "description": "Internal server error" }
                    }
                }),
            );
        }
    }
    Json(json!({
        "openapi": "3.0.3",
        "info": {
            "title": "Media Buddy REST API",
            "version": env!("CARGO_PKG_VERSION")
        },
        "components": {
            "securitySchemes": {
                "bearerAuth": { "type": "http", "scheme": "bearer" },
                "apiToken": { "type": "apiKey", "in": "header", "name": "x-api-token" }
            }
        },
        "paths": Value::Object(paths),
    }))
    .into_response()
}

fn secret_state(value: &str) -> Value {
    let trimmed = value.trim();
    let last4 = {
        let mut chars: Vec<char> = trimmed.chars().rev().take(4).collect();
        chars.reverse();
        (!chars.is_empty() && trimmed.chars().count() >= 4)
            .then(|| chars.into_iter().collect::<String>())
    };
    json!({
        "configured": !trimmed.is_empty(),
        "last4": last4
    })
}

fn settings_view(settings: &Settings) -> Value {
    json!({
        "keys": {
            "pixabay": secret_state(&settings.pixabay_key),
            "pexels": secret_state(&settings.pexels_key),
            "unsplash": secret_state(&settings.unsplash_key),
            "api_token": secret_state(&settings.api_token),
        },
        "theme": settings.theme,
        "api": {
            "host": settings.api_host,
            "port": settings.api_port,
            "auto_start": settings.api_auto_start,
            "cors_enabled": settings.api_cors_enabled,
            "auth_header": "Authorization: Bearer <token>",
            "alternate_auth_header": "x-api-token: <token>",
        },
        "vision": {
            "auto_load": settings.vision_auto_load,
            "auto_unload": settings.vision_auto_unload,
            "allow_cpu": settings.vision_allow_cpu,
            "execution_mode": settings.vision_execution_mode,
            "cpu_instances": settings.vision_cpu_instances,
            "cpu_threads_per_instance": settings.vision_cpu_threads_per_instance,
            "max_per_gpu": settings.vision_max_per_gpu,
            "max_total": settings.vision_max_total,
            "reserved_vram": settings.vision_reserved_vram,
        },
        "search": {
            "unsplash_detail_threshold": settings.unsplash_detail_threshold,
        }
    })
}

async fn handle_get_settings(State(ctx): State<ApiContext>) -> Response {
    let settings = ctx.settings.read().await.clone();
    ok(settings_view(&settings))
}

#[derive(Deserialize, Default)]
struct SettingsPatch {
    #[serde(default)]
    pixabay_key: Option<String>,
    #[serde(default)]
    pexels_key: Option<String>,
    #[serde(default)]
    unsplash_key: Option<String>,
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    vision_auto_load: Option<bool>,
    #[serde(default)]
    vision_auto_unload: Option<bool>,
    #[serde(default)]
    vision_allow_cpu: Option<bool>,
    #[serde(default)]
    vision_execution_mode: Option<String>,
    #[serde(default)]
    vision_cpu_instances: Option<u32>,
    #[serde(default)]
    vision_cpu_threads_per_instance: Option<u32>,
    #[serde(default)]
    vision_max_per_gpu: Option<u32>,
    #[serde(default)]
    vision_max_total: Option<u32>,
    #[serde(default)]
    vision_reserved_vram: Option<f32>,
    #[serde(default)]
    api_host: Option<String>,
    #[serde(default)]
    api_port: Option<u16>,
    #[serde(default)]
    api_auto_start: Option<bool>,
    #[serde(default)]
    api_cors_enabled: Option<bool>,
    #[serde(default)]
    unsplash_detail_threshold: Option<u32>,
    #[serde(default)]
    api_token: Option<String>,
}

async fn handle_update_settings(
    State(ctx): State<ApiContext>,
    Json(body): Json<SettingsPatch>,
) -> Response {
    let mut next = ctx.settings.read().await.clone();
    if let Some(v) = body.pixabay_key {
        next.pixabay_key = v;
    }
    if let Some(v) = body.pexels_key {
        next.pexels_key = v;
    }
    if let Some(v) = body.unsplash_key {
        next.unsplash_key = v;
    }
    if let Some(v) = body.theme {
        next.theme = v;
    }
    if let Some(v) = body.vision_auto_load {
        next.vision_auto_load = v;
    }
    if let Some(v) = body.vision_auto_unload {
        next.vision_auto_unload = v;
    }
    if let Some(v) = body.vision_allow_cpu {
        next.vision_allow_cpu = v;
    }
    if let Some(v) = body.vision_execution_mode {
        next.vision_execution_mode = v;
    }
    if let Some(v) = body.vision_cpu_instances {
        next.vision_cpu_instances = v.clamp(1, 64);
    }
    if let Some(v) = body.vision_cpu_threads_per_instance {
        next.vision_cpu_threads_per_instance = v.min(256);
    }
    if let Some(v) = body.vision_max_per_gpu {
        next.vision_max_per_gpu = v.clamp(1, 64);
    }
    if let Some(v) = body.vision_max_total {
        next.vision_max_total = v.clamp(1, 128);
    }
    if let Some(v) = body.vision_reserved_vram {
        next.vision_reserved_vram = v.clamp(0.0, 256.0);
    }
    if let Some(v) = body.api_host {
        next.api_host = v;
    }
    if let Some(v) = body.api_port {
        next.api_port = v;
    }
    if let Some(v) = body.api_auto_start {
        next.api_auto_start = v;
    }
    if let Some(v) = body.api_cors_enabled {
        next.api_cors_enabled = v;
    }
    if let Some(v) = body.unsplash_detail_threshold {
        next.unsplash_detail_threshold = v.clamp(1, 200);
    }
    if let Some(v) = body.api_token {
        next.api_token = v;
    }

    if let Err(e) = next.save(&ctx.paths.config_file) {
        return internal(e);
    }
    *ctx.settings.write().await = next.clone();
    ok(settings_view(&next))
}

#[derive(Deserialize)]
struct ValidateKeyBody {
    provider: ApiKeyProvider,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    save: bool,
}

async fn handle_validate_api_key(
    State(ctx): State<ApiContext>,
    Json(body): Json<ValidateKeyBody>,
) -> Response {
    let stored = ctx.settings.read().await.clone();
    let key = body.key.unwrap_or_else(|| match body.provider {
        ApiKeyProvider::Pixabay => stored.pixabay_key.clone(),
        ApiKeyProvider::Pexels => stored.pexels_key.clone(),
        ApiKeyProvider::Unsplash => stored.unsplash_key.clone(),
    });
    let probe = api_keys::probe(&ctx.http, body.provider, &key, Some(&ctx.quota)).await;
    if body.save && probe.valid {
        let mut next = ctx.settings.read().await.clone();
        match body.provider {
            ApiKeyProvider::Pixabay => next.pixabay_key = key,
            ApiKeyProvider::Pexels => next.pexels_key = key,
            ApiKeyProvider::Unsplash => next.unsplash_key = key,
        }
        if let Err(e) = next.save(&ctx.paths.config_file) {
            return internal(e);
        }
        *ctx.settings.write().await = next;
    }
    ok(json!({
        "probe": probe,
        "saved": body.save && probe.valid,
    }))
}

async fn handle_quota(State(ctx): State<ApiContext>) -> Response {
    ok(ctx.quota.snapshot())
}

#[derive(Deserialize, Default)]
struct LogsQuery {
    #[serde(default)]
    since: Option<u64>,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn handle_logs(State(ctx): State<ApiContext>, Query(q): Query<LogsQuery>) -> Response {
    let level = q.level.as_deref().unwrap_or("INFO");
    let mut entries = ctx.log_buffer.snapshot(q.since, level);
    let total = entries.len();
    if let Some(limit) = q.limit.map(|n| n.clamp(1, 5000)) {
        if entries.len() > limit {
            entries = entries[entries.len() - limit..].to_vec();
        }
    }
    ok(json!({
        "entries": entries,
        "count": entries.len(),
        "total_before_limit": total,
    }))
}

async fn handle_clear_logs(State(ctx): State<ApiContext>) -> Response {
    ctx.log_buffer.clear();
    ok(json!({ "cleared": true }))
}

async fn handle_shutdown() -> Response {
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        crate::shutdown_app_process();
    });
    ok(json!({ "shutting_down": true }))
}

async fn dir_total_size(dir: &std::path::Path) -> u64 {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut total: u64 = 0;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                if let Ok(meta) = e.metadata() {
                    if meta.is_file() {
                        total = total.saturating_add(meta.len());
                    }
                }
            }
        }
        total
    })
    .await
    .unwrap_or(0)
}

fn normalize_kind_filter(raw: &str) -> Vec<String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "all" | "any" | "both" | "media" => vec!["photo".into(), "video".into()],
        "photos" | "image" | "images" | "picture" | "pictures" => vec!["photo".into()],
        "videos" | "clip" | "clips" => vec!["video".into()],
        other => vec![other.to_string()],
    }
}

#[derive(Deserialize)]
struct ListImagesQuery {
    page: Option<u32>,
    per_page: Option<u32>,
    source: Option<String>,
    kind: Option<String>,
    query: Option<String>,
    vision_processed: Option<String>,
}

fn image_api_view(ctx: &ApiContext, image: &Image) -> Value {
    let mut value = serde_json::to_value(image).unwrap_or_else(|_| json!({}));
    let Some(obj) = value.as_object_mut() else {
        return value;
    };

    if !image.path.trim().is_empty() {
        let absolute = ctx.paths.root.join(&image.path);
        obj.insert(
            "absolute_path".into(),
            Value::String(absolute.to_string_lossy().to_string()),
        );
        obj.insert(
            "local_path".into(),
            Value::String(absolute.to_string_lossy().to_string()),
        );
        obj.insert(
            "file_url".into(),
            Value::String(format!("/api/v1/images/{}/file", image.id)),
        );
    }
    if !image.thumb_path.trim().is_empty() {
        let absolute = ctx.paths.root.join(&image.thumb_path);
        obj.insert(
            "absolute_thumb_path".into(),
            Value::String(absolute.to_string_lossy().to_string()),
        );
        obj.insert(
            "thumb_url".into(),
            Value::String(format!("/api/v1/images/{}/thumb", image.id)),
        );
    }

    value
}

async fn handle_list_images(
    State(ctx): State<ApiContext>,
    Query(q): Query<ListImagesQuery>,
) -> Response {
    let mgr = ctx.image_manager.clone();
    let images = match tokio::task::spawn_blocking(move || mgr.get_all_images()).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return internal(e),
        Err(e) => return internal(e),
    };

    let mut filtered: Vec<Image> = images;
    if let Some(s) = q.source.as_deref() {
        if !s.is_empty() {
            let sl = s.to_lowercase();
            filtered.retain(|i| i.source.to_lowercase() == sl);
        }
    }
    if let Some(k) = q.kind.as_deref() {
        let kinds = normalize_kind_filter(k);
        if !kinds.is_empty() {
            filtered.retain(|i| kinds.iter().any(|kind| i.kind.eq_ignore_ascii_case(kind)));
        }
    }
    if let Some(qf) = q.query.as_deref() {
        if !qf.is_empty() {
            let qfl = qf.to_lowercase();
            filtered.retain(|i| i.query.to_lowercase().contains(&qfl));
        }
    }
    if let Some(vp) = q.vision_processed.as_deref() {
        if !vp.is_empty() {
            let want = vp.eq_ignore_ascii_case("true");
            filtered.retain(|i| i.vision_processed == want);
        }
    }

    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(50).clamp(1, 500);
    let total = filtered.len() as u32;
    let total_pages = total.div_ceil(per_page).max(1);
    // Compute the offset in usize to avoid a u32 multiplication overflow
    // (debug panic / release wrap) on a large attacker-supplied page number.
    let start = (page as usize - 1) * per_page as usize;
    let end = (start + per_page as usize).min(filtered.len());
    let slice = if start < filtered.len() {
        filtered[start..end].to_vec()
    } else {
        Vec::new()
    };
    let images: Vec<Value> = slice
        .iter()
        .map(|image| image_api_view(&ctx, image))
        .collect();

    ok(json!({
        "images": images,
        "total": total,
        "page": page,
        "per_page": per_page,
        "total_pages": total_pages,
    }))
}

async fn handle_get_image(
    State(ctx): State<ApiContext>,
    AxumPath(image_id): AxumPath<String>,
) -> Response {
    let mgr = ctx.image_manager.clone();
    let image = match tokio::task::spawn_blocking(move || mgr.get_image_by_id(&image_id)).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return internal(e),
        Err(e) => return internal(e),
    };
    match image {
        Some(img) => ok(image_api_view(&ctx, &img)),
        None => err(StatusCode::NOT_FOUND, "Image not found"),
    }
}

async fn handle_delete_image(
    State(ctx): State<ApiContext>,
    AxumPath(image_id): AxumPath<String>,
) -> Response {
    let mgr = ctx.image_manager.clone();
    let id_clone = image_id.clone();
    let result = match tokio::task::spawn_blocking(move || mgr.delete_images(&[id_clone])).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return internal(e),
        Err(e) => return internal(e),
    };
    if result.deleted == 0 {
        return err(StatusCode::NOT_FOUND, "Image not found");
    }
    ok(json!({ "deleted": result.deleted }))
}

#[derive(Deserialize)]
struct UpdateImageBody {
    #[serde(default)]
    alt: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

async fn handle_update_image(
    State(ctx): State<ApiContext>,
    AxumPath(image_id): AxumPath<String>,
    Json(body): Json<UpdateImageBody>,
) -> Response {
    if body.alt.is_none() && body.tags.is_none() {
        return err(StatusCode::BAD_REQUEST, "No updates provided");
    }
    let mgr = ctx.image_manager.clone();
    let id_for_update = image_id.clone();
    let updated_res = tokio::task::spawn_blocking(move || {
        mgr.update_image_metadata(&id_for_update, body.alt, body.tags, None)
    })
    .await;
    let updated = match updated_res {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return internal(e),
        Err(e) => return internal(e),
    };
    if !updated {
        return err(StatusCode::NOT_FOUND, "Image not found");
    }
    let mgr = ctx.image_manager.clone();
    let id_for_get = image_id.clone();
    match tokio::task::spawn_blocking(move || mgr.get_image_by_id(&id_for_get)).await {
        Ok(Ok(Some(img))) => ok(img),
        Ok(Ok(None)) => err(StatusCode::NOT_FOUND, "Image not found"),
        _ => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to fetch updated image",
        ),
    }
}

#[derive(Deserialize)]
struct BatchDeleteBody {
    ids: Vec<String>,
}

async fn handle_batch_delete(
    State(ctx): State<ApiContext>,
    Json(body): Json<BatchDeleteBody>,
) -> Response {
    if body.ids.is_empty() {
        return err(StatusCode::BAD_REQUEST, "No image IDs provided");
    }
    let mgr = ctx.image_manager.clone();
    let result = match tokio::task::spawn_blocking(move || mgr.delete_images(&body.ids)).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return internal(e),
        Err(e) => return internal(e),
    };
    ok(result)
}

async fn handle_image_file(
    State(ctx): State<ApiContext>,
    AxumPath(image_id): AxumPath<String>,
) -> Response {
    serve_image_file(&ctx, &image_id, false).await
}

async fn handle_image_thumb(
    State(ctx): State<ApiContext>,
    AxumPath(image_id): AxumPath<String>,
) -> Response {
    serve_image_file(&ctx, &image_id, true).await
}

async fn serve_image_file(ctx: &ApiContext, image_id: &str, thumb_only: bool) -> Response {
    let mgr = ctx.image_manager.clone();
    let id = image_id.to_string();
    let image = match tokio::task::spawn_blocking(move || mgr.get_image_by_id(&id)).await {
        Ok(Ok(Some(v))) => v,
        Ok(Ok(None)) => return err(StatusCode::NOT_FOUND, "Image not found"),
        Ok(Err(e)) => return internal(e),
        Err(e) => return internal(e),
    };
    let rel = if thumb_only {
        image.thumb_path.clone()
    } else if !image.path.is_empty() {
        image.path.clone()
    } else {
        image.thumb_path.clone()
    };
    if rel.is_empty() {
        return err(StatusCode::NOT_FOUND, "No file for image");
    }
    let full = ctx.paths.root.join(&rel);
    let bytes = match tokio::fs::read(&full).await {
        Ok(b) => b,
        Err(_) => return err(StatusCode::NOT_FOUND, "File not on disk"),
    };
    let mime = mime_for(&rel);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Response build failed"))
}

fn mime_for(rel: &str) -> &'static str {
    let lower = rel.to_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".mp4") || lower.ends_with(".m4v") {
        "video/mp4"
    } else if lower.ends_with(".webm") {
        "video/webm"
    } else if lower.ends_with(".mov") {
        "video/quicktime"
    } else {
        "image/jpeg"
    }
}

#[derive(Deserialize, Default)]
struct QueryFilters {
    #[serde(default)]
    source: Option<Value>,
    #[serde(default)]
    kind: Option<Value>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    tags_contain: Option<Value>,
    #[serde(default)]
    width_min: Option<i64>,
    #[serde(default)]
    width_max: Option<i64>,
    #[serde(default)]
    height_min: Option<i64>,
    #[serde(default)]
    height_max: Option<i64>,
    #[serde(default)]
    vision_processed: Option<bool>,
    #[serde(default)]
    preview_only: Option<bool>,
}

#[derive(Deserialize, Default)]
struct QuerySort {
    #[serde(default = "default_sort_field")]
    field: String,
    #[serde(default = "default_sort_order")]
    order: String,
}

fn default_sort_field() -> String {
    "downloaded_at".into()
}
fn default_sort_order() -> String {
    "desc".into()
}

#[derive(Deserialize, Default)]
struct QueryPagination {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_per_page")]
    per_page: u32,
}

fn default_page() -> u32 {
    1
}
fn default_per_page() -> u32 {
    50
}

#[derive(Deserialize, Default)]
struct QueryImagesBody {
    #[serde(default)]
    filters: QueryFilters,
    #[serde(default)]
    sort: QuerySort,
    #[serde(default)]
    pagination: QueryPagination,
}

async fn handle_query_images(
    State(ctx): State<ApiContext>,
    Json(body): Json<QueryImagesBody>,
) -> Response {
    let mgr = ctx.image_manager.clone();
    let mut images = match tokio::task::spawn_blocking(move || mgr.get_all_images()).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return internal(e),
        Err(e) => return internal(e),
    };

    apply_query_filters(&mut images, body.filters);

    let reverse = body.sort.order.eq_ignore_ascii_case("desc");
    let field = body.sort.field;
    sort_images(&mut images, &field, reverse);

    let page = body.pagination.page.max(1);
    let per_page = body.pagination.per_page.clamp(1, 500);
    let total = images.len() as u32;
    let total_pages = total.div_ceil(per_page).max(1);
    // Compute the offset in usize to avoid a u32 multiplication overflow
    // (debug panic / release wrap) on a large attacker-supplied page number.
    let start = (page as usize - 1) * per_page as usize;
    let end = (start + per_page as usize).min(images.len());
    let slice = if start < images.len() {
        images[start..end].to_vec()
    } else {
        Vec::new()
    };

    ok(json!({
        "images": slice,
        "total": total,
        "page": page,
        "per_page": per_page,
        "total_pages": total_pages,
    }))
}

fn apply_query_filters(images: &mut Vec<Image>, f: QueryFilters) {
    if let Some(src) = f.source.as_ref() {
        let sources_lower = match src {
            Value::String(s) => vec![s.to_lowercase()],
            Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                .collect(),
            _ => Vec::new(),
        };
        if !sources_lower.is_empty() {
            images.retain(|i| sources_lower.contains(&i.source.to_lowercase()));
        }
    }
    if let Some(kind_filter) = f.kind.as_ref() {
        let kinds = match kind_filter {
            Value::String(s) => normalize_kind_filter(s),
            Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .flat_map(normalize_kind_filter)
                .collect(),
            _ => Vec::new(),
        };
        if !kinds.is_empty() {
            images.retain(|i| kinds.iter().any(|kind| i.kind.eq_ignore_ascii_case(kind)));
        }
    }
    if let Some(q) = f.query {
        if !q.is_empty() {
            let ql = q.to_lowercase();
            images.retain(|i| i.query.to_lowercase().contains(&ql));
        }
    }
    if let Some(tags_filter) = f.tags_contain {
        let want: Vec<String> = match tags_filter {
            Value::String(s) => vec![s.to_lowercase()],
            Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                .collect(),
            _ => Vec::new(),
        };
        if !want.is_empty() {
            images.retain(|i| i.tags.iter().any(|t| want.contains(&t.to_lowercase())));
        }
    }
    if let Some(min) = f.width_min {
        images.retain(|i| i.width >= min);
    }
    if let Some(max) = f.width_max {
        images.retain(|i| i.width <= max);
    }
    if let Some(min) = f.height_min {
        images.retain(|i| i.height >= min);
    }
    if let Some(max) = f.height_max {
        images.retain(|i| i.height <= max);
    }
    if let Some(vp) = f.vision_processed {
        images.retain(|i| i.vision_processed == vp);
    }
    if let Some(po) = f.preview_only {
        images.retain(|i| i.preview_only == po);
    }
}

fn sort_images(images: &mut [Image], field: &str, reverse: bool) {
    images.sort_by(|a, b| {
        let cmp = match field {
            "width" => a.width.cmp(&b.width),
            "height" => a.height.cmp(&b.height),
            "source" => a.source.cmp(&b.source),
            "query" => a.query.cmp(&b.query),
            "filename" => a.filename.cmp(&b.filename),
            _ => a.downloaded_at.cmp(&b.downloaded_at),
        };
        if reverse {
            cmp.reverse()
        } else {
            cmp
        }
    });
}

// ---------- Search ----------

#[derive(Deserialize)]
struct SearchAllBody {
    query: String,
    #[serde(default)]
    sources: Option<HashMap<String, u32>>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    filters: Option<SearchFilters>,
}

async fn handle_search_all(
    State(ctx): State<ApiContext>,
    Json(body): Json<SearchAllBody>,
) -> Response {
    if body.query.is_empty() {
        return err(StatusCode::BAD_REQUEST, "Query required");
    }
    let sources: HashMap<String, u32> = body
        .sources
        .unwrap_or_else(|| {
            let mut m = HashMap::new();
            m.insert("pixabay".into(), 1);
            m.insert("pexels".into(), 1);
            m.insert("unsplash".into(), 1);
            m
        })
        .into_iter()
        .map(|(k, v)| (k, v.clamp(1, MAX_PER_SOURCE)))
        .collect();
    let kind = SearchKind::from_str(body.kind.as_deref().unwrap_or("photo"));
    let filters = body.filters.unwrap_or_default();
    match search::search_all(
        &ctx.http,
        ctx.settings.clone(),
        body.query.clone(),
        sources,
        kind,
        filters,
        Some(ctx.quota.clone()),
    )
    .await
    {
        Ok(results) => ok(json!({
            "results": results,
            "count": results.len(),
            "query": body.query,
        })),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct OneSourceQuery {
    q: Option<String>,
    query: Option<String>,
    page: Option<u32>,
    kind: Option<String>, // "photo" | "video"
}

async fn handle_search_one_source(
    OriginalUri(uri): OriginalUri,
    State(ctx): State<ApiContext>,
    Query(q): Query<OneSourceQuery>,
) -> Response {
    let path = uri.path().to_string();
    let source = path.rsplit('/').next().unwrap_or("").to_string();
    let query = q.q.or(q.query).unwrap_or_default();
    if query.is_empty() {
        return err(StatusCode::BAD_REQUEST, "Query parameter `q` required");
    }
    let page = q.page.unwrap_or(1);
    let want_video = q
        .kind
        .as_deref()
        .map(|k| k.eq_ignore_ascii_case("video") || k.eq_ignore_ascii_case("videos"))
        .unwrap_or(false);
    let settings = ctx.settings.read().await.clone();
    let filters = SearchFilters::default();
    // Per-source REST endpoints retain provider-default page sizes.
    let pixabay_per_page = 200u32;
    let pexels_per_page = 80u32;
    let unsplash_per_page = 30u32;
    let tracker = Some(&ctx.quota);
    let res = match (source.as_str(), want_video) {
        ("pixabay", false) => {
            search::search_pixabay(
                &ctx.http,
                &settings.pixabay_key,
                &query,
                &filters,
                page,
                pixabay_per_page,
                tracker,
            )
            .await
        }
        ("pixabay", true) => {
            search::search_pixabay_videos(
                &ctx.http,
                &settings.pixabay_key,
                &query,
                &filters,
                page,
                pixabay_per_page,
                tracker,
            )
            .await
        }
        ("pexels", false) => {
            search::search_pexels(
                &ctx.http,
                &settings.pexels_key,
                &query,
                &filters,
                page,
                pexels_per_page,
                tracker,
            )
            .await
        }
        ("pexels", true) => {
            search::search_pexels_videos(
                &ctx.http,
                &settings.pexels_key,
                &query,
                &filters,
                page,
                pexels_per_page,
                tracker,
            )
            .await
        }
        ("unsplash", _) => {
            search::search_unsplash(
                &ctx.http,
                &settings.unsplash_key,
                &query,
                &filters,
                tracker,
                search::UnsplashSearchOptions {
                    page,
                    per_page: unsplash_per_page,
                    // Only fetch details for the first page in the per-source endpoint.
                    fetch_details: page <= 1,
                },
            )
            .await
        }
        _ => return err(StatusCode::NOT_FOUND, "Unknown source"),
    };
    match res {
        Ok(results) => ok(json!({
            "results": results,
            "count": results.len(),
            "source": source,
            "query": query,
        })),
        Err(e) => internal(e),
    }
}

// ---------- Download ----------

#[derive(Deserialize)]
struct DownloadSingleBody {
    url: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_api_source")]
    source: String,
    #[serde(default = "default_api_query")]
    query: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    alt: String,
    #[serde(default)]
    preview_only: bool,
}

fn default_api_source() -> String {
    "API".into()
}
fn default_api_query() -> String {
    "api-download".into()
}

fn infer_download_kind(url: &str, requested: Option<&str>) -> &'static str {
    if let Some(kind) = requested {
        let normalized = kind.trim().to_ascii_lowercase();
        if matches!(normalized.as_str(), "video" | "videos" | "clip" | "clips") {
            return "video";
        }
        if matches!(normalized.as_str(), "photo" | "photos" | "image" | "images") {
            return "photo";
        }
    }
    let path = url
        .split('?')
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    if path.ends_with(".mp4")
        || path.ends_with(".webm")
        || path.ends_with(".mov")
        || path.ends_with(".m4v")
    {
        "video"
    } else {
        "photo"
    }
}

async fn handle_download_single(
    State(ctx): State<ApiContext>,
    Json(body): Json<DownloadSingleBody>,
) -> Response {
    if body.url.is_empty() {
        return err(StatusCode::BAD_REQUEST, "URL required");
    }
    if ctx.image_manager.is_url_saved(&body.url) {
        return err(StatusCode::CONFLICT, "Image already exists");
    }
    let kind = infer_download_kind(&body.url, body.kind.as_deref());
    let mut result = SearchResult::empty(&body.source, kind, &body.query);
    result.url = body.url;
    result.tags = body.tags;
    result.alt = body.alt;
    match downloader::download_one(
        &ctx.download_http,
        &ctx.image_manager,
        &result,
        body.preview_only,
    )
    .await
    {
        Ok(Some(img)) => ok(img),
        Ok(None) => err(StatusCode::INTERNAL_SERVER_ERROR, "Download failed"),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct DownloadBatchBody {
    items: Vec<SearchResult>,
    #[serde(default)]
    preview_only: bool,
    #[serde(default = "default_concurrency")]
    concurrency: usize,
}

fn default_concurrency() -> usize {
    8
}

async fn handle_download_batch(
    State(ctx): State<ApiContext>,
    Json(body): Json<DownloadBatchBody>,
) -> Response {
    if body.items.is_empty() {
        return err(StatusCode::BAD_REQUEST, "No items provided");
    }
    if body.items.len() > MAX_BATCH_ITEMS {
        return err(StatusCode::BAD_REQUEST, "Too many items in one batch");
    }
    let total = body.items.len() as u32;
    let task_id = ctx.tasks.create("download_batch", total).await;
    spawn_download_task(
        task_id.clone(),
        ctx.tasks.clone(),
        ctx.download_http.clone(),
        ctx.image_manager.clone(),
        body.items,
        body.preview_only,
        body.concurrency,
    );
    ok(json!({
        "task_id": task_id,
        "total": total,
        "message": "Download started",
    }))
}

fn spawn_download_task(
    task_id: String,
    tasks: Arc<TaskTracker>,
    http: reqwest::Client,
    manager: Arc<ImageManager>,
    items: Vec<SearchResult>,
    preview_only: bool,
    concurrency: usize,
) {
    tokio::spawn(async move {
        tasks.set_running(&task_id).await;
        let semaphore = Arc::new(tokio::sync::Semaphore::new(
            concurrency.clamp(1, MAX_CONCURRENCY),
        ));
        let mut handles = Vec::with_capacity(items.len());
        for item in items {
            let permit = semaphore.clone();
            let http = http.clone();
            let mgr = manager.clone();
            let tasks_clone = tasks.clone();
            let task_id = task_id.clone();
            handles.push(tokio::spawn(async move {
                let _p = permit.acquire_owned().await.ok();
                let url = item.url.clone();
                let outcome = downloader::download_one(&http, &mgr, &item, preview_only).await;
                tasks_clone.bump(&task_id, 1).await;
                match outcome {
                    Ok(Some(img)) => Ok(img),
                    Ok(None) => Err(json!({
                        "url": url,
                        "error": "download returned no saved item"
                    })),
                    Err(err) => {
                        tracing::warn!("download failed for {}: {}", item.url, err);
                        Err(json!({
                            "url": url,
                            "error": err.to_string()
                        }))
                    }
                }
            }));
        }
        let mut saved: Vec<Image> = Vec::new();
        let mut failures: Vec<Value> = Vec::new();
        for h in handles {
            match h.await {
                Ok(Ok(img)) => saved.push(img),
                Ok(Err(failure)) => failures.push(failure),
                Err(err) => failures.push(json!({
                    "url": null,
                    "error": format!("download task join failed: {err}")
                })),
            }
        }
        let count = saved.len();
        tasks
            .complete(
                &task_id,
                Some(json!({
                    "saved": count,
                    "failed": failures.len(),
                    "images": saved,
                    "failures": failures,
                })),
            )
            .await;
    });
}

async fn handle_task_status(
    State(ctx): State<ApiContext>,
    AxumPath(task_id): AxumPath<String>,
) -> Response {
    match ctx.tasks.get(&task_id).await {
        Some(t) => ok(t),
        None => err(StatusCode::NOT_FOUND, "Task not found"),
    }
}

// ---------- Topics ----------

#[derive(Deserialize, Default)]
struct TopicCreateBody {
    query: String,
    #[serde(default)]
    filters: Option<SearchFilters>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    sources: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
struct TopicMoreBody {
    #[serde(default)]
    count_per_source: Option<u32>,
}

#[derive(Deserialize, Default)]
struct TopicRenameBody {
    #[serde(default)]
    name: Option<String>,
}

async fn handle_list_topics(State(ctx): State<ApiContext>) -> Response {
    let store = ctx.topics.clone();
    match tokio::task::spawn_blocking(move || store.list_summaries()).await {
        Ok(Ok(topics)) => {
            let count = topics.len();
            ok(json!({ "topics": topics, "count": count }))
        }
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e),
    }
}

async fn handle_create_topic(
    State(ctx): State<ApiContext>,
    Json(body): Json<TopicCreateBody>,
) -> Response {
    let query = body.query.trim().to_string();
    if query.is_empty() {
        return err(StatusCode::BAD_REQUEST, "Query required");
    }
    let filters = body.filters.unwrap_or_default();
    let kind = body.kind.unwrap_or_else(|| "photo".into());
    let sources = body
        .sources
        .unwrap_or_else(|| vec!["pixabay".into(), "pexels".into(), "unsplash".into()]);
    let store = ctx.topics.clone();
    match tokio::task::spawn_blocking(move || {
        store.find_or_create(&query, &filters, &kind, &sources)
    })
    .await
    {
        Ok(Ok(topic)) => ok(topic),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e),
    }
}

async fn handle_topic_status(
    State(ctx): State<ApiContext>,
    AxumPath(topic_id): AxumPath<String>,
) -> Response {
    let store = ctx.topics.clone();
    match tokio::task::spawn_blocking(move || store.status(&topic_id)).await {
        Ok(Ok(Some(status))) => ok(status),
        Ok(Ok(None)) => err(StatusCode::NOT_FOUND, "Topic not found"),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e),
    }
}

async fn handle_topic_get_more(
    State(ctx): State<ApiContext>,
    AxumPath(topic_id): AxumPath<String>,
    body: Option<Json<TopicMoreBody>>,
) -> Response {
    let Json(body) = body.unwrap_or_default();
    let count_per_source = body.count_per_source.map(|c| c.clamp(1, MAX_PER_SOURCE));
    match topics::topic_get_more(
        &ctx.topics,
        &topic_id,
        &ctx.http,
        ctx.settings.clone(),
        ctx.image_manager.clone(),
        Some(ctx.quota.clone()),
        count_per_source,
    )
    .await
    {
        Ok(result) => ok(result),
        Err(e) => {
            if e.to_string().contains("topic not found") {
                err(StatusCode::NOT_FOUND, "Topic not found")
            } else {
                internal(e)
            }
        }
    }
}

async fn handle_topic_reset(
    State(ctx): State<ApiContext>,
    AxumPath(topic_id): AxumPath<String>,
) -> Response {
    let store = ctx.topics.clone();
    match tokio::task::spawn_blocking(move || store.reset(&topic_id)).await {
        Ok(Ok(())) => ok(json!({ "ok": true })),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e),
    }
}

async fn handle_topic_delete(
    State(ctx): State<ApiContext>,
    AxumPath(topic_id): AxumPath<String>,
) -> Response {
    let store = ctx.topics.clone();
    match tokio::task::spawn_blocking(move || store.delete(&topic_id)).await {
        Ok(Ok(())) => ok(json!({ "deleted": true })),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e),
    }
}

async fn handle_topic_rename(
    State(ctx): State<ApiContext>,
    AxumPath(topic_id): AxumPath<String>,
    Json(body): Json<TopicRenameBody>,
) -> Response {
    let store = ctx.topics.clone();
    match tokio::task::spawn_blocking(move || store.rename(&topic_id, body.name)).await {
        Ok(Ok(())) => ok(json!({ "renamed": true })),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e),
    }
}

async fn handle_topic_image_ids(
    State(ctx): State<ApiContext>,
    AxumPath(topic_id): AxumPath<String>,
) -> Response {
    let store = ctx.topics.clone();
    match tokio::task::spawn_blocking(move || store.topic_image_ids(&topic_id)).await {
        Ok(Ok(ids)) => {
            let count = ids.len();
            ok(json!({ "ids": ids, "count": count }))
        }
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e),
    }
}

// ---------- Vision (Florence-2) ----------

async fn handle_vision_status(State(ctx): State<ApiContext>) -> Response {
    let status = ctx.vision.status();
    ok(json!({
        "instances_total": status.instances,
        "instances_loaded": status.instances,
        "ready": status.loaded,
        "precision": status.precision,
        "model_dir": status.model_dir,
        "runtime": status.runtime,
        "mode": status.mode,
        "devices": status.devices,
        "workers": status.workers,
        "warnings": status.warnings,
    }))
}

#[derive(Deserialize, Default)]
struct VisionLoadBody {
    #[serde(default)]
    precision: Option<String>,
    #[serde(default)]
    count: Option<usize>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    cpu_instances: Option<usize>,
    #[serde(default)]
    gpu_instances_per_gpu: Option<usize>,
    #[serde(default)]
    max_total_instances: Option<usize>,
    #[serde(default)]
    reserved_vram_gb: Option<f64>,
    #[serde(default)]
    allow_cpu_fallback: Option<bool>,
    #[serde(default)]
    cpu_threads_per_instance: Option<usize>,
}

async fn handle_vision_load(
    State(ctx): State<ApiContext>,
    body: Option<Json<VisionLoadBody>>,
) -> Response {
    let Json(body) = body.unwrap_or_default();
    let precision = parse_vision_precision(body.precision.as_deref());
    let settings = ctx.settings.read().await.clone();
    let cpu_threads = body.cpu_threads_per_instance.or_else(|| {
        let threads = settings.vision_cpu_threads_per_instance as usize;
        (threads > 0).then_some(threads)
    });
    let options = VisionLoadOptions {
        precision,
        mode: VisionExecutionMode::parse(
            body.mode
                .as_deref()
                .or(Some(settings.vision_execution_mode.as_str())),
        ),
        cpu_instances: body
            .cpu_instances
            .or(body.count)
            .unwrap_or(settings.vision_cpu_instances as usize),
        gpu_instances_per_gpu: body
            .gpu_instances_per_gpu
            .unwrap_or(settings.vision_max_per_gpu as usize),
        max_total_instances: body
            .max_total_instances
            .unwrap_or(settings.vision_max_total as usize),
        reserved_vram_gb: body
            .reserved_vram_gb
            .unwrap_or(settings.vision_reserved_vram as f64),
        allow_cpu_fallback: body.allow_cpu_fallback.unwrap_or(settings.vision_allow_cpu),
        cpu_threads_per_instance: cpu_threads,
    };
    let cache_dir = ctx.paths.models.clone();
    let vision = ctx.vision.clone();
    tracing::info!("REST Florence-2 load requested");
    let load_result = tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            vision.load_with_options(&cache_dir, options)
        }))
    })
    .await;
    match load_result {
        Ok(Ok(Ok(_n))) => {
            let st = ctx.vision.status();
            tracing::info!("REST Florence-2 loaded {} worker(s)", st.instances);
            ok(json!({
                "ready": st.loaded,
                "instances": st.instances,
                "precision": st.precision,
                "model_dir": st.model_dir,
                "runtime": st.runtime,
                "mode": st.mode,
                "devices": st.devices,
                "workers": st.workers,
                "warnings": st.warnings,
            }))
        }
        Ok(Ok(Err(e))) => internal(e),
        Ok(Err(panic)) => internal(format!(
            "Florence-2 load panicked: {}",
            panic_message(panic.as_ref())
        )),
        Err(e) => internal(e),
    }
}

async fn handle_vision_unload(State(ctx): State<ApiContext>) -> Response {
    ctx.vision.unload_all();
    let st = ctx.vision.status();
    ok(json!({
        "ready": st.loaded,
        "instances": st.instances,
        "precision": st.precision,
        "model_dir": st.model_dir,
        "runtime": st.runtime,
        "mode": st.mode,
        "devices": st.devices,
        "workers": st.workers,
        "warnings": st.warnings,
    }))
}

#[derive(Deserialize, Default)]
struct VisionAnalyzeBody {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    detect_objects: Option<bool>,
    #[serde(default)]
    overwrite_caption: Option<bool>,
    #[serde(default)]
    caption_mode: Option<String>,
    #[serde(default)]
    caption_task: Option<String>,
    #[serde(default)]
    caption_min_chars: Option<usize>,
}

#[derive(Debug, Clone)]
struct RestVisionAnalyzeOptions {
    detect_objects: bool,
    caption_mode: CaptionWriteMode,
    caption_min_chars: usize,
    caption_task: Option<String>,
}

fn rest_vision_options(body: &VisionAnalyzeBody) -> RestVisionAnalyzeOptions {
    let caption_mode =
        parse_caption_write_mode(body.caption_mode.as_deref(), body.overwrite_caption);
    RestVisionAnalyzeOptions {
        detect_objects: body.detect_objects.unwrap_or(true),
        caption_mode,
        caption_min_chars: body.caption_min_chars.unwrap_or(80).clamp(1, 1000),
        caption_task: (caption_mode != CaptionWriteMode::Skip)
            .then(|| caption_task_from_name(body.caption_task.as_deref()).to_string()),
    }
}

async fn handle_vision_analyze_path(
    State(ctx): State<ApiContext>,
    body: Option<Json<VisionAnalyzeBody>>,
) -> Response {
    let Json(body) = body.unwrap_or_default();
    let Some(rel_or_abs) = body.path.as_deref() else {
        return err(StatusCode::BAD_REQUEST, "path required");
    };
    // Only allow a relative path with no traversal, contained in the data
    // root. Reject absolute paths / drive prefixes / `..`, and never echo the
    // caller-supplied path back (existence oracle).
    let rel = std::path::Path::new(rel_or_abs);
    let has_escape = rel.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    });
    if rel.is_absolute() || has_escape {
        return err(
            StatusCode::BAD_REQUEST,
            "path must be relative to the library root and contain no '..'",
        );
    }
    let candidate = ctx.paths.root.join(rel);
    let abs = match (candidate.canonicalize(), ctx.paths.root.canonicalize()) {
        (Ok(c), Ok(root)) if c.starts_with(&root) => c,
        _ => return err(StatusCode::NOT_FOUND, "file not found"),
    };
    let options = rest_vision_options(&body);
    let vision = ctx.vision.clone();
    let analysis = tokio::task::spawn_blocking(move || {
        vision.analyse_with_options(
            &abs,
            VisionAnalysisOptions {
                caption_task: options.caption_task,
                detect_objects: options.detect_objects,
            },
        )
    })
    .await;
    match analysis {
        Ok(Ok(r)) => ok(json!({
            "caption": r.caption,
            "objects": r.objects,
        })),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e),
    }
}

#[derive(Debug)]
enum ApiFlowError {
    BadRequest(String),
    NotFound(String),
    Internal(String),
}

impl ApiFlowError {
    fn into_response(self) -> Response {
        match self {
            Self::BadRequest(message) => err(StatusCode::BAD_REQUEST, message),
            Self::NotFound(message) => err(StatusCode::NOT_FOUND, message),
            Self::Internal(detail) => internal(detail),
        }
    }

    fn task_message(&self) -> String {
        match self {
            Self::BadRequest(message) | Self::NotFound(message) => message.clone(),
            Self::Internal(_) => "Internal server error".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct VisionAnalyzeOutput {
    image_id: String,
    caption: String,
    objects: Vec<DetectedObject>,
}

fn merge_vision_tags(existing: &[String], objects: &[DetectedObject]) -> Vec<String> {
    let mut out = Vec::with_capacity(existing.len() + objects.len());
    let mut seen = std::collections::BTreeSet::new();
    for tag in existing {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_ascii_lowercase()) {
            out.push(trimmed.to_string());
        }
    }
    for object in objects {
        let label = object.label.trim();
        if label.is_empty() {
            continue;
        }
        if seen.insert(label.to_ascii_lowercase()) {
            out.push(label.to_string());
        }
    }
    out
}

fn image_is_analyzable(image: &Image) -> bool {
    !image.path.trim().is_empty() && !image.kind.eq_ignore_ascii_case("video")
}

async fn analyze_image_by_id(
    ctx: &ApiContext,
    image_id: String,
    options: RestVisionAnalyzeOptions,
) -> std::result::Result<VisionAnalyzeOutput, ApiFlowError> {
    let mgr = ctx.image_manager.clone();
    let id_for_lookup = image_id.clone();
    let img = match tokio::task::spawn_blocking(move || mgr.get_image_by_id(&id_for_lookup)).await {
        Ok(Ok(Some(img))) => img,
        Ok(Ok(None)) => return Err(ApiFlowError::NotFound("image not found".into())),
        Ok(Err(e)) => return Err(ApiFlowError::Internal(e.to_string())),
        Err(e) => return Err(ApiFlowError::Internal(e.to_string())),
    };

    if img.path.is_empty() {
        return Err(ApiFlowError::BadRequest(
            "image has no local path (preview-only)".into(),
        ));
    }
    if img.kind.eq_ignore_ascii_case("video") {
        return Err(ApiFlowError::BadRequest(
            "vision analysis currently supports image files, not videos".into(),
        ));
    }
    let abs_path = ctx.paths.root.join(&img.path);
    if !abs_path.exists() {
        return Err(ApiFlowError::NotFound("image file missing on disk".into()));
    }

    let vision = ctx.vision.clone();
    let analysis_path = abs_path.clone();
    let analysis_options = options.clone();
    let analysis = tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            vision.analyse_with_options(
                &analysis_path,
                VisionAnalysisOptions {
                    caption_task: analysis_options.caption_task.clone(),
                    detect_objects: analysis_options.detect_objects,
                },
            )
        }))
    })
    .await;
    let result = match analysis {
        Ok(Ok(Ok(r))) => r,
        Ok(Ok(Err(e))) => return Err(ApiFlowError::Internal(e.to_string())),
        Ok(Err(panic)) => {
            return Err(ApiFlowError::Internal(format!(
                "vision analysis panicked: {}",
                panic_message(panic.as_ref())
            )))
        }
        Err(e) => return Err(ApiFlowError::Internal(e.to_string())),
    };

    let caption_for_db = should_write_caption(
        options.caption_mode,
        &img.alt,
        &result.caption,
        options.caption_min_chars,
    )
    .then_some(result.caption.clone());
    let tag_set = merge_vision_tags(&img.tags, &result.objects);
    let tag_patch = if tag_set == img.tags {
        None
    } else {
        Some(tag_set)
    };
    let mgr2 = ctx.image_manager.clone();
    let id2 = image_id.clone();
    let updated = tokio::task::spawn_blocking(move || {
        mgr2.update_image_metadata(&id2, caption_for_db, tag_patch, Some(true))
    })
    .await;
    match updated {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => {
            return Err(ApiFlowError::NotFound(
                "image disappeared before metadata update".into(),
            ))
        }
        Ok(Err(e)) => return Err(ApiFlowError::Internal(e.to_string())),
        Err(e) => return Err(ApiFlowError::Internal(e.to_string())),
    }

    Ok(VisionAnalyzeOutput {
        image_id,
        caption: result.caption,
        objects: result.objects,
    })
}

async fn handle_vision_analyze_image(
    State(ctx): State<ApiContext>,
    AxumPath(image_id): AxumPath<String>,
    body: Option<Json<VisionAnalyzeBody>>,
) -> Response {
    let Json(body) = body.unwrap_or_default();
    let options = rest_vision_options(&body);
    match analyze_image_by_id(&ctx, image_id, options).await {
        Ok(result) => ok(result),
        Err(e) => e.into_response(),
    }
}

fn parse_vision_precision(s: Option<&str>) -> crate::vision::Precision {
    match s.map(|s| s.to_ascii_lowercase()) {
        Some(s) if s == "fp16" => Precision::Fp16,
        Some(s) if s == "int8" => Precision::Int8,
        Some(s) if s == "q4f16" || s == "q4" => Precision::Q4f16,
        _ => Precision::Fp32,
    }
}

// ---------- Combo ----------

#[derive(Deserialize)]
struct ComboSearchDownloadBody {
    query: String,
    #[serde(default)]
    sources: Option<HashMap<String, u32>>,
    #[serde(default = "default_combo_limit")]
    limit: usize,
    #[serde(default)]
    preview_only: bool,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default = "default_concurrency")]
    concurrency: usize,
    #[serde(default)]
    filters: Option<SearchFilters>,
}

#[derive(Clone)]
struct VisionWorkflowOptions {
    analyze: RestVisionAnalyzeOptions,
    load_if_needed: bool,
    unload_after: bool,
    vision_instances: Option<usize>,
}

fn default_combo_limit() -> usize {
    10
}

fn normalize_sources(
    input: Option<HashMap<String, u32>>,
    default_all: bool,
) -> HashMap<String, u32> {
    input
        .unwrap_or_else(|| {
            let mut m = HashMap::new();
            m.insert("pixabay".into(), 1);
            if default_all {
                m.insert("pexels".into(), 1);
                m.insert("unsplash".into(), 1);
            }
            m
        })
        .into_iter()
        .map(|(k, v)| (k, v.clamp(1, MAX_PER_SOURCE)))
        .collect()
}

async fn ensure_vision_ready(
    ctx: &ApiContext,
    options: &VisionWorkflowOptions,
) -> std::result::Result<bool, ApiFlowError> {
    if ctx.vision.status().loaded {
        return Ok(false);
    }
    if !options.load_if_needed {
        return Err(ApiFlowError::BadRequest(
            "vision model is not loaded; call /api/v1/vision/load first or set load_if_needed=true"
                .into(),
        ));
    }
    let settings = ctx.settings.read().await.clone();
    let cpu_threads = {
        let threads = settings.vision_cpu_threads_per_instance as usize;
        (threads > 0).then_some(threads)
    };
    let requested_instances = options.vision_instances;
    let load_options = VisionLoadOptions {
        precision: Precision::Fp32,
        mode: VisionExecutionMode::parse(Some(settings.vision_execution_mode.as_str())),
        cpu_instances: requested_instances.unwrap_or(settings.vision_cpu_instances as usize),
        gpu_instances_per_gpu: settings.vision_max_per_gpu as usize,
        max_total_instances: requested_instances.unwrap_or(settings.vision_max_total as usize),
        reserved_vram_gb: settings.vision_reserved_vram as f64,
        allow_cpu_fallback: settings.vision_allow_cpu,
        cpu_threads_per_instance: cpu_threads,
    };
    let cache_dir = ctx.paths.models.clone();
    let vision = ctx.vision.clone();
    let loaded = tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            vision.load_with_options(&cache_dir, load_options)
        }))
    })
    .await;
    match loaded {
        Ok(Ok(Ok(_))) => Ok(true),
        Ok(Ok(Err(e))) => Err(ApiFlowError::Internal(e.to_string())),
        Ok(Err(panic)) => Err(ApiFlowError::Internal(format!(
            "vision load panicked: {}",
            panic_message(panic.as_ref())
        ))),
        Err(e) => Err(ApiFlowError::Internal(e.to_string())),
    }
}

async fn unprocessed_analyzable_ids(
    ctx: &ApiContext,
    filters: Option<QueryFilters>,
    limit: usize,
) -> std::result::Result<Vec<String>, ApiFlowError> {
    let mgr = ctx.image_manager.clone();
    let mut images = match tokio::task::spawn_blocking(move || mgr.get_all_images()).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return Err(ApiFlowError::Internal(e.to_string())),
        Err(e) => return Err(ApiFlowError::Internal(e.to_string())),
    };
    if let Some(filters) = filters {
        apply_query_filters(&mut images, filters);
    }
    images.retain(|image| !image.vision_processed && image_is_analyzable(image));
    Ok(images
        .into_iter()
        .take(limit.min(MAX_ANALYZE_ITEMS))
        .map(|image| image.id)
        .collect())
}

async fn analyze_ids_concurrent(
    ctx: &ApiContext,
    tasks: Arc<TaskTracker>,
    task_id: String,
    ids: Vec<String>,
    options: RestVisionAnalyzeOptions,
) -> (Vec<VisionAnalyzeOutput>, Vec<Value>) {
    let concurrency = ctx.vision.status().instances.clamp(1, MAX_CONCURRENCY);
    let mut analysis = Vec::new();
    let mut failures = Vec::new();
    let mut stream = futures_util::stream::iter(ids.into_iter().map(|id| {
        let ctx = ctx.clone();
        let options = options.clone();
        async move {
            let result = analyze_image_by_id(&ctx, id.clone(), options).await;
            (id, result)
        }
    }))
    .buffer_unordered(concurrency);

    while let Some((id, result)) = stream.next().await {
        match result {
            Ok(result) => analysis.push(result),
            Err(e) => failures.push(json!({
                "image_id": id,
                "error": e.task_message(),
            })),
        }
        tasks.bump(&task_id, 1).await;
    }

    (analysis, failures)
}

async fn spawn_analyze_ids_task(
    ctx: ApiContext,
    task_type: &str,
    ids: Vec<String>,
    options: VisionWorkflowOptions,
) -> String {
    let ids: Vec<String> = ids.into_iter().take(MAX_ANALYZE_ITEMS).collect();
    let total = ids.len() as u32;
    let task_id = ctx.tasks.create(task_type, total).await;
    let task_id_for_spawn = task_id.clone();
    let tasks = ctx.tasks.clone();
    tokio::spawn(async move {
        tasks.set_running(&task_id_for_spawn).await;
        if ids.is_empty() {
            tasks
                .complete(
                    &task_id_for_spawn,
                    Some(json!({
                        "analyzed": 0,
                        "failed": 0,
                        "analysis": [],
                        "failures": [],
                    })),
                )
                .await;
            return;
        }
        let loaded_by_task = match ensure_vision_ready(&ctx, &options).await {
            Ok(v) => v,
            Err(e) => {
                tasks.fail(&task_id_for_spawn, e.task_message()).await;
                return;
            }
        };
        let (analysis, failures) = analyze_ids_concurrent(
            &ctx,
            tasks.clone(),
            task_id_for_spawn.clone(),
            ids,
            options.analyze,
        )
        .await;
        if options.unload_after {
            ctx.vision.unload_all();
        }
        tasks
            .complete(
                &task_id_for_spawn,
                Some(json!({
                    "analyzed": analysis.len(),
                    "failed": failures.len(),
                    "loaded_by_task": loaded_by_task,
                    "unloaded": options.unload_after,
                    "analysis": analysis,
                    "failures": failures,
                })),
            )
            .await;
    });
    task_id
}

async fn handle_combo_search_download(
    State(ctx): State<ApiContext>,
    Json(body): Json<ComboSearchDownloadBody>,
) -> Response {
    if body.query.is_empty() {
        return err(StatusCode::BAD_REQUEST, "Query required");
    }
    let sources = normalize_sources(body.sources, false);
    let limit = body.limit.min(MAX_COMBO_LIMIT);
    let task_id = ctx.tasks.create("search_download", limit as u32).await;
    let task_id_for_spawn = task_id.clone();
    let tasks = ctx.tasks.clone();
    let search_http = ctx.http.clone();
    let download_http = ctx.download_http.clone();
    let manager = ctx.image_manager.clone();
    let settings = ctx.settings.clone();
    let query = body.query.clone();
    let preview_only = body.preview_only;
    let concurrency = body.concurrency.clamp(1, MAX_CONCURRENCY);

    let kind = SearchKind::from_str(body.kind.as_deref().unwrap_or("photo"));
    let filters = body.filters.clone().unwrap_or_default();
    let combo_tracker = Some(ctx.quota.clone());

    tokio::spawn(async move {
        tasks.set_running(&task_id_for_spawn).await;
        let raw = match search::search_all(
            &search_http,
            settings,
            query,
            sources,
            kind,
            filters,
            combo_tracker,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                // Log detail server-side; return a generic failure to the
                // task-status client (mirrors the `internal()` policy).
                tracing::error!("combo search-download failed: {e}");
                tasks.fail(&task_id_for_spawn, "Search failed".into()).await;
                return;
            }
        };
        let unique: Vec<SearchResult> = raw
            .into_iter()
            .filter(|r| {
                !manager.is_url_saved(&r.url)
                    && !manager.is_source_id_saved(&r.source, &r.source_id)
            })
            .take(limit)
            .collect();
        let total = unique.len() as u32;
        let saved =
            downloader::download_many(download_http, manager, unique, preview_only, concurrency)
                .await;
        tasks
            .complete(
                &task_id_for_spawn,
                Some(json!({
                    "saved": saved.len(),
                    "considered": total,
                    "images": saved,
                })),
            )
            .await;
    });

    ok(json!({
        "task_id": task_id,
        "message": "Search and download started",
    }))
}

#[derive(Deserialize)]
struct ComboDownloadAnalyzeBody {
    url: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_api_source")]
    source: String,
    #[serde(default = "default_api_query")]
    query: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    alt: String,
    #[serde(default)]
    preview_only: bool,
    #[serde(default)]
    detect_objects: Option<bool>,
    #[serde(default)]
    overwrite_caption: Option<bool>,
    #[serde(default)]
    caption_mode: Option<String>,
    #[serde(default)]
    caption_task: Option<String>,
    #[serde(default)]
    caption_min_chars: Option<usize>,
    #[serde(default)]
    load_if_needed: Option<bool>,
    #[serde(default)]
    unload_after: Option<bool>,
    #[serde(default)]
    vision_instances: Option<usize>,
}

#[derive(Default)]
struct WorkflowAnalyzeInput {
    detect_objects: Option<bool>,
    overwrite_caption: Option<bool>,
    caption_mode: Option<String>,
    caption_task: Option<String>,
    caption_min_chars: Option<usize>,
}

fn workflow_options(
    analyze: WorkflowAnalyzeInput,
    load_if_needed: Option<bool>,
    unload_after: Option<bool>,
    vision_instances: Option<usize>,
    default_load: bool,
    default_unload: bool,
) -> VisionWorkflowOptions {
    let body = VisionAnalyzeBody {
        path: None,
        detect_objects: analyze.detect_objects,
        overwrite_caption: analyze.overwrite_caption,
        caption_mode: analyze.caption_mode,
        caption_task: analyze.caption_task,
        caption_min_chars: analyze.caption_min_chars,
    };
    VisionWorkflowOptions {
        analyze: rest_vision_options(&body),
        load_if_needed: load_if_needed.unwrap_or(default_load),
        unload_after: unload_after.unwrap_or(default_unload),
        vision_instances,
    }
}

fn download_body_to_search_result(body: &ComboDownloadAnalyzeBody) -> SearchResult {
    let kind = infer_download_kind(&body.url, body.kind.as_deref());
    let mut result = SearchResult::empty(&body.source, kind, &body.query);
    result.url = body.url.clone();
    result.tags = body.tags.clone();
    result.alt = body.alt.clone();
    result
}

async fn handle_combo_download_analyze(
    State(ctx): State<ApiContext>,
    Json(body): Json<ComboDownloadAnalyzeBody>,
) -> Response {
    if body.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "URL required");
    }
    if body.preview_only {
        return err(
            StatusCode::BAD_REQUEST,
            "download-analyze requires preview_only=false so the original file can be analyzed",
        );
    }
    if ctx.image_manager.is_url_saved(&body.url) {
        return err(StatusCode::CONFLICT, "Image already exists");
    }
    let task_id = ctx.tasks.create("download_analyze", 2).await;
    let task_id_for_spawn = task_id.clone();
    let tasks = ctx.tasks.clone();
    let result = download_body_to_search_result(&body);
    let options = workflow_options(
        WorkflowAnalyzeInput {
            detect_objects: body.detect_objects,
            overwrite_caption: body.overwrite_caption,
            caption_mode: body.caption_mode,
            caption_task: body.caption_task,
            caption_min_chars: body.caption_min_chars,
        },
        body.load_if_needed,
        body.unload_after,
        body.vision_instances,
        true,
        false,
    );
    tokio::spawn(async move {
        tasks.set_running(&task_id_for_spawn).await;
        let downloaded =
            match downloader::download_one(&ctx.download_http, &ctx.image_manager, &result, false)
                .await
            {
                Ok(Some(image)) => image,
                Ok(None) => {
                    tasks
                        .fail(&task_id_for_spawn, "Download failed".into())
                        .await;
                    return;
                }
                Err(e) => {
                    tracing::warn!("download-analyze download failed for {}: {}", result.url, e);
                    tasks
                        .fail(&task_id_for_spawn, "Download failed".into())
                        .await;
                    return;
                }
            };
        tasks.bump(&task_id_for_spawn, 1).await;
        match ensure_vision_ready(&ctx, &options).await {
            Ok(_) => {}
            Err(e) => {
                tasks.fail(&task_id_for_spawn, e.task_message()).await;
                return;
            }
        }
        let analysis =
            match analyze_image_by_id(&ctx, downloaded.id.clone(), options.analyze.clone()).await {
                Ok(result) => result,
                Err(e) => {
                    tasks.fail(&task_id_for_spawn, e.task_message()).await;
                    return;
                }
            };
        tasks.bump(&task_id_for_spawn, 1).await;
        if options.unload_after {
            ctx.vision.unload_all();
        }
        tasks
            .complete(
                &task_id_for_spawn,
                Some(json!({
                    "saved": 1,
                    "analyzed": 1,
                    "unloaded": options.unload_after,
                    "image": downloaded,
                    "analysis": analysis,
                })),
            )
            .await;
    });
    ok(json!({
        "task_id": task_id,
        "message": "Download and analysis started",
    }))
}

#[derive(Deserialize, Default)]
struct ComboAnalyzeBody {
    #[serde(default)]
    ids: Option<Vec<String>>,
    #[serde(default)]
    filters: Option<QueryFilters>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    detect_objects: Option<bool>,
    #[serde(default)]
    overwrite_caption: Option<bool>,
    #[serde(default)]
    caption_mode: Option<String>,
    #[serde(default)]
    caption_task: Option<String>,
    #[serde(default)]
    caption_min_chars: Option<usize>,
    #[serde(default)]
    load_if_needed: Option<bool>,
    #[serde(default)]
    unload_after: Option<bool>,
    #[serde(default)]
    vision_instances: Option<usize>,
}

async fn resolve_combo_analyze_ids(
    ctx: &ApiContext,
    ids: Option<Vec<String>>,
    filters: Option<QueryFilters>,
    limit: Option<usize>,
) -> std::result::Result<Vec<String>, ApiFlowError> {
    let limit = limit.unwrap_or(50).clamp(1, MAX_ANALYZE_ITEMS);
    if let Some(ids) = ids {
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for id in ids {
            let trimmed = id.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                out.push(trimmed.to_string());
            }
            if out.len() >= limit {
                break;
            }
        }
        return Ok(out);
    }
    unprocessed_analyzable_ids(ctx, filters, limit).await
}

async fn handle_combo_analyze_unprocessed(
    State(ctx): State<ApiContext>,
    body: Option<Json<ComboAnalyzeBody>>,
) -> Response {
    let Json(body) = body.unwrap_or_default();
    let ids = match unprocessed_analyzable_ids(
        &ctx,
        body.filters,
        body.limit.unwrap_or(50).clamp(1, MAX_ANALYZE_ITEMS),
    )
    .await
    {
        Ok(ids) => ids,
        Err(e) => return e.into_response(),
    };
    let options = workflow_options(
        WorkflowAnalyzeInput {
            detect_objects: body.detect_objects,
            overwrite_caption: body.overwrite_caption,
            caption_mode: body.caption_mode,
            caption_task: body.caption_task,
            caption_min_chars: body.caption_min_chars,
        },
        body.load_if_needed,
        body.unload_after,
        body.vision_instances,
        true,
        false,
    );
    let task_id = spawn_analyze_ids_task(ctx, "analyze_unprocessed", ids.clone(), options).await;
    ok(json!({
        "task_id": task_id,
        "queued": ids.len(),
        "message": "Analyze-unprocessed task started",
    }))
}

async fn handle_combo_smart_analyze(
    State(ctx): State<ApiContext>,
    body: Option<Json<ComboAnalyzeBody>>,
) -> Response {
    let Json(body) = body.unwrap_or_default();
    let ids = match resolve_combo_analyze_ids(&ctx, body.ids, body.filters, body.limit).await {
        Ok(ids) => ids,
        Err(e) => return e.into_response(),
    };
    let options = workflow_options(
        WorkflowAnalyzeInput {
            detect_objects: body.detect_objects,
            overwrite_caption: body.overwrite_caption,
            caption_mode: body.caption_mode,
            caption_task: body.caption_task,
            caption_min_chars: body.caption_min_chars,
        },
        Some(true),
        body.unload_after,
        body.vision_instances,
        true,
        true,
    );
    let task_id = spawn_analyze_ids_task(ctx, "smart_analyze", ids.clone(), options).await;
    ok(json!({
        "task_id": task_id,
        "queued": ids.len(),
        "message": "Smart analysis task started",
    }))
}

#[derive(Deserialize)]
struct ComboSearchDownloadAnalyzeBody {
    query: String,
    #[serde(default)]
    sources: Option<HashMap<String, u32>>,
    #[serde(default = "default_combo_limit")]
    limit: usize,
    #[serde(default)]
    preview_only: bool,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default = "default_concurrency")]
    concurrency: usize,
    #[serde(default)]
    filters: Option<SearchFilters>,
    #[serde(default)]
    detect_objects: Option<bool>,
    #[serde(default)]
    overwrite_caption: Option<bool>,
    #[serde(default)]
    caption_mode: Option<String>,
    #[serde(default)]
    caption_task: Option<String>,
    #[serde(default)]
    caption_min_chars: Option<usize>,
    #[serde(default)]
    load_if_needed: Option<bool>,
    #[serde(default)]
    unload_after: Option<bool>,
    #[serde(default)]
    vision_instances: Option<usize>,
}

async fn handle_combo_search_download_analyze(
    State(ctx): State<ApiContext>,
    Json(body): Json<ComboSearchDownloadAnalyzeBody>,
) -> Response {
    if body.query.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "Query required");
    }
    if body.preview_only {
        return err(
            StatusCode::BAD_REQUEST,
            "search-download-analyze requires preview_only=false so original files can be analyzed",
        );
    }
    let sources = normalize_sources(body.sources, true);
    let limit = body.limit.min(MAX_COMBO_LIMIT);
    let total = (limit as u32).saturating_mul(2).max(1);
    let task_id = ctx.tasks.create("search_download_analyze", total).await;
    let task_id_for_spawn = task_id.clone();
    let tasks = ctx.tasks.clone();
    let query = body.query.clone();
    let kind = SearchKind::from_str(body.kind.as_deref().unwrap_or("photo"));
    let filters = body.filters.unwrap_or_default();
    let concurrency = body.concurrency.clamp(1, MAX_CONCURRENCY);
    let options = workflow_options(
        WorkflowAnalyzeInput {
            detect_objects: body.detect_objects,
            overwrite_caption: body.overwrite_caption,
            caption_mode: body.caption_mode,
            caption_task: body.caption_task,
            caption_min_chars: body.caption_min_chars,
        },
        body.load_if_needed,
        body.unload_after,
        body.vision_instances,
        true,
        false,
    );
    tokio::spawn(async move {
        tasks.set_running(&task_id_for_spawn).await;
        let raw = match search::search_all(
            &ctx.http,
            ctx.settings.clone(),
            query,
            sources,
            kind,
            filters,
            Some(ctx.quota.clone()),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("combo search-download-analyze search failed: {e}");
                tasks.fail(&task_id_for_spawn, "Search failed".into()).await;
                return;
            }
        };
        let unique: Vec<SearchResult> = raw
            .into_iter()
            .filter(|r| {
                !ctx.image_manager.is_url_saved(&r.url)
                    && !ctx
                        .image_manager
                        .is_source_id_saved(&r.source, &r.source_id)
            })
            .take(limit)
            .collect();
        let considered = unique.len();
        let saved = downloader::download_many(
            ctx.download_http.clone(),
            ctx.image_manager.clone(),
            unique,
            false,
            concurrency,
        )
        .await;
        tasks.bump(&task_id_for_spawn, saved.len() as u32).await;
        match ensure_vision_ready(&ctx, &options).await {
            Ok(_) => {}
            Err(e) => {
                tasks.fail(&task_id_for_spawn, e.task_message()).await;
                return;
            }
        }
        let ids: Vec<String> = saved.iter().map(|image| image.id.clone()).collect();
        let (analysis, failures) = analyze_ids_concurrent(
            &ctx,
            tasks.clone(),
            task_id_for_spawn.clone(),
            ids,
            options.analyze,
        )
        .await;
        if options.unload_after {
            ctx.vision.unload_all();
        }
        tasks
            .complete(
                &task_id_for_spawn,
                Some(json!({
                    "considered": considered,
                    "saved": saved.len(),
                    "analyzed": analysis.len(),
                    "failed": failures.len(),
                    "unloaded": options.unload_after,
                    "images": saved,
                    "analysis": analysis,
                    "failures": failures,
                })),
            )
            .await;
    });
    ok(json!({
        "task_id": task_id,
        "message": "Search, download, and analysis started",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(id: &str, kind: &str, path: &str, vision_processed: bool) -> Image {
        Image {
            id: id.into(),
            source: "Pixabay".into(),
            source_id: format!("source-{id}"),
            kind: kind.into(),
            source_page_url: String::new(),
            filename: format!("{id}.jpg"),
            path: path.into(),
            thumb_path: format!("images/thumbs/{id}.jpg"),
            url: format!("https://example.com/{id}.jpg"),
            urls: json!({}),
            width: 1200,
            height: 800,
            duration_secs: None,
            file_size: Some(1024),
            query: "sunset beach".into(),
            alt: String::new(),
            tags: vec!["beach".into(), "warm".into()],
            color: None,
            blur_hash: None,
            author_name: String::new(),
            author_url: String::new(),
            author_avatar: String::new(),
            views: None,
            downloads: None,
            likes: None,
            comments: None,
            preview_only: path.is_empty(),
            vision_processed,
            ai_generated: None,
            created_at_provider: None,
            downloaded_at: "2026-06-04T17:00:00Z".into(),
            source_data: Value::Null,
        }
    }

    #[test]
    fn merge_vision_tags_preserves_existing_and_dedupes_objects() {
        let existing = vec!["Beach".into(), " warm ".into(), "beach".into()];
        let objects = vec![
            DetectedObject {
                label: "person".into(),
                bbox: [0.0, 0.0, 1.0, 1.0],
            },
            DetectedObject {
                label: "Person".into(),
                bbox: [1.0, 1.0, 2.0, 2.0],
            },
            DetectedObject {
                label: " ".into(),
                bbox: [0.0, 0.0, 0.0, 0.0],
            },
        ];
        assert_eq!(
            merge_vision_tags(&existing, &objects),
            vec!["Beach", "warm", "person"]
        );
    }

    #[test]
    fn image_analyzable_rejects_videos_and_preview_only_rows() {
        assert!(image_is_analyzable(&image(
            "photo",
            "photo",
            "images/originals/photo.jpg",
            false
        )));
        assert!(!image_is_analyzable(&image(
            "video",
            "video",
            "videos/originals/video.mp4",
            false
        )));
        assert!(!image_is_analyzable(&image("preview", "photo", "", false)));
    }

    #[test]
    fn normalize_sources_defaults_and_clamps() {
        let defaults = normalize_sources(None, true);
        assert_eq!(defaults.get("pixabay"), Some(&1));
        assert_eq!(defaults.get("pexels"), Some(&1));
        assert_eq!(defaults.get("unsplash"), Some(&1));

        let mut input = HashMap::new();
        input.insert("pixabay".into(), MAX_PER_SOURCE + 50);
        input.insert("pexels".into(), 0);
        let normalized = normalize_sources(Some(input), false);
        assert_eq!(normalized.get("pixabay"), Some(&MAX_PER_SOURCE));
        assert_eq!(normalized.get("pexels"), Some(&1));
        assert!(!normalized.contains_key("unsplash"));
    }

    #[test]
    fn query_filters_select_unprocessed_analyzable_images() {
        let mut images = vec![
            image("a", "photo", "images/originals/a.jpg", false),
            image("b", "photo", "images/originals/b.jpg", true),
            image("c", "video", "videos/originals/c.mp4", false),
        ];
        apply_query_filters(
            &mut images,
            QueryFilters {
                vision_processed: Some(false),
                ..Default::default()
            },
        );
        images.retain(image_is_analyzable);
        let ids: Vec<_> = images.into_iter().map(|image| image.id).collect();
        assert_eq!(ids, vec!["a"]);
    }
}
