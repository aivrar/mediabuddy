//! Florence-2 vision engine.
//!
//! Loads four ONNX models (vision encoder, text embeddings, encoder,
//! decoder-with-KV-cache) plus the Hugging Face tokenizer, and runs the
//! standard Florence-2 caption / object-detection / OCR task prompts.
//!
//! Model layout expected under `<data>/models/florence2-base-ft/`:
//!
//! ```text
//! models/florence2-base-ft/
//! ├── tokenizer.json
//! ├── preprocessor_config.json
//! ├── config.json
//! └── onnx/
//!     ├── vision_encoder_fp16.onnx
//!     ├── embed_tokens_fp16.onnx
//!     ├── encoder_model_fp16.onnx
//!     └── decoder_model_merged_fp16.onnx
//! ```
//!
//! The engine is loaded lazily on the first analysis request and can be
//! unloaded to free GPU/CPU memory.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

#[allow(dead_code)]
mod preprocess;

/// Florence-2 task prompts.
#[allow(dead_code)]
pub const TASK_CAPTION: &str = "<CAPTION>";
#[allow(dead_code)]
pub const TASK_DETAILED_CAPTION: &str = "<DETAILED_CAPTION>";
#[allow(dead_code)]
pub const TASK_MORE_DETAILED_CAPTION: &str = "<MORE_DETAILED_CAPTION>";
#[allow(dead_code)]
pub const TASK_OBJECT_DETECTION: &str = "<OD>";
#[allow(dead_code)]
pub const TASK_DENSE_REGION_CAPTION: &str = "<DENSE_REGION_CAPTION>";
#[allow(dead_code)]
pub const TASK_REGION_PROPOSAL: &str = "<REGION_PROPOSAL>";
#[allow(dead_code)]
pub const TASK_OCR: &str = "<OCR>";

/// What variant to load — controls the model file suffix and inference precision.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
pub struct AnalysisResult {
    pub caption: String,
    pub objects: Vec<String>,
}

/// Holds zero-or-more loaded inference engines.  When empty the registry is
/// considered "unloaded" and analyse calls return an error.
pub struct VisionRegistry {
    inner: Mutex<RegistryState>,
}

struct RegistryState {
    instances: Vec<Arc<VisionEngine>>,
    precision: Option<Precision>,
    model_dir: Option<PathBuf>,
}

impl VisionRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RegistryState {
                instances: Vec::new(),
                precision: None,
                model_dir: None,
            }),
        }
    }

    pub fn status(&self) -> VisionStatus {
        let st = self.inner.lock().unwrap();
        VisionStatus {
            loaded: !st.instances.is_empty(),
            instances: st.instances.len(),
            precision: st.precision.map(|p| format!("{:?}", p).to_lowercase()),
            model_dir: st
                .model_dir
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
        }
    }

    pub fn unload_all(&self) {
        let mut st = self.inner.lock().unwrap();
        st.instances.clear();
        st.precision = None;
        st.model_dir = None;
    }

    /// Stub: model loading is implemented in a follow-up session.  Returns
    /// a clear error for now so callers see "not yet implemented" instead
    /// of a generic 500.
    pub fn load(&self, model_dir: &Path, precision: Precision, _count: usize) -> Result<usize> {
        if !verify_model_files(model_dir, precision) {
            return Err(AppError::other(format!(
                "Florence-2 model files not found at {:?}. Expected onnx/vision_encoder{0}.onnx \
                 and friends. Download from https://huggingface.co/onnx-community/Florence-2-base-ft.",
                precision.suffix()
            )));
        }
        Err(AppError::other(
            "Florence-2 ONNX inference engine isn't wired up in this build yet. \
             Vision analysis routes will activate in a follow-up release.",
        ))
    }

    pub fn analyse(&self, _image_path: &Path, _need_objects: bool) -> Result<AnalysisResult> {
        Err(AppError::other(
            "Vision engine is not yet wired up. Load + analyse will activate \
             once the ONNX inference pipeline ships.",
        ))
    }
}

/// Placeholder for a single inference engine.  Real implementation will hold
/// `ort::Session` handles for all four ONNX models plus a `tokenizers::Tokenizer`.
#[allow(dead_code)]
pub struct VisionEngine {
    model_dir: PathBuf,
    precision: Precision,
}

fn verify_model_files(dir: &Path, precision: Precision) -> bool {
    let suffix = precision.suffix();
    let needed = [
        format!("onnx/vision_encoder{}.onnx", suffix),
        format!("onnx/embed_tokens{}.onnx", suffix),
        format!("onnx/encoder_model{}.onnx", suffix),
        format!("onnx/decoder_model_merged{}.onnx", suffix),
    ];
    needed.iter().all(|rel| dir.join(rel).exists())
        && dir.join("tokenizer.json").exists()
}
