mod api_keys;
mod api_server;
mod config;
mod db;
mod downloader;
mod error;
mod image_manager;
mod logbuf;
mod paths;
mod quota;
mod search;
mod system_monitor;
mod topics;
mod types;
mod urlguard;
mod vision;

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use futures_util::{stream, StreamExt};
use serde::Serialize;
use serde_json::json;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Emitter, Manager};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::api_server::{ApiContext, ApiServer, ApiStatus};
use crate::config::Settings;
use crate::error::panic_message;
use crate::image_manager::ImageManager;
use crate::logbuf::{LogBuffer, LogEntry};
use crate::paths::AppPaths;
use crate::quota::{QuotaSnapshot, QuotaTracker};
use crate::search::{SearchFilters, SearchResult};
use crate::system_monitor::{SystemMonitor, SystemStats};
use crate::topics::{Topic, TopicGetMoreResult, TopicStatus, TopicStore, TopicSummary};
use crate::types::{DeleteResult, Image};
use crate::vision::{
    caption_task_from_name, parse_caption_write_mode, CaptionPolicy, CaptionWriteMode, Precision,
    VisionAnalysisOptions, VisionExecutionMode, VisionLoadOptions, VisionRegistry, VisionStatus,
};

pub struct AppState {
    pub paths: Arc<AppPaths>,
    pub settings: Arc<RwLock<Settings>>,
    pub image_manager: Arc<ImageManager>,
    pub http: reqwest::Client,
    /// HTTP client used only for downloading caller-supplied media URLs.
    /// Configured with no automatic redirect following so the downloader
    /// can re-validate every hop against the SSRF allowlist itself.
    pub download_http: reqwest::Client,
    pub system: Arc<SystemMonitor>,
    pub api_server: Arc<ApiServer>,
    pub log_buffer: LogBuffer,
    pub vision: Arc<VisionRegistry>,
    pub quota: Arc<QuotaTracker>,
    pub topics: Arc<TopicStore>,
}

impl AppState {
    fn api_context(&self) -> ApiContext {
        ApiContext {
            paths: self.paths.clone(),
            settings: self.settings.clone(),
            image_manager: self.image_manager.clone(),
            http: self.http.clone(),
            download_http: self.download_http.clone(),
            system: self.system.clone(),
            tasks: self.api_server.tasks.clone(),
            started_at: self.api_server.started_at.clone(),
            bound: self.api_server.bound.clone(),
            vision: self.vision.clone(),
            quota: self.quota.clone(),
            topics: self.topics.clone(),
            log_buffer: self.log_buffer.clone(),
            rate_limiter: Arc::new(crate::api_server::RateLimiter::new()),
        }
    }
}

/// Generate a random REST API bearer token (~244 bits of entropy from two
/// UUIDv4s, which are backed by the OS CSPRNG via `getrandom`).
fn generate_api_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn apphub_registry_dir() -> Option<PathBuf> {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return Some(PathBuf::from(local).join("AppHub").join("registry"));
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return Some(
            PathBuf::from(profile)
                .join("AppData")
                .join("Local")
                .join("AppHub")
                .join("registry"),
        );
    }
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("AppHub")
            .join("registry")
    })
}

fn app_dir_for_discovery() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .to_string_lossy()
        .to_string()
}

fn discovery_base_url(bound_host: &str, port: u16) -> String {
    let host = bound_host.trim();
    let advertised = match host.to_ascii_lowercase().as_str() {
        "" | "0.0.0.0" | "::" | "[::]" | "localhost" | "::1" | "[::1]" => "127.0.0.1".to_string(),
        _ if host.contains(':') && !host.starts_with('[') => format!("[{host}]"),
        _ => host.to_string(),
    };
    format!("http://{advertised}:{port}")
}

fn discovery_is_specific_non_loopback(bound_host: &str) -> bool {
    let host = bound_host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => !ip.is_loopback() && !ip.is_unspecified(),
        Err(_) => false,
    }
}

fn publish_discovery(
    paths: &AppPaths,
    settings: &Settings,
    status: &ApiStatus,
) -> Result<(), String> {
    let Some(port) = status.port else {
        return Ok(());
    };
    let bound_host = status.host.as_deref().unwrap_or(settings.api_host.as_str());
    write_discovery(paths, settings, bound_host, port)
}

/// Rewrite the discovery file after settings change while the REST server is
/// running, so tools that read it don't keep using a token that no longer
/// works. `bound` is the address the server is actually listening on (the
/// host/port in `settings` only take effect at the next start); `None` means
/// the server is stopped and there is nothing to publish.
pub(crate) fn republish_discovery(
    paths: &AppPaths,
    settings: &Settings,
    bound: Option<(String, u16)>,
) {
    let Some((host, port)) = bound else {
        return;
    };
    if let Err(e) = write_discovery(paths, settings, &host, port) {
        tracing::warn!("could not refresh MediaBuddy discovery registry: {e}");
    }
}

fn write_discovery(
    paths: &AppPaths,
    settings: &Settings,
    bound_host: &str,
    port: u16,
) -> Result<(), String> {
    let dir = apphub_registry_dir()
        .ok_or_else(|| "could not resolve AppHub registry directory".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("mediabuddy.json");
    let api = discovery_base_url(bound_host, port);
    let mut endpoints = json!({
        "api": api,
        "status": format!("{api}/api/v1/status")
    });
    if matches!(
        bound_host.trim().to_ascii_lowercase().as_str(),
        "" | "0.0.0.0" | "::" | "[::]" | "127.0.0.1" | "localhost" | "::1" | "[::1]"
    ) {
        endpoints["localhost_api"] = json!(format!("http://127.0.0.1:{port}"));
    }
    if discovery_is_specific_non_loopback(bound_host) {
        endpoints["wsl_api"] = json!(api);
    }
    let payload = json!({
        "name": "mediabuddy",
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "started_at": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        "app_dir": app_dir_for_discovery(),
        "endpoints": endpoints,
        "auth": {
            "header": "Authorization",
            "scheme": "Bearer",
            "token": settings.api_token.clone()
        },
        "extra": {
            "capabilities": [
                "media-search",
                "image-search",
                "video-search",
                "asset-download",
                "topics",
                "vision"
            ],
            "data_root": paths.root.to_string_lossy().to_string(),
            "models_dir": paths.models.to_string_lossy().to_string(),
            "api_host": bound_host
        }
    });
    let body = serde_json::to_vec_pretty(&payload).map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| e.to_string())
}

