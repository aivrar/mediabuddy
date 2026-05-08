//! Florence-2 vision engine.
//!
//! Loads four ONNX models (vision encoder, text embeddings, BART encoder,
//! BART decoder) plus the Hugging Face tokenizer, and runs Florence-2's
//! task prompts (`<CAPTION>`, `<OD>`, `<OCR>`, …).
//!
//! Weights are auto-downloaded from HuggingFace
//! `onnx-community/Florence-2-base-ft` on first load. The cache lives
//! under `<data>/models/` and is reused across runs.
//!
//! The engine is loaded lazily via `vision_load` and can be unloaded
//! to free RAM with `vision_unload`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

mod download;
mod inference;
mod preprocess;
mod runtime;

static ORT_INIT: Once = Once::new();
#[allow(static_mut_refs)]
static mut ORT_INIT_ERR: Option<String> = None;

use inference::VisionEngine;

/// Florence-2 task prompts. See model card for the full list.
#[allow(dead_code)]
pub const TASK_CAPTION: &str = "<CAPTION>";
pub const TASK_DETAILED_CAPTION: &str = "<DETAILED_CAPTION>";
#[allow(dead_code)]
pub const TASK_MORE_DETAILED_CAPTION: &str = "<MORE_DETAILED_CAPTION>";
pub const TASK_OBJECT_DETECTION: &str = "<OD>";
#[allow(dead_code)]
pub const TASK_DENSE_REGION_CAPTION: &str = "<DENSE_REGION_CAPTION>";
#[allow(dead_code)]
pub const TASK_REGION_PROPOSAL: &str = "<REGION_PROPOSAL>";
#[allow(dead_code)]
pub const TASK_OCR: &str = "<OCR>";

/// Which weight variant to download and load.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Precision {
    Fp32,
    Fp16,
    Int8,
    Q4f16,
}

impl Precision {
    pub fn suffix(self) -> &'static str {
        match self {
            Precision::Fp32 => "",
            Precision::Fp16 => "_fp16",
            Precision::Int8 => "_int8",
            Precision::Q4f16 => "_q4f16",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionStatus {
    pub loaded: bool,
    pub instances: usize,
    pub precision: Option<String>,
    pub model_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedObject {
    pub label: String,
    /// `[x1, y1, x2, y2]` in pixel coordinates of the source image.
    pub bbox: [f32; 4],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub caption: String,
    pub objects: Vec<DetectedObject>,
}

pub struct VisionRegistry {
    inner: Mutex<RegistryState>,
    next_idx: AtomicUsize,
}

struct RegistryState {
    instances: Vec<Arc<VisionEngine>>,
    precision: Option<Precision>,
    cache_dir: Option<PathBuf>,
}

impl VisionRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RegistryState {
                instances: Vec::new(),
                precision: None,
                cache_dir: None,
            }),
            next_idx: AtomicUsize::new(0),
        }
    }

    pub fn status(&self) -> VisionStatus {
        let st = self.inner.lock().unwrap();
        VisionStatus {
            loaded: !st.instances.is_empty(),
            instances: st.instances.len(),
            precision: st.precision.map(|p| format!("{p:?}").to_lowercase()),
            model_dir: st
                .cache_dir
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
        }
    }

    pub fn unload_all(&self) {
        let mut st = self.inner.lock().unwrap();
        st.instances.clear();
        st.precision = None;
        st.cache_dir = None;
        self.next_idx.store(0, Ordering::Relaxed);
    }

    /// Download (if needed) and load the Florence-2 ONNX engines.
    /// `cache_dir` is the root for HF's cache layout — typically
    /// `<data>/models/`.
    pub fn load(&self, cache_dir: &Path, precision: Precision, count: usize) -> Result<usize> {
        if !matches!(precision, Precision::Fp32) {
            return Err(AppError::other(format!(
                "Florence-2 precision {precision:?} isn't wired up yet on the CPU execution \
                 provider — the f16/int8/q4 ONNX exports use non-f32 input dtypes that need \
                 client-side casting. Use `fp32` for now; quantized variants will activate \
                 alongside the DirectML/CUDA EP."
            )));
        }

        // 1. Ensure onnxruntime.dll is on disk and ort is initialized.
        ensure_ort_initialized(cache_dir)?;

        // 2. Download Florence-2 weights via HF Hub (xet-backed CDN).
        let paths = download::ensure_florence2_models(cache_dir, precision)?;

        let count = count.clamp(1, 4);
        let mut instances = Vec::with_capacity(count);
        for _ in 0..count {
            instances.push(Arc::new(VisionEngine::open(&paths, precision)?));
        }

        let mut st = self.inner.lock().unwrap();
        st.instances = instances;
        st.precision = Some(precision);
        st.cache_dir = Some(cache_dir.to_path_buf());
        self.next_idx.store(0, Ordering::Relaxed);
        Ok(st.instances.len())
    }

    fn pick(&self) -> Option<Arc<VisionEngine>> {
        let st = self.inner.lock().unwrap();
        if st.instances.is_empty() {
            return None;
        }
        let i = self.next_idx.fetch_add(1, Ordering::Relaxed) % st.instances.len();
        Some(st.instances[i].clone())
    }

    pub fn analyse(&self, image_path: &Path, need_objects: bool) -> Result<AnalysisResult> {
        let engine = self.pick().ok_or_else(|| {
            AppError::other(
                "Florence-2 model is not loaded. Call vision_load (or POST /api/v1/vision/load) first.",
            )
        })?;
        let caption = engine.caption(image_path, TASK_DETAILED_CAPTION)?;
        let objects = if need_objects {
            engine.detect_objects(image_path, TASK_OBJECT_DETECTION)?
        } else {
            Vec::new()
        };
        Ok(AnalysisResult { caption, objects })
    }
}

/// Download `onnxruntime.dll` if missing and run `ort::init_from()` exactly
/// once for the lifetime of the process. ort's environment is process-wide
/// and cannot be re-initialized — so subsequent calls are no-ops, but a
/// failure during the first init is sticky and returned on every call.
fn ensure_ort_initialized(cache_dir: &Path) -> Result<()> {
    let dll_path = runtime::ensure_onnxruntime_dll(cache_dir)?;
    ORT_INIT.call_once(|| {
        // SAFETY: setting an env var early in the process before any other
        // thread reads it; ort consults ORT_DYLIB_PATH on its first init.
        unsafe {
            std::env::set_var("ORT_DYLIB_PATH", &dll_path);
        }
        if let Err(e) = ort::init_from(dll_path.to_string_lossy().to_string())
            .with_name("mediabuddy")
            .commit()
        {
            // SAFETY: only written here, inside call_once, before any other
            // thread can read it (subsequent threads will see it under the
            // same Once barrier).
            unsafe {
                ORT_INIT_ERR = Some(format!("ort init failed: {e}"));
            }
        }
    });
    // SAFETY: only read after the Once barrier above.
    if let Some(msg) = unsafe { ORT_INIT_ERR.as_deref() } {
        return Err(AppError::other(msg.to_string()));
    }
    Ok(())
}
