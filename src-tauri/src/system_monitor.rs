use std::sync::Mutex;

use serde::Serialize;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use crate::vision::detect_gpu_adapters;

#[derive(Debug, Clone, Serialize)]
pub struct SystemStats {
    pub cpu_percent: f32,
    pub ram_percent: f32,
    pub ram_used_gb: f64,
    pub ram_total_gb: f64,
    pub gpus: Vec<GpuStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuStats {
    pub index: u32,
    pub name: String,
    pub util_percent: u32,
    pub vram_used_gb: f64,
    pub vram_total_gb: f64,
    pub vram_percent: f32,
    pub temp_c: Option<u32>,
}

pub struct SystemMonitor {
    sys: Mutex<System>,
    nvml: Option<nvml_wrapper::Nvml>,
}

impl SystemMonitor {
    pub fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::new().with_cpu_usage())
                .with_memory(MemoryRefreshKind::new().with_ram()),
        );
        let nvml = match nvml_wrapper::Nvml::init() {
            Ok(n) => Some(n),
            Err(err) => {
                tracing::info!("NVML unavailable (no NVIDIA GPU monitoring): {err}");
                None
            }
        };
        Self {
            sys: Mutex::new(sys),
            nvml,
        }
    }

    pub fn snapshot(&self) -> SystemStats {
        let (cpu_percent, ram_percent, ram_used_gb, ram_total_gb) = {
            let mut sys = self.sys.lock().unwrap();
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            let cpu = sys.global_cpu_usage();
            let total = sys.total_memory();
            let used = sys.used_memory();
            let pct = if total > 0 {
                (used as f64 / total as f64 * 100.0) as f32
            } else {
                0.0
            };
            let to_gb = |bytes: u64| (bytes as f64) / (1024.0 * 1024.0 * 1024.0);
            (cpu, pct, to_gb(used), to_gb(total))
        };

        let mut gpus = self.nvml.as_ref().map(read_gpus).unwrap_or_default();
        if gpus.is_empty() {
            gpus = read_dxgi_gpus();
        }

        SystemStats {
            cpu_percent,
            ram_percent,
            ram_used_gb,
            ram_total_gb,
            gpus,
        }
    }
}

fn read_dxgi_gpus() -> Vec<GpuStats> {
    detect_gpu_adapters()
        .into_iter()
        .map(|adapter| GpuStats {
            index: adapter.dml_device_id,
            name: adapter.name,
            util_percent: 0,
            vram_used_gb: 0.0,
            vram_total_gb: adapter.dedicated_vram_gb.max(adapter.shared_system_gb),
            vram_percent: 0.0,
            temp_c: None,
        })
        .collect()
}

fn read_gpus(nvml: &nvml_wrapper::Nvml) -> Vec<GpuStats> {
    let count = match nvml.device_count() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let device = match nvml.device_by_index(i) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let name = device.name().unwrap_or_else(|_| format!("GPU {i}"));
        let util = device.utilization_rates().map(|u| u.gpu).unwrap_or(0);
        let mem = device.memory_info().ok();
        let (used_gb, total_gb, vram_pct) = match mem {
            Some(m) => {
                let to_gb = |bytes: u64| (bytes as f64) / (1024.0 * 1024.0 * 1024.0);
                let pct = if m.total > 0 {
                    (m.used as f64 / m.total as f64 * 100.0) as f32
                } else {
                    0.0
                };
                (to_gb(m.used), to_gb(m.total), pct)
            }
            None => (0.0, 0.0, 0.0),
        };
        let temp = device
            .temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu)
            .ok();
        out.push(GpuStats {
            index: i,
            name,
            util_percent: util,
            vram_used_gb: used_gb,
            vram_total_gb: total_gb,
            vram_percent: vram_pct,
            temp_c: temp,
        });
    }
    out
}
