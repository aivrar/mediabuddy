//! Trust-on-first-use (TOFU) integrity for the runtime-downloaded ONNX
//! runtime DLL and Florence-2 model files.
//!
//! These artifacts are fetched over the network and then loaded/parsed as
//! native code or model graphs. We can't ship a trusted hash for them here
//! (they're large and version-dependent), so instead we record the SHA-256
//! of each file the first time it is seen and re-verify it on every
//! subsequent load. A mismatch (post-install tampering, a partial/corrupt
//! file, or a silently re-pulled upstream blob) fails the load closed.
//!
//! To upgrade to fully-pinned hashes later, pre-populate `integrity.json`
//! with values from a trusted source.

use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{AppError, Result};

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn load_manifest(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn save_manifest(path: &Path, map: &Map<String, Value>) -> Result<()> {
    let json = serde_json::to_string_pretty(&Value::Object(map.clone()))?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Verify `file` against the hash recorded for `key` in the manifest, or
/// record it if this is the first time we've seen `key`. Fails closed on a
/// mismatch.
pub fn verify_or_record(manifest_path: &Path, key: &str, file: &Path) -> Result<()> {
    let actual = sha256_file(file)?;
    let mut map = load_manifest(manifest_path);
    match map.get(key).and_then(|v| v.as_str()) {
        Some(expected) => {
            if expected != actual {
                return Err(AppError::other(format!(
                    "integrity check failed for '{key}': the on-disk file does not match the \
                     hash recorded on first download. Delete the models cache and reload to \
                     re-fetch from source."
                )));
            }
        }
        None => {
            map.insert(key.to_string(), Value::String(actual));
            save_manifest(manifest_path, &map)?;
            tracing::info!("integrity: recorded hash for '{key}'");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::hex_encode;
    use sha2::{Digest, Sha256};

    #[test]
    fn sha256_abc_vector() {
        // Known SHA-256("abc") test vector.
        let digest = Sha256::digest(b"abc");
        assert_eq!(
            hex_encode(&digest),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
