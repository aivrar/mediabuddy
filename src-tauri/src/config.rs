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
    #[serde(default)]
    pub vision_allow_cpu: bool,
    #[serde(default = "default_cpu_instances")]
    pub vision_cpu_instances: u32,
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
    #[serde(default)]
    pub api_auto_start: bool,
    #[serde(default)]
    pub api_cors_enabled: bool,
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
            vision_allow_cpu: false,
            vision_cpu_instances: default_cpu_instances(),
            vision_max_per_gpu: default_max_per_gpu(),
            vision_max_total: default_max_total(),
            vision_reserved_vram: default_reserved_vram(),
            api_host: default_api_host(),
            api_port: default_api_port(),
            api_auto_start: false,
            api_cors_enabled: false,
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
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }
}
