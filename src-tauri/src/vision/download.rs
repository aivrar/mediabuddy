//! Auto-download Florence-2 ONNX weights via HuggingFace Hub.
//!
//! HF's CDN serves files through xet (content-addressed, chunked,
//! deduplicated) under the hood. The sync `hf-hub` API uses ureq +
//! rustls and caches into the directory we hand it, so re-runs are
//! free after the first download.

use std::path::{Path, PathBuf};

use hf_hub::api::sync::ApiBuilder;

use crate::error::{AppError, Result};
use crate::vision::Precision;

const REPO: &str = "onnx-community/Florence-2-base-ft";

#[derive(Debug, Clone)]
pub struct ModelPaths {
    pub vision_encoder: PathBuf,
    pub embed_tokens: PathBuf,
    pub encoder_model: PathBuf,
    pub decoder_model: PathBuf,
    pub tokenizer: PathBuf,
}

/// Resolve every Florence-2 file we need for `precision`, downloading
/// any that aren't already in `cache_dir`. Caller passes the directory
/// where HF's cache layout will live (e.g. `<data>/models`).
pub fn ensure_florence2_models(cache_dir: &Path, precision: Precision) -> Result<ModelPaths> {
    std::fs::create_dir_all(cache_dir)?;
    let suffix = precision.suffix();

    let api = ApiBuilder::new()
        .with_cache_dir(cache_dir.to_path_buf())
        .with_progress(true)
        .build()
        .map_err(|e| AppError::other(format!("hf-hub api build failed: {e}")))?;

    let repo = api.model(REPO.to_string());

    let fetch = |rel: &str| -> Result<PathBuf> {
        tracing::info!("hf-hub: fetching {REPO}/{rel}");
        repo.get(rel)
            .map_err(|e| AppError::other(format!("download {rel}: {e}")))
    };

    let vision_encoder = fetch(&format!("onnx/vision_encoder{suffix}.onnx"))?;
    let embed_tokens = fetch(&format!("onnx/embed_tokens{suffix}.onnx"))?;
    let encoder_model = fetch(&format!("onnx/encoder_model{suffix}.onnx"))?;
    let decoder_model = fetch(&format!("onnx/decoder_model{suffix}.onnx"))?;
    let tokenizer = fetch("tokenizer.json")?;

    // Some ONNX exports ship weights as a separate `*.onnx_data` blob next
    // to the graph. ort loads them automatically when they sit beside the
    // .onnx file in the same directory, so we eagerly fetch them when they
    // exist on the hub. Missing ones are fine — silently ignore.
    for name in [
        format!("onnx/vision_encoder{suffix}.onnx_data"),
        format!("onnx/embed_tokens{suffix}.onnx_data"),
        format!("onnx/encoder_model{suffix}.onnx_data"),
        format!("onnx/decoder_model{suffix}.onnx_data"),
    ] {
        if let Ok(p) = repo.get(&name) {
            tracing::info!("hf-hub: fetched external weights {name} -> {}", p.display());
        }
    }

    Ok(ModelPaths {
        vision_encoder,
        embed_tokens,
        encoder_model,
        decoder_model,
        tokenizer,
    })
}
