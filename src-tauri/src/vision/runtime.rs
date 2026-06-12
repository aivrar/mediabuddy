//! Auto-download ONNX Runtime native DLLs for `ort`'s load-dynamic mode.
//!
//! Different accelerator providers need different ORT builds, and ORT can
//! only be initialized once per process. The first vision load therefore
//! commits the process to one runtime flavor until the app restarts.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{AppError, Result};

const ORT_VERSION: &str = "1.22.0";

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const ORT_CPU_ZIP_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-win-x64-1.22.0.zip";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const ORT_DML_NUGET_URL: &str =
    "https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime.DirectML/1.22.0";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const ORT_CUDA_NUGET_URL: &str =
    "https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime.Gpu.Windows/1.22.0";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const NVIDIA_CUDNN_CU12_PACKAGE: &str = "nvidia-cudnn-cu12";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const NVIDIA_CUDNN_CU12_VERSION: &str = "9.8.0.87";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const ORT_DLL_NAME: &str = "onnxruntime.dll";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeFlavor {
    Cpu,
    DirectMl,
    Cuda,
}

impl RuntimeFlavor {
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimeFlavor::Cpu => "cpu",
            RuntimeFlavor::DirectMl => "directml",
            RuntimeFlavor::Cuda => "cuda",
        }
    }
}

pub struct RuntimeInstall {
    pub flavor: RuntimeFlavor,
    pub dll_path: PathBuf,
    pub runtime_dir: PathBuf,
}

struct RuntimeEntry {
    inner_path: &'static str,
    file_name: &'static str,
}

