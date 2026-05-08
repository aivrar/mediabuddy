//! Auto-download `onnxruntime.dll` for `ort`'s load-dynamic mode.
//!
//! With `ort = { features = ["load-dynamic"] }` the runtime is loaded
//! at process start via `ort::init_from(path)`, so we need a real
//! `onnxruntime.dll` somewhere on disk. We grab the official Windows
//! x64 build from the microsoft/onnxruntime GitHub release zip and
//! cache it under `<data>/runtime/`.
//!
//! The download is one-time; subsequent loads are no-ops once the dll
//! is present.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};

const ORT_VERSION: &str = "1.20.1";

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const ORT_ZIP_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download/v1.20.1/onnxruntime-win-x64-1.20.1.zip";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const ORT_DLL_NAME: &str = "onnxruntime.dll";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const ORT_DLL_INNER_PATH: &str = "onnxruntime-win-x64-1.20.1/lib/onnxruntime.dll";

/// Ensure `onnxruntime.dll` exists in `cache_dir/runtime/` and return its path.
/// Downloads + extracts if missing. Idempotent.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub fn ensure_onnxruntime_dll(cache_dir: &Path) -> Result<PathBuf> {
    let runtime_dir = cache_dir.join("runtime");
    std::fs::create_dir_all(&runtime_dir)?;
    let dll_path = runtime_dir.join(ORT_DLL_NAME);
    if dll_path.exists() {
        return Ok(dll_path);
    }

    tracing::info!(
        "fetching ONNX Runtime {ORT_VERSION} -> {}",
        dll_path.display()
    );
    let bytes = http_get_blocking(ORT_ZIP_URL)?;
    extract_one(&bytes, ORT_DLL_INNER_PATH, &dll_path)?;
    Ok(dll_path)
}

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
pub fn ensure_onnxruntime_dll(_cache_dir: &Path) -> Result<PathBuf> {
    Err(AppError::other(
        "Auto-download of onnxruntime is only wired up for Windows x64 in this build. \
         Install onnxruntime manually and set ORT_DYLIB_PATH.",
    ))
}

fn http_get_blocking(url: &str) -> Result<Vec<u8>> {
    // Use the existing reqwest blocking-via-tokio stack would require a runtime.
    // Use ureq (already pulled in by hf-hub) for a simple sync GET.
    let resp = ureq::get(url)
        .call()
        .map_err(|e| AppError::other(format!("ort dll download: {e}")))?;
    let mut buf = Vec::with_capacity(15 * 1024 * 1024);
    resp.into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| AppError::other(format!("ort dll read: {e}")))?;
    Ok(buf)
}

fn extract_one(zip_bytes: &[u8], inner: &str, out_path: &Path) -> Result<()> {
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| AppError::other(format!("ort zip: {e}")))?;
    let mut file = archive
        .by_name(inner)
        .map_err(|e| AppError::other(format!("entry {inner} not found: {e}")))?;
    let mut out = std::fs::File::create(out_path)?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| AppError::other(format!("ort dll extract: {e}")))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
    }
    Ok(())
}
