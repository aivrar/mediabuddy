mod api_server;
mod config;
mod db;
mod downloader;
mod error;
mod image_manager;
mod logbuf;
mod paths;
mod search;
mod system_monitor;
mod types;
mod vision;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::api_server::{ApiContext, ApiServer, ApiStatus};
use crate::config::Settings;
use crate::image_manager::ImageManager;
use crate::logbuf::{LogBuffer, LogEntry};
use crate::paths::AppPaths;
use crate::search::SearchResult;
use crate::system_monitor::{SystemMonitor, SystemStats};
use crate::types::{DeleteResult, Image};
use crate::vision::{Precision, VisionRegistry, VisionStatus};

pub struct AppState {
    pub paths: Arc<AppPaths>,
    pub settings: Arc<RwLock<Settings>>,
    pub image_manager: Arc<ImageManager>,
    pub http: reqwest::Client,
    pub system: Arc<SystemMonitor>,
    pub api_server: Arc<ApiServer>,
    pub log_buffer: LogBuffer,
    pub vision: Arc<VisionRegistry>,
}

impl AppState {
    fn api_context(&self) -> ApiContext {
        ApiContext {
            paths: self.paths.clone(),
            settings: self.settings.clone(),
            image_manager: self.image_manager.clone(),
            http: self.http.clone(),
            system: self.system.clone(),
            tasks: self.api_server.tasks.clone(),
            started_at: self.api_server.started_at.clone(),
            vision: self.vision.clone(),
        }
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

// ---------- Search & download ----------

#[tauri::command]
async fn search_images(
    query: String,
    sources: HashMap<String, u32>,
    kind: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<SearchResult>, String> {
    let client = state.http.clone();
    let settings = state.settings.clone();
    let manager = state.image_manager.clone();
    let kind = search::Kind::from_str(kind.as_deref().unwrap_or("photo"));
    let raw = search::search_all(&client, settings, query, sources, kind)
        .await
        .map_err(|e| e.to_string())?;
    let filtered: Vec<SearchResult> = raw
        .into_iter()
        .filter(|r| !r.url.is_empty() && !manager.is_url_saved(&r.url))
        .collect();
    Ok(filtered)
}

#[tauri::command]
async fn download_images(
    results: Vec<SearchResult>,
    preview_only: Option<bool>,
    concurrency: Option<usize>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Image>, String> {
    let client = state.http.clone();
    let manager = state.image_manager.clone();
    let saved = downloader::download_many(
        client,
        manager,
        results,
        preview_only.unwrap_or(false),
        concurrency.unwrap_or(8),
    )
    .await;
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
    #[serde(default = "default_vision_count")]
    count: usize,
}

fn default_vision_count() -> usize {
    1
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
    let model_dir = state.paths.models.join("florence2-base-ft");
    let vision = state.vision.clone();
    tokio::task::spawn_blocking(move || vision.load(&model_dir, precision, params.count))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    Ok(state.vision.status())
}

#[tauri::command]
async fn vision_unload(state: tauri::State<'_, AppState>) -> Result<VisionStatus, String> {
    state.vision.unload_all();
    Ok(state.vision.status())
}

fn parse_precision(s: Option<&str>) -> Precision {
    match s.map(|s| s.to_ascii_lowercase()) {
        Some(s) if s == "fp32" => Precision::Fp32,
        Some(s) if s == "int8" => Precision::Int8,
        Some(s) if s == "q4f16" || s == "q4" => Precision::Q4f16,
        _ => Precision::Fp16,
    }
}

// ---------- Image binary access ----------

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
    let rel = if !img.thumb_path.is_empty() { img.thumb_path } else { img.path };
    if rel.is_empty() {
        return Err("no thumbnail or original path on record".into());
    }
    let path = state.paths.root.join(&rel);
    tokio::fs::read(&path).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn get_data_root(state: tauri::State<'_, AppState>) -> String {
    state.paths.root.to_string_lossy().to_string()
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
        .start(snapshot.api_host.clone(), snapshot.api_port, snapshot.api_cors_enabled, ctx)
        .await
        .map_err(|e| e.to_string())?;
    Ok(state.api_server.status().await)
}

#[tauri::command]
async fn api_stop(state: tauri::State<'_, AppState>) -> Result<ApiStatus, String> {
    state
        .api_server
        .stop()
        .await
        .map_err(|e| e.to_string())?;
    Ok(state.api_server.status().await)
}

// ---------- Settings ----------

#[tauri::command]
async fn get_settings(state: tauri::State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.settings.read().await.clone())
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
    let log_buffer = LogBuffer::new();
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let _ = tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer())
            .with(crate::logbuf::BufferLayer::new(log_buffer.clone()))
            .try_init();
    }

    let paths = Arc::new(AppPaths::discover().expect("failed to set up app paths"));
    let settings = Settings::load_or_default(&paths.config_file);
    if !paths.config_file.exists() {
        let _ = settings.save(&paths.config_file);
    }
    let manager = Arc::new(
        ImageManager::new((*paths).clone()).expect("failed to initialize image manager"),
    );
    let http = reqwest::Client::builder()
        .user_agent(concat!(
            "MediaBuddy/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/aivrar/mediabuddy)"
        ))
        .build()
        .expect("failed to build http client");

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
        system: Arc::new(SystemMonitor::new()),
        api_server,
        log_buffer,
        vision: Arc::new(VisionRegistry::new()),
    };

    let auto_start_ctx = if auto_start_api {
        Some((state.api_server.clone(), state.api_context()))
    } else {
        None
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            if let Some((server, ctx)) = auto_start_ctx {
                let runtime = tauri::async_runtime::handle();
                runtime.spawn(async move {
                    if let Err(e) = server.start(api_host, api_port, api_cors, ctx).await {
                        tracing::error!("auto-start API failed: {e}");
                    } else {
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
            search_images,
            download_images,
            get_settings,
            save_settings,
            get_system_stats,
            api_status,
            api_start,
            api_stop,
            get_logs,
            clear_logs,
            read_thumb_bytes,
            get_data_root,
            vision_status,
            vision_load,
            vision_unload,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Media Buddy");
}
