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
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use futures_util::{stream, StreamExt};
use serde::Serialize;
use serde_json::json;
use tauri::http::{header, Request, Response, StatusCode};
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
    caption_task_from_name, parse_caption_write_mode, should_write_caption, CaptionWriteMode,
    Precision, VisionAnalysisOptions, VisionExecutionMode, VisionLoadOptions, VisionRegistry,
    VisionStatus,
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

fn publish_discovery(
    paths: &AppPaths,
    settings: &Settings,
    status: &ApiStatus,
) -> Result<(), String> {
    let Some(port) = status.port else {
        return Ok(());
    };
    let dir = apphub_registry_dir()
        .ok_or_else(|| "could not resolve AppHub registry directory".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("mediabuddy.json");
    let api = format!("http://127.0.0.1:{port}");
    let payload = json!({
        "name": "mediabuddy",
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "started_at": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        "app_dir": app_dir_for_discovery(),
        "endpoints": {
            "api": api,
            "status": format!("http://127.0.0.1:{port}/api/v1/status")
        },
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
            "api_host": status.host.clone().unwrap_or_else(|| settings.api_host.clone())
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
) -> Result<bool, String> {
    let mgr = state.image_manager.clone();
    tokio::task::spawn_blocking(move || mgr.update_image_metadata(&id, alt, tags, None))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
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
                && !manager.is_source_id_saved(&r.source, &r.source_id)
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
    #[serde(default)]
    load_if_needed: Option<bool>,
    #[serde(default)]
    concurrency: Option<usize>,
}

#[derive(Debug, Clone)]
struct LibraryAnalyzeOptions {
    detect_objects: bool,
    caption_mode: CaptionWriteMode,
    caption_min_chars: usize,
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
    state.vision.unload_all();
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
    let caption_min_chars = params.caption_min_chars.unwrap_or(80).clamp(1, 1000);
    let caption_task = (caption_mode != CaptionWriteMode::Skip)
        .then(|| caption_task_from_name(params.caption_task.as_deref()).to_string());
    let max_concurrency = state.vision.status().instances.clamp(1, 32);
    let concurrency = params
        .concurrency
        .unwrap_or(max_concurrency)
        .clamp(1, max_concurrency);
    tracing::info!(
        "Florence-2 analysis started: {} image(s), concurrency {}, detect_objects={}, caption_mode={}, caption_task={}",
        ids.len(),
        concurrency,
        detect_objects,
        caption_mode.as_str(),
        caption_task.as_deref().unwrap_or("none")
    );

    let paths = state.paths.clone();
    let manager = state.image_manager.clone();
    let vision = state.vision.clone();
    let analyze_options = LibraryAnalyzeOptions {
        detect_objects,
        caption_mode,
        caption_min_chars,
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

fn merge_object_tags(
    existing: &[String],
    objects: &[crate::vision::DetectedObject],
) -> (Vec<String>, Vec<String>) {
    let mut merged = Vec::with_capacity(existing.len() + objects.len());
    let mut seen = HashSet::new();
    for tag in existing {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_ascii_lowercase()) {
            merged.push(trimmed.to_string());
        }
    }

    let mut added = Vec::new();
    for object in objects {
        let label = object.label.trim();
        if label.is_empty() {
            continue;
        }
        if seen.insert(label.to_ascii_lowercase()) {
            let tag = label.to_string();
            merged.push(tag.clone());
            added.push(tag);
        }
    }
    (merged, added)
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
                image: Some(image),
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
                image: Some(image),
            };
        }
    };

    let caption = analysis.caption.trim().to_string();
    let caption_written = should_write_caption(
        options.caption_mode,
        &image.alt,
        &caption,
        options.caption_min_chars,
    );
    let caption_patch = caption_written.then_some(caption.clone());
    let (merged_tags, tags_added) = merge_object_tags(&image.tags, &analysis.objects);
    let tags_patch = (!tags_added.is_empty()).then_some(merged_tags);

    let update_result = manager.update_image_metadata(&id, caption_patch, tags_patch, Some(true));
    if let Err(e) = update_result {
        return VisionAnalyzeItem {
            id,
            ok: false,
            skipped: false,
            error: Some(e.to_string()),
            caption: Some(caption),
            caption_written,
            tags_added,
            objects: analysis.objects,
            image: Some(image),
        };
    }

    let updated = manager.get_image_by_id(&id).ok().flatten().unwrap_or(image);
    VisionAnalyzeItem {
        id,
        ok: true,
        skipped: false,
        error: None,
        caption: Some(caption),
        caption_written,
        tags_added,
        objects: analysis.objects,
        image: Some(updated),
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

fn resolve_library_media_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
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

fn parse_media_range(header_value: &str, len: u64) -> Option<(u64, u64)> {
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
    let path = state.paths.config_file.clone();
    let to_save = settings.clone();
    tokio::task::spawn_blocking(move || to_save.save(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    *state.settings.write().await = settings;
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
                let _ = window;
                shutdown_app_process();
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
        .run(|_app_handle, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                api.prevent_exit();
                shutdown_app_process();
            }
            tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                api.prevent_close();
                shutdown_app_process();
            }
            _ => {}
        });
}