fn unpublish_discovery() {
    if let Some(dir) = apphub_registry_dir() {
        let _ = std::fs::remove_file(dir.join("mediabuddy.json"));
    }
}

fn library_mime_for(rel: &str) -> &'static str {
    let lower = rel.to_ascii_lowercase();
    if lower.ends_with(".mp4") || lower.ends_with(".m4v") {
        "video/mp4"
    } else if lower.ends_with(".webm") {
        "video/webm"
    } else if lower.ends_with(".mov") {
        "video/quicktime"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else {
        "image/jpeg"
    }
}

// ---------- Image library ----------

#[tauri::command]
async fn list_images(state: tauri::State<'_, AppState>) -> Result<Vec<Image>, String> {
    let mgr = state.image_manager.clone();
    tokio::task::spawn_blocking(move || mgr.get_all_images())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_images(
    ids: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<DeleteResult, String> {
    let mgr = state.image_manager.clone();
    tokio::task::spawn_blocking(move || mgr.delete_images(&ids))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn is_url_saved(url: String, state: tauri::State<'_, AppState>) -> bool {
    state.image_manager.is_url_saved(&url)
}

#[tauri::command]
async fn update_image(
    id: String,
    alt: Option<String>,
    tags: Option<Vec<String>>,
    state: tauri::State<'_, AppState>,
) -> Result<Option<Image>, String> {
    // Returns the row as saved (with its caption origin), or `None` if the
    // item no longer exists.
    let mgr = state.image_manager.clone();
    tokio::task::spawn_blocking(move || mgr.update_image_metadata(&id, alt, tags))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Add tags to an item, merging into the tags stored right now rather than
/// a list the window loaded earlier. Returns the saved row, or `None` if the
/// item no longer exists.
#[tauri::command]
async fn append_image_tags(
    id: String,
    tags: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Option<Image>, String> {
    let mgr = state.image_manager.clone();
    tokio::task::spawn_blocking(move || mgr.append_tags(&id, &tags))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// How many deleted items are being kept out of search results and
/// downloads.
#[tauri::command]
fn blocked_count(state: tauri::State<'_, AppState>) -> usize {
    state.image_manager.blocked_count()
}

/// Forget every deleted item so it can be found and downloaded again.
#[tauri::command]
async fn clear_blocked(state: tauri::State<'_, AppState>) -> Result<usize, String> {
    let mgr = state.image_manager.clone();
    let cleared = tokio::task::spawn_blocking(move || mgr.clear_blocked())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    tracing::info!("blocked list cleared: {cleared} item(s)");
    Ok(cleared)
}

// ---------- Search & download ----------

#[tauri::command]
async fn search_images(
    query: String,
    sources: HashMap<String, u32>,
    kind: Option<String>,
    filters: Option<SearchFilters>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<SearchResult>, String> {
    let client = state.http.clone();
    let settings = state.settings.clone();
    let manager = state.image_manager.clone();
    let kind = search::Kind::from_str(kind.as_deref().unwrap_or("photo"));
    let filters = filters.unwrap_or_default();
    let tracker = Some(state.quota.clone());
    tracing::info!(
        "search started: query='{query}', kind={kind:?}, sources={}",
        sources.len()
    );
    let raw = search::search_all(&client, settings, query, sources, kind, filters, tracker)
        .await
        .map_err(|e| e.to_string())?;
    let filtered: Vec<SearchResult> = raw
        .into_iter()
        .filter(|r| {
            !r.url.is_empty()
                && !manager.is_url_saved(&r.url)
                && !manager.is_source_id_saved(&r.source, &r.kind, &r.source_id)
        })
        .collect();
    tracing::info!("search finished: {} new result(s)", filtered.len());
    Ok(filtered)
}

#[tauri::command]
async fn download_images(
    results: Vec<SearchResult>,
    preview_only: Option<bool>,
    concurrency: Option<usize>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Image>, String> {
    let client = state.download_http.clone();
    let manager = state.image_manager.clone();
    tracing::info!(
        "download started: {} item(s), preview_only={}, concurrency={}",
        results.len(),
        preview_only.unwrap_or(false),
        concurrency.unwrap_or(8)
    );
    let saved = downloader::download_many(
        client,
        manager,
        results,
        preview_only.unwrap_or(false),
        concurrency.unwrap_or(8),
    )
    .await;
    tracing::info!("download finished: {} item(s) saved", saved.len());
    Ok(saved)
}

// ---------- System monitor ----------

#[tauri::command]
fn get_system_stats(state: tauri::State<'_, AppState>) -> SystemStats {
    state.system.snapshot()
}

// ---------- Vision (Florence-2) ----------

#[derive(serde::Deserialize)]
pub struct VisionLoadParams {
    #[serde(default)]
    precision: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    count: Option<usize>,
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

#[derive(serde::Deserialize, Clone)]
pub struct VisionAnalyzeParams {
    ids: Vec<String>,
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
    /// Keep captions a person wrote, whatever the mode. Defaults to true.
    #[serde(default)]
    protect_manual: Option<bool>,
    #[serde(default)]
    load_if_needed: Option<bool>,
    #[serde(default)]
    concurrency: Option<usize>,
}

#[derive(Debug, Clone)]
struct LibraryAnalyzeOptions {
    detect_objects: bool,
    caption_policy: CaptionPolicy,
    caption_task: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VisionAnalyzeItem {
    pub id: String,
    pub ok: bool,
    pub skipped: bool,
    pub error: Option<String>,
    pub caption: Option<String>,
    pub caption_written: bool,
    pub tags_added: Vec<String>,
    pub objects: Vec<crate::vision::DetectedObject>,
    pub image: Option<Image>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VisionAnalyzeSummary {
    pub total: usize,
    pub analyzed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub results: Vec<VisionAnalyzeItem>,
}

#[tauri::command]
async fn vision_status(state: tauri::State<'_, AppState>) -> Result<VisionStatus, String> {
    Ok(state.vision.status())
}

#[tauri::command]
async fn vision_load(
    params: VisionLoadParams,
    state: tauri::State<'_, AppState>,
) -> Result<VisionStatus, String> {
    let precision = parse_precision(params.precision.as_deref());
    let settings = state.settings.read().await.clone();
    let cpu_threads = params.cpu_threads_per_instance.or_else(|| {
        let threads = settings.vision_cpu_threads_per_instance as usize;
        (threads > 0).then_some(threads)
    });
    let mode = VisionExecutionMode::parse(
        params
            .mode
            .as_deref()
            .or(Some(settings.vision_execution_mode.as_str())),
    );
    let options = VisionLoadOptions {
        precision,
        mode,
        cpu_instances: params
            .cpu_instances
            .or(params.count)
            .unwrap_or(settings.vision_cpu_instances as usize),
        gpu_instances_per_gpu: params
            .gpu_instances_per_gpu
            .unwrap_or(settings.vision_max_per_gpu as usize),
        max_total_instances: params
            .max_total_instances
            .unwrap_or(settings.vision_max_total as usize),
        reserved_vram_gb: params
            .reserved_vram_gb
            .unwrap_or(settings.vision_reserved_vram as f64),
        allow_cpu_fallback: params
            .allow_cpu_fallback
            .unwrap_or(settings.vision_allow_cpu),
        cpu_threads_per_instance: cpu_threads,
    };
    let cache_dir = state.paths.models.clone();
    let vision = state.vision.clone();
    tracing::info!("Florence-2 load requested");
    tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            vision.load_with_options(&cache_dir, options)
        }))
    })
    .await
    .map_err(|e| format!("vision load worker failed: {e}"))?
    .map_err(|panic| {
        format!(
            "Florence-2 load panicked: {}",
            panic_message(panic.as_ref())
        )
    })?
    .map_err(|e| e.to_string())?;
    let status = state.vision.status();
    tracing::info!("Florence-2 loaded {} worker(s)", status.instances);
    Ok(status)
}

#[tauri::command]
async fn vision_unload(state: tauri::State<'_, AppState>) -> Result<VisionStatus, String> {
    if !state.vision.try_unload() {
        return Err(
            "Florence-2 is busy analyzing. Wait for the running job to finish, then unload."
                .to_string(),
        );
    }
    tracing::info!("Florence-2 unloaded");
    Ok(state.vision.status())
}

#[tauri::command]
async fn vision_analyze_images(
    params: VisionAnalyzeParams,
    state: tauri::State<'_, AppState>,
) -> Result<VisionAnalyzeSummary, String> {
    let ids = dedupe_ids(params.ids);
    if ids.is_empty() {
        return Ok(VisionAnalyzeSummary {
            total: 0,
            analyzed: 0,
            skipped: 0,
            failed: 0,
            results: Vec::new(),
        });
    }

    // Held until the last image is done, so nothing unloads the model
    // part-way through this batch.
    let _job = state.vision.begin_job();

    if !state.vision.status().loaded {
        if params.load_if_needed.unwrap_or(false) {
            let settings = state.settings.read().await.clone();
            let options = VisionLoadOptions {
                precision: Precision::Fp32,
                mode: VisionExecutionMode::parse(Some(settings.vision_execution_mode.as_str())),
                cpu_instances: settings.vision_cpu_instances as usize,
                gpu_instances_per_gpu: settings.vision_max_per_gpu as usize,
                max_total_instances: settings.vision_max_total as usize,
                reserved_vram_gb: settings.vision_reserved_vram as f64,
                allow_cpu_fallback: settings.vision_allow_cpu,
                cpu_threads_per_instance: {
                    let threads = settings.vision_cpu_threads_per_instance as usize;
                    (threads > 0).then_some(threads)
                },
            };
            let cache_dir = state.paths.models.clone();
            let vision = state.vision.clone();
            tracing::info!("Florence-2 auto-load requested for {} image(s)", ids.len());
            let loaded = tokio::task::spawn_blocking(move || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    vision.load_with_options(&cache_dir, options)
                }))
            })
            .await
            .map_err(|e| format!("vision load worker failed: {e}"))?
            .map_err(|panic| {
                format!(
                    "Florence-2 load panicked: {}",
                    panic_message(panic.as_ref())
                )
            })?
            .map_err(|e| e.to_string())?;
            tracing::info!("Florence-2 auto-loaded {loaded} worker(s)");
        } else {
            return Err(
                "Florence-2 is not loaded. Load it in Settings or enable load_if_needed."
                    .to_string(),
            );
        }
    }

    let detect_objects = params.detect_objects.unwrap_or(true);
    let caption_mode =
        parse_caption_write_mode(params.caption_mode.as_deref(), params.overwrite_caption);
    let caption_policy = CaptionPolicy {
        mode: caption_mode,
        min_chars: params.caption_min_chars.unwrap_or(80).clamp(1, 1000),
        protect_manual: params.protect_manual.unwrap_or(true),
    };
    let caption_task = (caption_mode != CaptionWriteMode::Skip)
        .then(|| caption_task_from_name(params.caption_task.as_deref()).to_string());
    let max_concurrency = state.vision.status().instances.clamp(1, 32);
    let concurrency = params
        .concurrency
        .unwrap_or(max_concurrency)
        .clamp(1, max_concurrency);
    tracing::info!(
        "Florence-2 analysis started: {} image(s), concurrency {}, detect_objects={}, caption_mode={}, protect_manual={}, caption_task={}",
        ids.len(),
        concurrency,
        detect_objects,
        caption_mode.as_str(),
        caption_policy.protect_manual,
        caption_task.as_deref().unwrap_or("none")
    );

    let paths = state.paths.clone();
    let manager = state.image_manager.clone();
    let vision = state.vision.clone();
    let analyze_options = LibraryAnalyzeOptions {
        detect_objects,
        caption_policy,
        caption_task,
    };
    let results: Vec<VisionAnalyzeItem> = stream::iter(ids.clone())
        .map(|id| {
            let paths = paths.clone();
            let manager = manager.clone();
            let vision = vision.clone();
            let analyze_options = analyze_options.clone();
            let error_id = id.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    analyze_library_image(paths, manager, vision, id, analyze_options)
                })
                .await
                .unwrap_or_else(|e| VisionAnalyzeItem {
                    id: error_id,
                    ok: false,
                    skipped: false,
                    error: Some(format!("analysis worker failed: {e}")),
                    caption: None,
                    caption_written: false,
                    tags_added: Vec::new(),
                    objects: Vec::new(),
                    image: None,
                })
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let analyzed = results.iter().filter(|r| r.ok).count();
    let skipped = results.iter().filter(|r| r.skipped).count();
    let failed = results.iter().filter(|r| !r.ok && !r.skipped).count();
    for result in results.iter().filter(|r| !r.ok && !r.skipped) {
        let error = result
            .error
            .as_deref()
            .unwrap_or("unknown Florence-2 analysis error");
        tracing::warn!("Florence-2 analysis failed for {}: {}", result.id, error);
    }
    tracing::info!(
        "Florence-2 analysis finished: {} analyzed, {} skipped, {} failed",
        analyzed,
        skipped,
        failed
    );

    Ok(VisionAnalyzeSummary {
        total: ids.len(),
        analyzed,
        skipped,
        failed,
        results,
    })
}

fn dedupe_ids(ids: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        let id = id.trim().to_string();
        if !id.is_empty() && seen.insert(id.clone()) {
            out.push(id);
        }
    }
    out
}

fn analyze_library_image(
    paths: Arc<AppPaths>,
    manager: Arc<ImageManager>,
    vision: Arc<VisionRegistry>,
    id: String,
    options: LibraryAnalyzeOptions,
) -> VisionAnalyzeItem {
    let image = match manager.get_image_by_id(&id) {
        Ok(Some(image)) => image,
        Ok(None) => {
            return VisionAnalyzeItem {
                id,
                ok: false,
                skipped: true,
                error: Some("image not found".to_string()),
                caption: None,
                caption_written: false,
                tags_added: Vec::new(),
                objects: Vec::new(),
                image: None,
            }
        }
        Err(e) => {
            return VisionAnalyzeItem {
                id,
                ok: false,
                skipped: false,
                error: Some(e.to_string()),
                caption: None,
                caption_written: false,
                tags_added: Vec::new(),
                objects: Vec::new(),
                image: None,
            }
        }
    };

    if image.kind.eq_ignore_ascii_case("video")
        || image.preview_only
        || image.path.trim().is_empty()
    {
        return VisionAnalyzeItem {
            id,
            ok: false,
            skipped: true,
            error: Some("Florence-2 currently analyzes downloaded still images only".to_string()),
            caption: None,
            caption_written: false,
            tags_added: Vec::new(),
            objects: Vec::new(),
            image: Some(image),
        };
    }

    let abs_path = match resolve_library_media_path(&paths.root, &image.path) {
        Ok(path) => path,
        Err(e) => {
            return VisionAnalyzeItem {
                id,
                ok: false,
                skipped: false,
                error: Some(e),
                caption: None,
                caption_written: false,
                tags_added: Vec::new(),
                objects: Vec::new(),
                image: Some(image),
            }
        }
    };

    let analysis = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        vision.analyse_with_options(
            &abs_path,
            VisionAnalysisOptions {
                caption_task: options.caption_task.clone(),
                detect_objects: options.detect_objects,
            },
        )
    })) {
        Ok(Ok(result)) => result,
        // `image` was read before inference started and may be stale by now,
        // so failures return no image rather than an out-of-date copy.
        Ok(Err(e)) => {
            return VisionAnalyzeItem {
                id,
                ok: false,
                skipped: false,
                error: Some(e.to_string()),
                caption: None,
                caption_written: false,
                tags_added: Vec::new(),
                objects: Vec::new(),
                image: None,
            }
        }
        Err(panic) => {
            let message = panic_message(panic.as_ref());
            return VisionAnalyzeItem {
                id,
                ok: false,
                skipped: false,
                error: Some(format!("Florence-2 analysis panicked: {message}")),
                caption: None,
                caption_written: false,
                tags_added: Vec::new(),
                objects: Vec::new(),
                image: None,
            };
        }
    };

    // Inference took a while; the row may have been edited or deleted since
    // it was read above. `apply_vision_result` re-reads it under the database
    // lock and decides against what is there now.
    let caption = analysis.caption.trim().to_string();
    let labels: Vec<String> = analysis
        .objects
        .iter()
        .map(|object| object.label.clone())
        .collect();
    match manager.apply_vision_result(&id, &caption, &labels, &options.caption_policy) {
        Ok(Some(outcome)) => VisionAnalyzeItem {
            id,
            ok: true,
            skipped: false,
            error: None,
            caption: Some(caption),
            caption_written: outcome.caption_written,
            tags_added: outcome.tags_added,
            objects: analysis.objects,
            image: Some(outcome.image),
        },
        Ok(None) => VisionAnalyzeItem {
            id,
            ok: false,
            skipped: true,
            error: Some("item was deleted while it was being analyzed".to_string()),
            caption: Some(caption),
            caption_written: false,
            tags_added: Vec::new(),
            objects: analysis.objects,
            image: None,
        },
        Err(e) => VisionAnalyzeItem {
            id,
            ok: false,
            skipped: false,
            error: Some(e.to_string()),
            caption: Some(caption),
            caption_written: false,
            tags_added: Vec::new(),
            objects: analysis.objects,
            image: None,
        },
    }
}

