use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

fn default_theme() -> String {
    "dark".into()
}
fn default_api_host() -> String {
    "127.0.0.1".into()
}
fn default_api_port() -> u16 {
    5000
}
fn default_api_auto_start() -> bool {
    true
}
fn default_api_cors_enabled() -> bool {
    // Default OFF. CORS only matters for browser-origin callers, and an
    // open/permissive CORS policy on a loopback control API lets any web
    // page the user visits read responses cross-origin. When enabled, the
    // server restricts allowed origins to loopback (see api_server).
    false
}
fn default_max_per_gpu() -> u32 {
    4
}
fn default_max_total() -> u32 {
    8
}
fn default_reserved_vram() -> f32 {
    0.5
}
fn default_cpu_instances() -> u32 {
    1
}
fn default_cpu_threads_per_instance() -> u32 {
    0
}
fn default_vision_execution_mode() -> String {
    "auto".into()
}
fn default_vision_allow_cpu() -> bool {
    true
}
fn default_unsplash_detail_threshold() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub pixabay_key: String,
    #[serde(default)]
    pub pexels_key: String,
    #[serde(default)]
    pub unsplash_key: String,

    #[serde(default = "default_theme")]
    pub theme: String,

    // Vision (Florence-2) config — wired up in vision phase
    #[serde(default)]
    pub vision_auto_load: bool,
    #[serde(default)]
    pub vision_auto_unload: bool,
    #[serde(default = "default_vision_allow_cpu")]
    pub vision_allow_cpu: bool,
    #[serde(default = "default_vision_execution_mode")]
    pub vision_execution_mode: String,
    #[serde(default = "default_cpu_instances")]
    pub vision_cpu_instances: u32,
    #[serde(default = "default_cpu_threads_per_instance")]
    pub vision_cpu_threads_per_instance: u32,
    #[serde(default = "default_max_per_gpu")]
    pub vision_max_per_gpu: u32,
    #[serde(default = "default_max_total")]
    pub vision_max_total: u32,
    #[serde(default = "default_reserved_vram")]
    pub vision_reserved_vram: f32,

    // REST API server config
    #[serde(default = "default_api_host")]
    pub api_host: String,
    #[serde(default = "default_api_port")]
    pub api_port: u16,
    #[serde(default = "default_api_auto_start")]
    pub api_auto_start: bool,
    #[serde(default = "default_api_cors_enabled")]
    pub api_cors_enabled: bool,

    /// When a single search is asked for more than this many Unsplash items,
    /// the per-result detail fetch is skipped. Each detail fetch is a
    /// separate API call that counts against Unsplash's hourly quota
    /// (50/hr on the free tier), so this protects you from accidentally
    /// burning your quota on one large search.
    #[serde(default = "default_unsplash_detail_threshold")]
    pub unsplash_detail_threshold: u32,

    /// Bearer token for the REST API server. When non-empty, every
    /// /api/v1/* request must carry `Authorization: Bearer <token>` (or
    /// `x-api-token: <token>`). A random token is auto-generated on first
    /// run. An empty token only keeps the server open on a loopback bind;
    /// binding a non-loopback host with an empty token is refused.
    #[serde(default)]
    pub api_token: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pixabay_key: String::new(),
            pexels_key: String::new(),
            unsplash_key: String::new(),
            theme: default_theme(),
            vision_auto_load: false,
            vision_auto_unload: false,
            vision_allow_cpu: default_vision_allow_cpu(),
            vision_execution_mode: default_vision_execution_mode(),
            vision_cpu_instances: default_cpu_instances(),
            vision_cpu_threads_per_instance: default_cpu_threads_per_instance(),
            vision_max_per_gpu: default_max_per_gpu(),
            vision_max_total: default_max_total(),
            vision_reserved_vram: default_reserved_vram(),
            api_host: default_api_host(),
            api_port: default_api_port(),
            api_auto_start: default_api_auto_start(),
            api_cors_enabled: default_api_cors_enabled(),
            unsplash_detail_threshold: default_unsplash_detail_threshold(),
            api_token: String::new(),
        }
    }
}

impl Settings {
    pub fn load_or_default(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|err| {
                tracing::warn!(
                    "settings file at {:?} was malformed, using defaults: {}",
                    path,
                    err
                );
                // Preserve the unparseable file (it may contain recoverable
                // API keys) instead of silently overwriting it with defaults.
                let _ = std::fs::rename(path, path.with_extension("json.corrupt"));
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        // Write to a temp file and atomically rename into place, so a crash
        // or concurrent read mid-write can never truncate/corrupt the real
        // settings file (which holds provider keys + the REST token).
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}
