use serde::{Deserialize, Serialize};

const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;
const CUDA_REQUIRED_DLLS: &[&str] = &["cudart64_12.dll", "cublas64_12.dll", "cublasLt64_12.dll"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpuAdapter {
    pub dml_device_id: u32,
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_vram_gb: f64,
    pub dedicated_system_gb: f64,
    pub shared_system_gb: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct HostResources {
    pub logical_cpus: usize,
    pub ram_total_gb: f64,
}

#[derive(Debug, Clone)]
pub struct CudaDependencyStatus {
    pub ready: bool,
    pub missing: Vec<String>,
}

pub fn host_resources() -> HostResources {
    let logical_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    HostResources {
        logical_cpus,
        ram_total_gb: sys.total_memory() as f64 / BYTES_PER_GB,
    }
}

pub fn cuda_dependency_status() -> CudaDependencyStatus {
    #[cfg(windows)]
    {
        let mut missing = Vec::new();
        for dll in CUDA_REQUIRED_DLLS {
            if find_dll_on_path(dll).is_none() {
                missing.push((*dll).to_string());
            }
        }

        CudaDependencyStatus {
            ready: missing.is_empty(),
            missing,
        }
    }
    #[cfg(not(windows))]
    {
        CudaDependencyStatus {
            ready: false,
            missing: CUDA_REQUIRED_DLLS
                .iter()
                .map(|dll| (*dll).to_string())
                .collect(),
        }
    }
}

#[cfg(windows)]
fn find_dll_on_path(dll: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(dll);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn detect_gpu_adapters() -> Vec<GpuAdapter> {
    #[cfg(windows)]
    {
        detect_dxgi_adapters()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
fn detect_dxgi_adapters() -> Vec<GpuAdapter> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND,
    };

    let factory = match unsafe { CreateDXGIFactory1::<IDXGIFactory1>() } {
        Ok(factory) => factory,
        Err(err) => {
            tracing::warn!("dxgi: unable to enumerate GPU adapters: {err}");
            return Vec::new();
        }
    };

    let mut adapters = Vec::new();
    let mut idx = 0u32;
    loop {
        let device_idx = idx;
        let adapter = match unsafe { factory.EnumAdapters1(idx) } {
            Ok(adapter) => adapter,
            Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(err) => {
                tracing::warn!("dxgi: adapter enumeration failed at index {idx}: {err}");
                break;
            }
        };
        idx += 1;

        let desc = match unsafe { adapter.GetDesc1() } {
            Ok(desc) => desc,
            Err(err) => {
                tracing::warn!("dxgi: unable to read adapter description: {err}");
                continue;
            }
        };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }

        adapters.push(GpuAdapter {
            dml_device_id: device_idx,
            name: utf16_name(&desc.Description),
            vendor_id: desc.VendorId,
            device_id: desc.DeviceId,
            dedicated_vram_gb: desc.DedicatedVideoMemory as f64 / BYTES_PER_GB,
            dedicated_system_gb: desc.DedicatedSystemMemory as f64 / BYTES_PER_GB,
            shared_system_gb: desc.SharedSystemMemory as f64 / BYTES_PER_GB,
        });
    }
    adapters
}

#[cfg(windows)]
fn utf16_name(raw: &[u16]) -> String {
    let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    let name = String::from_utf16_lossy(&raw[..len]).trim().to_string();
    if name.is_empty() {
        "Unknown GPU".to_string()
    } else {
        name
    }
}