fn parse_precision(s: Option<&str>) -> Precision {
    match s.map(|s| s.to_ascii_lowercase()) {
        Some(s) if s == "fp16" => Precision::Fp16,
        Some(s) if s == "int8" => Precision::Int8,
        Some(s) if s == "q4f16" || s == "q4" => Precision::Q4f16,
        // Default to fp32: only variant currently wired end-to-end on CPU.
        _ => Precision::Fp32,
    }
}

// ---------- Image binary access ----------

pub(crate) fn resolve_library_media_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err("no local media path on record".to_string());
    }

    let rel_path = Path::new(rel);
    if rel_path.is_absolute()
        || rel_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err("media path is outside the app data directory".to_string());
    }

    let root = root
        .canonicalize()
        .map_err(|e| format!("could not resolve app data directory: {e}"))?;
    let candidate = root.join(rel_path);
    let canonical = candidate
        .canonicalize()
        .map_err(|e| format!("could not read local media file: {e}"))?;

    if !canonical.starts_with(&root) {
        return Err("media path is outside the app data directory".to_string());
    }
    if !canonical.is_file() {
        return Err("local media path is not a file".to_string());
    }

    Ok(canonical)
}

#[tauri::command]
async fn read_thumb_bytes(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<u8>, String> {
    let mgr = state.image_manager.clone();
    let id_clone = id.clone();
    let img = tokio::task::spawn_blocking(move || mgr.get_image_by_id(&id_clone))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "image not found".to_string())?;
    let rel = if !img.thumb_path.is_empty() {
        img.thumb_path
    } else {
        img.path
    };
    if rel.is_empty() {
        return Err("no thumbnail or original path on record".into());
    }
    let path = resolve_library_media_path(&state.paths.root, &rel)?;
    tokio::fs::read(&path).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn read_image_bytes(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<u8>, String> {
    let mgr = state.image_manager.clone();
    let id_clone = id.clone();
    let img = tokio::task::spawn_blocking(move || mgr.get_image_by_id(&id_clone))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "image not found".to_string())?;

    let is_video = img.kind.eq_ignore_ascii_case("video");
    let rel = if !is_video && !img.path.is_empty() {
        img.path
    } else if !img.thumb_path.is_empty() {
        img.thumb_path
    } else {
        img.path
    };
    if rel.is_empty() {
        return Err("no image or thumbnail path on record".into());
    }
    let path = resolve_library_media_path(&state.paths.root, &rel)?;
    tokio::fs::read(&path).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn get_data_root(state: tauri::State<'_, AppState>) -> String {
    state.paths.root.to_string_lossy().to_string()
}

fn media_protocol_error(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

pub(crate) fn parse_media_range(header_value: &str, len: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let range = header_value.trim().strip_prefix("bytes=")?;
    let first = range.split(',').next()?.trim();
    let (start_raw, end_raw) = first.split_once('-')?;
    if start_raw.is_empty() {
        let suffix_len = end_raw.parse::<u64>().ok()?;
        if suffix_len == 0 {
            return None;
        }
        let start = len.saturating_sub(suffix_len);
        return Some((start, len - 1));
    }

    let start = start_raw.parse::<u64>().ok()?;
    if start >= len {
        return None;
    }
    let end = if end_raw.is_empty() {
        len - 1
    } else {
        end_raw.parse::<u64>().ok()?.min(len - 1)
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

fn media_protocol_response(
    paths: Arc<AppPaths>,
    manager: Arc<ImageManager>,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let id = request.uri().path().trim_start_matches('/').trim();
    if id.is_empty() {
        return media_protocol_error(StatusCode::BAD_REQUEST, "missing media id");
    }

    let image = match manager.get_image_by_id(id) {
        Ok(Some(image)) => image,
        Ok(None) => return media_protocol_error(StatusCode::NOT_FOUND, "media not found"),
        Err(err) => {
            tracing::warn!("media lookup failed for {id}: {err}");
            return media_protocol_error(StatusCode::INTERNAL_SERVER_ERROR, "media lookup failed");
        }
    };
    if image.path.trim().is_empty() {
        return media_protocol_error(
            StatusCode::NOT_FOUND,
            "this library item has no downloaded media file",
        );
    }

    let path = match resolve_library_media_path(&paths.root, &image.path) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!("media path rejected for {id}: {err}");
            return media_protocol_error(StatusCode::NOT_FOUND, "media file unavailable");
        }
    };

    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(err) => {
            tracing::warn!("media file open failed for {id}: {err}");
            return media_protocol_error(StatusCode::NOT_FOUND, "media file unavailable");
        }
    };
    let len = match file.metadata().map(|m| m.len()) {
        Ok(len) if len > 0 => len,
        _ => return media_protocol_error(StatusCode::NOT_FOUND, "media file is empty"),
    };

    const MAX_RANGE_BYTES: u64 = 2 * 1024 * 1024;
    let mime = library_mime_for(&image.path);
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store");

    let range_header = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok());

    let (status, start, end) = match range_header {
        Some(raw) => match parse_media_range(raw, len) {
            Some((start, requested_end)) => {
                let capped_end = start + (requested_end - start).min(MAX_RANGE_BYTES - 1);
                (StatusCode::PARTIAL_CONTENT, start, capped_end)
            }
            None => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                    .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .body(Vec::new())
                    .unwrap_or_else(|_| Response::new(Vec::new()));
            }
        },
        None => (StatusCode::OK, 0, len - 1),
    };

    let nbytes = end + 1 - start;
    let mut body = Vec::with_capacity(nbytes.min(MAX_RANGE_BYTES) as usize);
    if file.seek(SeekFrom::Start(start)).is_err()
        || file.take(nbytes).read_to_end(&mut body).is_err()
    {
        return media_protocol_error(StatusCode::INTERNAL_SERVER_ERROR, "media read failed");
    }

    builder = builder
        .status(status)
        .header(header::CONTENT_LENGTH, body.len().to_string());
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }

    builder
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

