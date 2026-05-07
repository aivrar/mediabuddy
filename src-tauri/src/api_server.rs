use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    extract::{OriginalUri, Path as AxumPath, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

use crate::config::Settings;
use crate::downloader;
use crate::image_manager::ImageManager;
use crate::paths::AppPaths;
use crate::search::{self, Kind as SearchKind, SearchResult};
use crate::system_monitor::SystemMonitor;
use crate::types::Image;
use crate::vision::VisionRegistry;

// ---------- Public surface ----------

#[derive(Clone)]
pub struct ApiContext {
    pub paths: Arc<AppPaths>,
    pub settings: Arc<RwLock<Settings>>,
    pub image_manager: Arc<ImageManager>,
    pub http: reqwest::Client,
    pub system: Arc<SystemMonitor>,
    pub tasks: Arc<TaskTracker>,
    pub started_at: Arc<RwLock<Option<u64>>>,
    pub vision: Arc<VisionRegistry>,
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
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind {addr}: {e}"))?;
        let bound_addr = listener.local_addr().map_err(|e| e.to_string())?;
        let bound_port = bound_addr.port();

        let router = build_router(ctx, cors_enabled);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let started_at = self.started_at.clone();
        *started_at.write().await = Some(now_secs());

        let join = tokio::spawn(async move {
            let server = axum::serve(listener, router).with_graceful_shutdown(async {
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

// ---------- Router ----------

fn build_router(ctx: ApiContext, cors_enabled: bool) -> Router {
    let mut router = Router::new()
        // Status & stats
        .route("/api/v1/status", get(handle_status))
        .route("/api/v1/stats", get(handle_stats))
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
        // Vision (stubbed until ONNX integration)
        .route("/api/v1/vision/status", get(handle_vision_status))
        .route("/api/v1/vision/load", post(handle_vision_unavailable))
        .route("/api/v1/vision/unload", post(handle_vision_unavailable))
        .route(
            "/api/v1/vision/analyze/{image_id}",
            post(handle_vision_unavailable),
        )
        .route("/api/v1/vision/analyze", post(handle_vision_unavailable))
        // Combo
        .route(
            "/api/v1/combo/search-download",
            post(handle_combo_search_download),
        )
        .route(
            "/api/v1/combo/download-analyze",
            post(handle_vision_unavailable),
        )
        .route(
            "/api/v1/combo/analyze-unprocessed",
            post(handle_vision_unavailable),
        )
        .route("/api/v1/combo/smart-analyze", post(handle_vision_unavailable))
        .route(
            "/api/v1/combo/search-download-analyze",
            post(handle_vision_unavailable),
        )
        .with_state(ctx);

    if cors_enabled {
        router = router.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );
    }
    router
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
        Ok(Err(e)) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
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
    top_queries.sort_by(|a, b| b.1.cmp(&a.1));
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

#[derive(Deserialize)]
struct ListImagesQuery {
    page: Option<u32>,
    per_page: Option<u32>,
    source: Option<String>,
    query: Option<String>,
    vision_processed: Option<String>,
}

async fn handle_list_images(
    State(ctx): State<ApiContext>,
    Query(q): Query<ListImagesQuery>,
) -> Response {
    let mgr = ctx.image_manager.clone();
    let images = match tokio::task::spawn_blocking(move || mgr.get_all_images()).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let mut filtered: Vec<Image> = images;
    if let Some(s) = q.source.as_deref() {
        if !s.is_empty() {
            let sl = s.to_lowercase();
            filtered.retain(|i| i.source.to_lowercase() == sl);
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
    let start = ((page - 1) * per_page) as usize;
    let end = (start + per_page as usize).min(filtered.len());
    let slice = if start < filtered.len() {
        filtered[start..end].to_vec()
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

async fn handle_get_image(
    State(ctx): State<ApiContext>,
    AxumPath(image_id): AxumPath<String>,
) -> Response {
    let mgr = ctx.image_manager.clone();
    let image = match tokio::task::spawn_blocking(move || mgr.get_image_by_id(&image_id)).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    match image {
        Some(img) => ok(img),
        None => err(StatusCode::NOT_FOUND, "Image not found"),
    }
}

async fn handle_delete_image(
    State(ctx): State<ApiContext>,
    AxumPath(image_id): AxumPath<String>,
) -> Response {
    let mgr = ctx.image_manager.clone();
    let id_clone = image_id.clone();
    let result =
        match tokio::task::spawn_blocking(move || mgr.delete_images(&[id_clone])).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
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
        Ok(Err(e)) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if !updated {
        return err(StatusCode::NOT_FOUND, "Image not found");
    }
    let mgr = ctx.image_manager.clone();
    let id_for_get = image_id.clone();
    match tokio::task::spawn_blocking(move || mgr.get_image_by_id(&id_for_get)).await {
        Ok(Ok(Some(img))) => ok(img),
        Ok(Ok(None)) => err(StatusCode::NOT_FOUND, "Image not found"),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch updated image"),
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
    let result =
        match tokio::task::spawn_blocking(move || mgr.delete_images(&body.ids)).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
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
        Ok(Err(e)) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
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
    } else {
        "image/jpeg"
    }
}

#[derive(Deserialize, Default)]
struct QueryFilters {
    #[serde(default)]
    source: Option<Value>,
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
        Ok(Err(e)) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let f = body.filters;
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
            images.retain(|i| {
                i.tags
                    .iter()
                    .any(|t| want.contains(&t.to_lowercase()))
            });
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

    let reverse = body.sort.order.eq_ignore_ascii_case("desc");
    let field = body.sort.field;
    sort_images(&mut images, &field, reverse);

    let page = body.pagination.page.max(1);
    let per_page = body.pagination.per_page.clamp(1, 500);
    let total = images.len() as u32;
    let total_pages = total.div_ceil(per_page).max(1);
    let start = ((page - 1) * per_page) as usize;
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
}

async fn handle_search_all(
    State(ctx): State<ApiContext>,
    Json(body): Json<SearchAllBody>,
) -> Response {
    if body.query.is_empty() {
        return err(StatusCode::BAD_REQUEST, "Query required");
    }
    let sources = body.sources.unwrap_or_else(|| {
        let mut m = HashMap::new();
        m.insert("pixabay".into(), 1);
        m.insert("pexels".into(), 1);
        m.insert("unsplash".into(), 1);
        m
    });
    let kind = SearchKind::from_str(body.kind.as_deref().unwrap_or("photo"));
    match search::search_all(&ctx.http, ctx.settings.clone(), body.query.clone(), sources, kind).await {
        Ok(results) => ok(json!({
            "results": results,
            "count": results.len(),
            "query": body.query,
        })),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
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
    let res = match (source.as_str(), want_video) {
        ("pixabay", false) => {
            search::search_pixabay(&ctx.http, &settings.pixabay_key, &query, page).await
        }
        ("pixabay", true) => {
            search::search_pixabay_videos(&ctx.http, &settings.pixabay_key, &query, page).await
        }
        ("pexels", false) => {
            search::search_pexels(&ctx.http, &settings.pexels_key, &query, page).await
        }
        ("pexels", true) => {
            search::search_pexels_videos(&ctx.http, &settings.pexels_key, &query, page).await
        }
        ("unsplash", _) => {
            search::search_unsplash(&ctx.http, &settings.unsplash_key, &query, page).await
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
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
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
    let mut result = SearchResult::empty(&body.source, "photo", &body.query);
    result.url = body.url;
    result.tags = body.tags;
    result.alt = body.alt;
    match downloader::download_one(&ctx.http, &ctx.image_manager, &result, body.preview_only)
        .await
    {
        Ok(Some(img)) => ok(img),
        Ok(None) => err(StatusCode::INTERNAL_SERVER_ERROR, "Download failed"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
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
    let total = body.items.len() as u32;
    let task_id = ctx.tasks.create("download_batch", total).await;
    spawn_download_task(
        task_id.clone(),
        ctx.tasks.clone(),
        ctx.http.clone(),
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
        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency.max(1)));
        let mut handles = Vec::with_capacity(items.len());
        for item in items {
            let permit = semaphore.clone();
            let http = http.clone();
            let mgr = manager.clone();
            let tasks_clone = tasks.clone();
            let task_id = task_id.clone();
            handles.push(tokio::spawn(async move {
                let _p = permit.acquire_owned().await.ok();
                let outcome =
                    downloader::download_one(&http, &mgr, &item, preview_only).await;
                tasks_clone.bump(&task_id, 1).await;
                match outcome {
                    Ok(Some(img)) => Some(img),
                    Ok(None) => None,
                    Err(err) => {
                        tracing::warn!("download failed for {}: {}", item.url, err);
                        None
                    }
                }
            }));
        }
        let mut saved: Vec<Image> = Vec::new();
        for h in handles {
            if let Ok(Some(img)) = h.await {
                saved.push(img);
            }
        }
        let count = saved.len();
        tasks
            .complete(
                &task_id,
                Some(json!({
                    "saved": count,
                    "images": saved,
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

// ---------- Vision (Florence-2) ----------
// The registry surface is wired up; the actual ONNX inference pipeline is
// scaffolded but not yet end-to-end. load/analyse routes return a clear
// 503 with a descriptive message until inference ships.

async fn handle_vision_status(State(ctx): State<ApiContext>) -> Response {
    let status = ctx.vision.status();
    ok(json!({
        "instances_total": status.instances,
        "instances_loaded": status.instances,
        "ready": status.loaded,
        "precision": status.precision,
        "model_dir": status.model_dir,
    }))
}

async fn handle_vision_unavailable() -> Response {
    err(
        StatusCode::SERVICE_UNAVAILABLE,
        "Vision (Florence-2) inference is scaffolded but not yet end-to-end. \
         The ort runtime, tokenizer and image preprocessor are in place; \
         the encoder→decoder generation loop ships in the next release.",
    )
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
}

fn default_combo_limit() -> usize {
    10
}

async fn handle_combo_search_download(
    State(ctx): State<ApiContext>,
    Json(body): Json<ComboSearchDownloadBody>,
) -> Response {
    if body.query.is_empty() {
        return err(StatusCode::BAD_REQUEST, "Query required");
    }
    let sources = body.sources.unwrap_or_else(|| {
        let mut m = HashMap::new();
        m.insert("pixabay".into(), 1);
        m
    });
    let task_id = ctx.tasks.create("search_download", body.limit as u32).await;
    let task_id_for_spawn = task_id.clone();
    let tasks = ctx.tasks.clone();
    let http = ctx.http.clone();
    let manager = ctx.image_manager.clone();
    let settings = ctx.settings.clone();
    let query = body.query.clone();
    let limit = body.limit;
    let preview_only = body.preview_only;

    let kind = SearchKind::from_str(body.kind.as_deref().unwrap_or("photo"));

    tokio::spawn(async move {
        tasks.set_running(&task_id_for_spawn).await;
        let raw =
            match search::search_all(&http, settings, query, sources, kind).await {
                Ok(v) => v,
                Err(e) => {
                    tasks.fail(&task_id_for_spawn, e.to_string()).await;
                    return;
                }
            };
        let unique: Vec<SearchResult> = raw
            .into_iter()
            .filter(|r| !manager.is_url_saved(&r.url))
            .take(limit)
            .collect();
        let total = unique.len() as u32;
        let saved =
            downloader::download_many(http, manager, unique, preview_only, 8).await;
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