/// Ensure `onnxruntime.dll` and provider DLLs exist under
/// `cache_dir/runtime/<flavor>/` and return the ORT DLL path. Idempotent.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub fn ensure_onnxruntime_runtime(
    cache_dir: &Path,
    flavor: RuntimeFlavor,
) -> Result<RuntimeInstall> {
    let runtime_dir = cache_dir
        .join("runtime")
        .join(flavor.as_str())
        .join(ORT_VERSION);
    std::fs::create_dir_all(&runtime_dir)?;
    let dll_path = runtime_dir.join(ORT_DLL_NAME);

    let (url, entries): (&str, &[RuntimeEntry]) = match flavor {
        RuntimeFlavor::Cpu => (
            ORT_CPU_ZIP_URL,
            &[
                RuntimeEntry {
                    inner_path: "onnxruntime-win-x64-1.22.0/lib/onnxruntime.dll",
                    file_name: "onnxruntime.dll",
                },
                RuntimeEntry {
                    inner_path: "onnxruntime-win-x64-1.22.0/lib/onnxruntime_providers_shared.dll",
                    file_name: "onnxruntime_providers_shared.dll",
                },
            ],
        ),
        RuntimeFlavor::DirectMl => (
            ORT_DML_NUGET_URL,
            &[
                RuntimeEntry {
                    inner_path: "runtimes/win-x64/native/onnxruntime.dll",
                    file_name: "onnxruntime.dll",
                },
                RuntimeEntry {
                    inner_path: "runtimes/win-x64/native/onnxruntime_providers_shared.dll",
                    file_name: "onnxruntime_providers_shared.dll",
                },
            ],
        ),
        RuntimeFlavor::Cuda => (
            ORT_CUDA_NUGET_URL,
            &[
                RuntimeEntry {
                    inner_path: "runtimes/win-x64/native/onnxruntime.dll",
                    file_name: "onnxruntime.dll",
                },
                RuntimeEntry {
                    inner_path: "runtimes/win-x64/native/onnxruntime_providers_shared.dll",
                    file_name: "onnxruntime_providers_shared.dll",
                },
                RuntimeEntry {
                    inner_path: "runtimes/win-x64/native/onnxruntime_providers_cuda.dll",
                    file_name: "onnxruntime_providers_cuda.dll",
                },
            ],
        ),
    };

    let needs_extract = entries
        .iter()
        .any(|entry| !runtime_dir.join(entry.file_name).exists());
    if needs_extract {
        tracing::info!(
            "fetching ONNX Runtime {ORT_VERSION} {} runtime -> {}",
            flavor.as_str(),
            runtime_dir.display()
        );
        let bytes = http_get_blocking(url)?;
        for entry in entries {
            let out = runtime_dir.join(entry.file_name);
            if !out.exists() {
                extract_one(&bytes, entry.inner_path, &out)?;
            }
        }
    }

    if flavor == RuntimeFlavor::Cuda {
        ensure_cuda_cudnn(&runtime_dir, cache_dir)?;
    }

    let manifest = cache_dir.join("integrity.json");
    for entry in entries {
        let out = runtime_dir.join(entry.file_name);
        super::integrity::verify_or_record(
            &manifest,
            &format!(
                "runtime/{}/{ORT_VERSION}/{}",
                flavor.as_str(),
                entry.file_name
            ),
            &out,
        )?;
    }

    Ok(RuntimeInstall {
        flavor,
        dll_path,
        runtime_dir,
    })
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn ensure_cuda_cudnn(runtime_dir: &Path, cache_dir: &Path) -> Result<()> {
    const CUDNN_ENTRIES: &[RuntimeEntry] = &[
        RuntimeEntry {
            inner_path: "nvidia/cudnn/bin/cudnn64_9.dll",
            file_name: "cudnn64_9.dll",
        },
        RuntimeEntry {
            inner_path: "nvidia/cudnn/bin/cudnn_adv64_9.dll",
            file_name: "cudnn_adv64_9.dll",
        },
        RuntimeEntry {
            inner_path: "nvidia/cudnn/bin/cudnn_cnn64_9.dll",
            file_name: "cudnn_cnn64_9.dll",
        },
        RuntimeEntry {
            inner_path: "nvidia/cudnn/bin/cudnn_ops64_9.dll",
            file_name: "cudnn_ops64_9.dll",
        },
        RuntimeEntry {
            inner_path: "nvidia/cudnn/bin/cudnn_graph64_9.dll",
            file_name: "cudnn_graph64_9.dll",
        },
        RuntimeEntry {
            inner_path: "nvidia/cudnn/bin/cudnn_heuristic64_9.dll",
            file_name: "cudnn_heuristic64_9.dll",
        },
        RuntimeEntry {
            inner_path: "nvidia/cudnn/bin/cudnn_engines_precompiled64_9.dll",
            file_name: "cudnn_engines_precompiled64_9.dll",
        },
        RuntimeEntry {
            inner_path: "nvidia/cudnn/bin/cudnn_engines_runtime_compiled64_9.dll",
            file_name: "cudnn_engines_runtime_compiled64_9.dll",
        },
    ];

    let needs_extract = CUDNN_ENTRIES
        .iter()
        .any(|entry| !runtime_dir.join(entry.file_name).exists());
    if needs_extract {
        tracing::info!(
            "fetching NVIDIA cuDNN {} CUDA 12 runtime -> {}",
            NVIDIA_CUDNN_CU12_VERSION,
            runtime_dir.display()
        );
        let wheel_url = pypi_wheel_url(
            NVIDIA_CUDNN_CU12_PACKAGE,
            NVIDIA_CUDNN_CU12_VERSION,
            "win_amd64.whl",
        )?;
        let bytes = http_get_blocking(&wheel_url)?;
        for entry in CUDNN_ENTRIES {
            let out = runtime_dir.join(entry.file_name);
            if !out.exists() {
                extract_one(&bytes, entry.inner_path, &out)?;
            }
        }
    }

    let manifest = cache_dir.join("integrity.json");
    for entry in CUDNN_ENTRIES {
        let out = runtime_dir.join(entry.file_name);
        super::integrity::verify_or_record(
            &manifest,
            &format!("runtime/cuda/{ORT_VERSION}/{}", entry.file_name),
            &out,
        )?;
    }
    Ok(())
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn pypi_wheel_url(package: &str, version: &str, filename_suffix: &str) -> Result<String> {
    let metadata_url = format!("https://pypi.org/pypi/{package}/{version}/json");
    let bytes = http_get_blocking(&metadata_url)?;
    let metadata: Value = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::other(format!("pypi metadata parse: {e}")))?;
    let urls = metadata
        .get("urls")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::other("pypi metadata missing urls"))?;

    for item in urls {
        let filename = item.get("filename").and_then(Value::as_str).unwrap_or("");
        let package_type = item
            .get("packagetype")
            .and_then(Value::as_str)
            .unwrap_or("");
        let url = item.get("url").and_then(Value::as_str).unwrap_or("");
        if package_type == "bdist_wheel" && filename.ends_with(filename_suffix) && !url.is_empty() {
            return Ok(url.to_string());
        }
    }

    Err(AppError::other(format!(
        "could not find {package} {version} wheel ending with {filename_suffix}"
    )))
}

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
pub fn ensure_onnxruntime_runtime(
    _cache_dir: &Path,
    _flavor: RuntimeFlavor,
) -> Result<RuntimeInstall> {
    Err(AppError::other(
        "Auto-download of onnxruntime is only wired up for Windows x64 in this build. \
         Install onnxruntime manually and set ORT_DYLIB_PATH.",
    ))
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub fn add_runtime_dll_directory(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::System::LibraryLoader::SetDllDirectoryW;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe { SetDllDirectoryW(PCWSTR(wide.as_ptr())) }
        .map_err(|e| AppError::other(format!("set DLL search directory: {e}")))
}

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
pub fn add_runtime_dll_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn http_get_blocking(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .call()
        .map_err(|e| AppError::other(format!("ort runtime download: {e}")))?;
    let mut buf = Vec::with_capacity(32 * 1024 * 1024);
    resp.into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| AppError::other(format!("ort runtime read: {e}")))?;
    Ok(buf)
}

fn extract_one(zip_bytes: &[u8], inner: &str, out_path: &Path) -> Result<()> {
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| AppError::other(format!("ort zip: {e}")))?;
    let mut file = archive
        .by_name(inner)
        .map_err(|e| AppError::other(format!("entry {inner} not found: {e}")))?;

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = out_path.with_extension("tmp");
    let mut out = std::fs::File::create(&tmp_path)?;
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
    out.flush()?;
    drop(out);
    std::fs::rename(&tmp_path, out_path)?;
    Ok(())
}