// ---------- Logs ----------

#[derive(serde::Deserialize, Default)]
pub struct LogQuery {
    #[serde(default)]
    since: Option<u64>,
    #[serde(default)]
    level: Option<String>,
}

#[tauri::command]
fn get_logs(query: LogQuery, state: tauri::State<'_, AppState>) -> Vec<LogEntry> {
    let level = query.level.unwrap_or_else(|| "DEBUG".to_string());
    state.log_buffer.snapshot(query.since, &level)
}

#[tauri::command]
fn clear_logs(state: tauri::State<'_, AppState>) {
    state.log_buffer.clear();
}

// ---------- REST API server ----------

#[tauri::command]
async fn api_status(state: tauri::State<'_, AppState>) -> Result<ApiStatus, String> {
    Ok(state.api_server.status().await)
}

#[tauri::command]
async fn api_start(state: tauri::State<'_, AppState>) -> Result<ApiStatus, String> {
    let snapshot = state.settings.read().await.clone();
    let ctx = state.api_context();
    state
        .api_server
        .start(
            snapshot.api_host.clone(),
            snapshot.api_port,
            snapshot.api_cors_enabled,
            ctx,
        )
        .await
        .map_err(|e| e.to_string())?;
    let status = state.api_server.status().await;
    if let Err(e) = publish_discovery(state.paths.as_ref(), &snapshot, &status) {
        tracing::warn!("could not publish MediaBuddy discovery registry: {e}");
    }
    Ok(status)
}

