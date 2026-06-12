//! Auto-download Florence-2 ONNX weights via HuggingFace Hub.
//!
//! HF's CDN serves files through xet (content-addressed, chunked,
//! deduplicated) under the hood. The sync `hf-hub` API uses ureq +
//! rustls and caches into the directory we hand it, so re-runs are
//! free after the first download.

use std::fs::File;
use std::io::{copy, Write};
use std::path::{Path, PathBuf};

use hf_hub::api::sync::ApiBuilder;

use crate::error::{AppError, Result};
use crate::vision::Precision;

const REPO: &str = "onnx-community/Florence-2-base-ft";
const HF_BASE_URL: &str = "https://huggingface.co";

/// Pin the HuggingFace model revision to a specific commit for supply-chain
/// reproducibility. `None` tracks the repo's default branch (current
/// behavior). Set this to a commit SHA obtained from a trusted source to
/// fully pin the weights; the TOFU integrity manifest (`integrity.json`)
/// still guards every downloaded file regardless of this setting.
const PINNED_REVISION: Option<&str> = None;

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

    let repo = match PINNED_REVISION {
        Some(rev) => api.repo(hf_hub::Repo::with_revision(
            REPO.to_string(),
            hf_hub::RepoType::Model,
            rev.to_string(),
        )),
        None => api.model(REPO.to_string()),
    };

    // TOFU integrity manifest shared with the onnxruntime DLL check.
    let manifest = cache_dir.join("integrity.json");
    let fetch = |rel: &str| -> Result<PathBuf> {
        tracing::info!("hf-hub: fetching {REPO}/{rel}");
        let p = repo
            .get(rel)
            .map_err(|e| AppError::other(format!("download {rel}: {e}")))?;
        super::integrity::verify_or_record(&manifest, rel, &p)?;
        Ok(p)
    };

    let vision_encoder = fetch(&format!("onnx/vision_encoder{suffix}.onnx"))?;
    let embed_tokens = fetch(&format!("onnx/embed_tokens{suffix}.onnx"))?;
    let encoder_model = fetch(&format!("onnx/encoder_model{suffix}.onnx"))?;
    let decoder_model = fetch(&format!("onnx/decoder_model{suffix}.onnx"))?;
    let tokenizer = match direct_download_tokenizer(cache_dir, &manifest) {
        Ok(path) => path,
        Err(direct_err) => {
            tracing::warn!(
                "direct tokenizer.json fetch failed ({direct_err}); trying hf-hub fallback"
            );
            match fetch("tokenizer.json") {
                Ok(path) => {
                    validate_tokenizer_file(&path)?;
                    path
                }
                Err(hub_err) => {
                    return Err(AppError::other(format!(
                        "download tokenizer.json failed: direct resolve failed ({direct_err}); hf-hub failed ({hub_err})"
                    )));
                }
            }
        }
    };

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
            super::integrity::verify_or_record(&manifest, &name, &p)?;
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

fn direct_download_tokenizer(cache_dir: &Path, manifest: &Path) -> Result<PathBuf> {
    let out_dir = cache_dir.join("manual").join(REPO.replace('/', "--"));
    std::fs::create_dir_all(&out_dir)?;

    let out = out_dir.join("tokenizer.json");
    if out.exists() {
        match validate_tokenizer_file(&out) {
            Ok(()) => {
                tracing::debug!("using cached tokenizer.json -> {}", out.display());
                super::integrity::verify_or_record(manifest, "tokenizer.json", &out)?;
                return Ok(out);
            }
            Err(err) => {
                tracing::warn!(
                    "cached tokenizer.json at {} is invalid ({err}); redownloading",
                    out.display()
                );
                std::fs::remove_file(&out)?;
            }
        }
    }

    let revision = PINNED_REVISION.unwrap_or("main");
    let url = format!("{HF_BASE_URL}/{REPO}/resolve/{revision}/tokenizer.json");
    tracing::info!("hf-hub: direct fetching {url}");

    let response = ureq::get(&url)
        .call()
        .map_err(|e| AppError::other(format!("download tokenizer.json direct: {e}")))?;

    let tmp = out.with_extension("json.tmp");
    let mut reader = response.into_reader();
    let mut file = File::create(&tmp)?;
    copy(&mut reader, &mut file)?;
    file.flush()?;
    drop(file);

    validate_tokenizer_file(&tmp)?;
    std::fs::rename(&tmp, &out)?;
    super::integrity::verify_or_record(manifest, "tokenizer.json", &out)?;
    Ok(out)
}

fn validate_tokenizer_file(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() < 1024 {
        return Err(AppError::other(format!(
            "tokenizer.json at {} is too small ({} bytes)",
            path.display(),
            metadata.len()
        )));
    }

    let text = std::fs::read_to_string(path)?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        AppError::other(format!(
            "tokenizer.json at {} is not valid JSON: {e}",
            path.display()
        ))
    })?;

    if json.get("model").is_none() || json.get("pre_tokenizer").is_none() {
        return Err(AppError::other(format!(
            "tokenizer.json at {} is missing tokenizer fields",
            path.display()
        )));
    }

    Ok(())
}