#[tauri::command]
async fn api_stop(state: tauri::State<'_, AppState>) -> Result<ApiStatus, String> {
    state.api_server.stop().await.map_err(|e| e.to_string())?;
    unpublish_discovery();
    Ok(state.api_server.status().await)
}

pub(crate) fn shutdown_app_process() -> ! {
    tracing::info!("Media Buddy shutdown requested");
    unpublish_discovery();
    std::process::exit(0);
}

#[tauri::command]
fn shutdown_app() {
    shutdown_app_process();
}

/// Event sent to the UI when the window is closed while downloads or an
/// analysis job are still running. The UI asks the user and, if they agree,
/// calls `shutdown_app`.
const CLOSE_BLOCKED_EVENT: &str = "close-blocked";

/// One click on the window's close button can reach us through more than one
/// event hook; deliveries this close together are the same click.
const CLOSE_DUPLICATE_WINDOW: Duration = Duration::from_millis(750);

struct CloseWarning {
    warned_at: Option<Instant>,
    /// Set once the UI confirms it received the warning.
    acknowledged: bool,
}

static CLOSE_WARNING: Mutex<CloseWarning> = Mutex::new(CloseWarning {
    warned_at: None,
    acknowledged: false,
});

/// What to do with a request to close the window.
#[derive(Debug, PartialEq, Eq)]
enum CloseDecision {
    /// Nothing is running (or the UI is not answering): quit now.
    Exit,
    /// Work is running: keep the window and ask the user.
    Warn,
    /// A repeat delivery of a click that is already being handled.
    Ignore,
}

fn decide_close(busy: bool, warning: &mut CloseWarning, now: Instant) -> CloseDecision {
    if !busy {
        return CloseDecision::Exit;
    }
    if let Some(at) = warning.warned_at {
        if !warning.acknowledged {
            if now.saturating_duration_since(at) < CLOSE_DUPLICATE_WINDOW {
                return CloseDecision::Ignore;
            }
            // The last warning was never picked up, so the UI is not able to
            // ask. Closing again must work — never trap the user in the app.
            return CloseDecision::Exit;
        }
    }
    warning.warned_at = Some(now);
    warning.acknowledged = false;
    CloseDecision::Warn
}

fn handle_close_request(app: &tauri::AppHandle) {
    let downloads = downloader::active_downloads();
    let analyses = app
        .try_state::<AppState>()
        .map(|state| state.vision.active_jobs())
        .unwrap_or(0);
    let decision = {
        let mut warning = CLOSE_WARNING.lock().unwrap_or_else(|p| p.into_inner());
        decide_close(downloads + analyses > 0, &mut warning, Instant::now())
    };
    match decision {
        CloseDecision::Exit => shutdown_app_process(),
        CloseDecision::Ignore => {}
        CloseDecision::Warn => {
            tracing::info!(
                "close requested with work running ({downloads} download(s), {analyses} analysis job(s)); asking the user"
            );
            let payload = json!({ "downloads": downloads, "analyses": analyses });
            if app.emit(CLOSE_BLOCKED_EVENT, payload).is_err() {
                shutdown_app_process();
            }
        }
    }
}

/// Work that quitting right now would cut off. The UI's Shutdown button asks
/// before quitting when either count is non-zero.
#[tauri::command]
fn active_work(state: tauri::State<'_, AppState>) -> serde_json::Value {
    json!({
        "downloads": downloader::active_downloads(),
        "analyses": state.vision.active_jobs(),
    })
}

/// Called by the UI as soon as it receives the close warning.
#[tauri::command]
fn close_warning_ack() {
    let mut warning = CLOSE_WARNING.lock().unwrap_or_else(|p| p.into_inner());
    warning.acknowledged = true;
}

// ---------- Settings ----------

#[tauri::command]
async fn get_settings(state: tauri::State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.settings.read().await.clone())
}

#[tauri::command]
fn get_quota_status(state: tauri::State<'_, AppState>) -> QuotaSnapshot {
    state.quota.snapshot()
}

// ---------- Topics ----------

#[derive(serde::Deserialize)]
pub struct TopicFindOrCreateParams {
    query: String,
    #[serde(default)]
    filters: Option<SearchFilters>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    sources: Option<Vec<String>>,
}

#[tauri::command]
async fn topic_find_or_create(
    params: TopicFindOrCreateParams,
    state: tauri::State<'_, AppState>,
) -> Result<Topic, String> {
    let query_for_log = params.query.clone();
    let store = state.topics.clone();
    let topic = tokio::task::spawn_blocking(move || {
        let filters = params.filters.unwrap_or_default();
        let kind = params.kind.unwrap_or_else(|| "photo".into());
        let sources = params
            .sources
            .unwrap_or_else(|| vec!["pixabay".into(), "pexels".into(), "unsplash".into()]);
        store.find_or_create(params.query.trim(), &filters, &kind, &sources)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    tracing::info!(
        "topic ready: query='{}', kind={}, sources={}",
        query_for_log,
        topic.kind,
        topic.enabled_sources.len()
    );
    Ok(topic)
}

#[tauri::command]
async fn topic_status(
    topic_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Option<TopicStatus>, String> {
    let store = state.topics.clone();
    tokio::task::spawn_blocking(move || store.status(&topic_id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn topic_list(state: tauri::State<'_, AppState>) -> Result<Vec<TopicSummary>, String> {
    let store = state.topics.clone();
    tokio::task::spawn_blocking(move || store.list_summaries())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn topic_get_more(
    topic_id: String,
    count_per_source: Option<u32>,
    state: tauri::State<'_, AppState>,
) -> Result<TopicGetMoreResult, String> {
    let store = state.topics.clone();
    let client = state.http.clone();
    let settings = state.settings.clone();
    let manager = state.image_manager.clone();
    let tracker = Some(state.quota.clone());
    topics::topic_get_more(
        &store,
        &topic_id,
        &client,
        settings,
        manager,
        tracker,
        count_per_source,
    )
    .await
    .inspect(|result| {
        tracing::info!(
            "topic fetch finished: topic={}, {} result(s), {} provider call(s)",
            topic_id,
            result.results.len(),
            result.progress.len()
        );
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn topic_reset(topic_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let store = state.topics.clone();
    tokio::task::spawn_blocking(move || store.reset(&topic_id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn topic_delete(topic_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let store = state.topics.clone();
    tokio::task::spawn_blocking(move || store.delete(&topic_id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn topic_image_ids(
    topic_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let store = state.topics.clone();
    tokio::task::spawn_blocking(move || store.topic_image_ids(&topic_id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Results the topic fetched earlier that were never downloaded or
/// dismissed. Lets the UI pick up where the user left off.
#[tauri::command]
async fn topic_pending(
    topic_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<SearchResult>, String> {
    let store = state.topics.clone();
    let manager = state.image_manager.clone();
    tokio::task::spawn_blocking(move || {
        store.pending_results(&topic_id, &manager, topics::MAX_PENDING_RESULTS)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Discard the topic's pending results. They stay marked as seen.
#[tauri::command]
async fn topic_dismiss_pending(
    topic_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<usize, String> {
    let store = state.topics.clone();
    tokio::task::spawn_blocking(move || store.dismiss_pending(&topic_id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn topic_rename(
    topic_id: String,
    name: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let store = state.topics.clone();
    tokio::task::spawn_blocking(move || store.rename(&topic_id, name))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn validate_api_key(
    provider: String,
    key: String,
    state: tauri::State<'_, AppState>,
) -> Result<api_keys::KeyProbe, String> {
    let provider = match provider.to_ascii_lowercase().as_str() {
        "pixabay" => api_keys::Provider::Pixabay,
        "pexels" => api_keys::Provider::Pexels,
        "unsplash" => api_keys::Provider::Unsplash,
        other => return Err(format!("unknown provider: {other}")),
    };
    Ok(api_keys::probe(&state.http, provider, &key, Some(&state.quota)).await)
}

#[tauri::command]
async fn save_settings(
    settings: Settings,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // One settings write at a time, shared with the REST settings endpoints.
    let _save = config::SAVE_LOCK.lock().await;
    let path = state.paths.config_file.clone();
    let to_save = settings.clone();
    tokio::task::spawn_blocking(move || to_save.save(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    *state.settings.write().await = settings.clone();
    let bound = state.api_server.bound.read().await.clone();
    republish_discovery(state.paths.as_ref(), &settings, bound);
    Ok(())
}

// ---------- Entry point ----------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = Arc::new(AppPaths::discover().expect("failed to set up app paths"));
    let log_buffer = LogBuffer::new();
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let file_appender = tracing_appender::rolling::daily(&paths.logs, "mediabuddy.log");
        let (file_writer, file_guard) = tracing_appender::non_blocking(file_appender);
        let _file_guard = Box::leak(Box::new(file_guard));
        let _ = tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer())
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(file_writer)
                    .with_ansi(false),
            )
            .with(crate::logbuf::BufferLayer::new(log_buffer.clone()))
            .try_init();
    }

    tracing::info!("data directory: {}", paths.root.display());
    let leftovers = downloader::cleanup_partial_files(&paths);
    if leftovers > 0 {
        tracing::info!("removed {leftovers} unfinished download file(s) from a previous run");
    }

    let mut settings = Settings::load_or_default(&paths.config_file);
    // Ensure the REST server is never unauthenticated by default — generate
    // and persist a token whenever one is missing. This covers fresh installs
    // AND upgrades whose settings.json predates this feature. The desktop UI
    // uses Tauri IPC (not REST), so normal use is unaffected; external REST
    // clients use the token shown in Settings.
    if settings.api_token.trim().is_empty() {
        settings.api_token = generate_api_token();
        settings.api_auto_start = true;
        let _ = settings.save(&paths.config_file);
    }
    let manager =
        Arc::new(ImageManager::new((*paths).clone()).expect("failed to initialize image manager"));
    let media_paths = paths.clone();
    let media_manager = manager.clone();
    let topics_store = Arc::new(TopicStore::new(manager.conn_handle()));
    let user_agent = concat!(
        "MediaBuddy/",
        env!("CARGO_PKG_VERSION"),
        " (+https://github.com/aivrar/mediabuddy)"
    );
    let http = reqwest::Client::builder()
        .user_agent(user_agent)
        .build()
        .expect("failed to build http client");
    // Dedicated client for downloading caller-supplied media: no automatic
    // redirect following, so downloader::fetch revalidates each hop against
    // the SSRF guard before connecting.
    let download_http = reqwest::Client::builder()
        .user_agent(user_agent)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build download http client");

    let api_server = Arc::new(ApiServer::new());
    let auto_start_api = settings.api_auto_start;
    let api_host = settings.api_host.clone();
    let api_port = settings.api_port;
    let api_cors = settings.api_cors_enabled;

    let state = AppState {
        paths,
        settings: Arc::new(RwLock::new(settings)),
        image_manager: manager,
        http,
        download_http,
        system: Arc::new(SystemMonitor::new()),
        api_server,
        log_buffer,
        vision: Arc::new(VisionRegistry::new()),
        quota: Arc::new(QuotaTracker::new()),
        topics: topics_store,
    };

    let auto_start_ctx = if auto_start_api {
        Some((
            state.api_server.clone(),
            state.api_context(),
            state.paths.clone(),
            state.settings.clone(),
        ))
    } else {
        None
    };

    tauri::Builder::default()
        .register_uri_scheme_protocol("mediabuddy-media", move |_ctx, request| {
            media_protocol_response(media_paths.clone(), media_manager.clone(), request)
        })
        .plugin(tauri_plugin_opener::init())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                handle_close_request(window.app_handle());
            }
        })
        .setup(move |app| {
            unpublish_discovery();
            if let Some((server, ctx, paths, settings)) = auto_start_ctx {
                let runtime = tauri::async_runtime::handle();
                runtime.spawn(async move {
                    if let Err(e) = server.start(api_host, api_port, api_cors, ctx).await {
                        tracing::error!("auto-start API failed: {e}");
                    } else {
                        let snapshot = settings.read().await.clone();
                        let status = server.status().await;
                        if let Err(e) = publish_discovery(paths.as_ref(), &snapshot, &status) {
                            tracing::warn!("could not publish MediaBuddy discovery registry: {e}");
                        }
                        tracing::info!("auto-start API: REST server is up");
                    }
                });
            }
            let _ = app;
            Ok(())
        })
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            list_images,
            delete_images,
            is_url_saved,
            update_image,
            append_image_tags,
            search_images,
            download_images,
            validate_api_key,
            get_quota_status,
            topic_find_or_create,
            topic_status,
            topic_list,
            topic_get_more,
            topic_reset,
            topic_delete,
            topic_image_ids,
            topic_rename,
            topic_pending,
            topic_dismiss_pending,
            blocked_count,
            clear_blocked,
            close_warning_ack,
            active_work,
            get_settings,
            save_settings,
            get_system_stats,
            api_status,
            api_start,
            api_stop,
            shutdown_app,
            get_logs,
            clear_logs,
            read_thumb_bytes,
            read_image_bytes,
            get_data_root,
            vision_status,
            vision_load,
            vision_unload,
            vision_analyze_images,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Media Buddy")
        .run(|app_handle, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                api.prevent_exit();
                shutdown_app_process();
            }
            tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                api.prevent_close();
                handle_close_request(app_handle);
            }
            _ => {}
        });
}

#[cfg(test)]
mod discovery_tests {
    use super::{discovery_base_url, discovery_is_specific_non_loopback};

    #[test]
    fn wildcard_discovery_uses_reachable_local_address() {
        assert_eq!(discovery_base_url("0.0.0.0", 5000), "http://127.0.0.1:5000");
        assert_eq!(discovery_base_url("::", 5000), "http://127.0.0.1:5000");
        assert!(!discovery_is_specific_non_loopback("0.0.0.0"));
    }

    #[test]
    fn specific_interface_is_published_for_suite_discovery() {
        assert_eq!(
            discovery_base_url("172.28.64.1", 5000),
            "http://172.28.64.1:5000"
        );
        assert!(discovery_is_specific_non_loopback("172.28.64.1"));
    }

    #[test]
    fn ipv6_discovery_is_bracketed() {
        assert_eq!(discovery_base_url("fd00::1", 5000), "http://[fd00::1]:5000");
        assert!(discovery_is_specific_non_loopback("fd00::1"));
    }
}

#[cfg(test)]
mod close_tests {
    use super::{decide_close, CloseDecision, CloseWarning, CLOSE_DUPLICATE_WINDOW};
    use std::time::{Duration, Instant};

    fn fresh() -> CloseWarning {
        CloseWarning {
            warned_at: None,
            acknowledged: false,
        }
    }

    #[test]
    fn idle_app_closes_immediately() {
        assert_eq!(
            decide_close(false, &mut fresh(), Instant::now()),
            CloseDecision::Exit
        );
    }

    #[test]
    fn busy_app_warns_once_per_click() {
        let mut warning = fresh();
        let t0 = Instant::now();
        assert_eq!(decide_close(true, &mut warning, t0), CloseDecision::Warn);
        // The same click arriving through a second event hook.
        assert_eq!(
            decide_close(true, &mut warning, t0 + Duration::from_millis(5)),
            CloseDecision::Ignore
        );
    }

    #[test]
    fn acknowledged_warning_asks_again_next_time() {
        let mut warning = fresh();
        let t0 = Instant::now();
        assert_eq!(decide_close(true, &mut warning, t0), CloseDecision::Warn);
        warning.acknowledged = true; // UI showed the question; user said no
        assert_eq!(
            decide_close(true, &mut warning, t0 + Duration::from_secs(20)),
            CloseDecision::Warn
        );
    }

    #[test]
    fn unanswered_warning_never_traps_the_user() {
        let mut warning = fresh();
        let t0 = Instant::now();
        assert_eq!(decide_close(true, &mut warning, t0), CloseDecision::Warn);
        assert_eq!(
            decide_close(true, &mut warning, t0 + CLOSE_DUPLICATE_WINDOW * 2),
            CloseDecision::Exit
        );
    }
}
