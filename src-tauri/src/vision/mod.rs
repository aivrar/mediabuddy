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
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

mod download;
mod hardware;
mod inference;
mod integrity;
mod preprocess;
mod runtime;

/// The process-wide ONNX Runtime flavor after `ort::init_from(...)` succeeds.
/// Failed init attempts are not sticky, so CPU fallback can still recover.
static ORT_INIT_STATE: OnceLock<Mutex<Option<runtime::RuntimeFlavor>>> = OnceLock::new();

pub(crate) use hardware::detect_gpu_adapters;
use inference::VisionEngine;
pub use runtime::RuntimeFlavor;

const EST_GPU_VRAM_PER_INSTANCE_GB: f64 = 2.0;
const EST_CPU_RAM_PER_INSTANCE_GB: f64 = 3.0;
const CPU_RAM_RESERVE_GB: f64 = 2.0;
const MAX_CPU_INSTANCES: usize = 16;
const MAX_GPU_INSTANCES_PER_GPU: usize = 16;
const MAX_TOTAL_INSTANCES: usize = 32;

fn recover_lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionWriteMode {
    MissingOnly,
    ReplaceShort,
    Overwrite,
    Skip,
}

impl CaptionWriteMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingOnly => "missing",
            Self::ReplaceShort => "short",
            Self::Overwrite => "overwrite",
            Self::Skip => "skip",
        }
    }
}

pub fn parse_caption_write_mode(
    mode: Option<&str>,
    overwrite_legacy: Option<bool>,
) -> CaptionWriteMode {
    match mode.map(|s| s.trim().to_ascii_lowercase()) {
        Some(s) if matches!(s.as_str(), "skip" | "none" | "tags_only" | "tags-only") => {
            CaptionWriteMode::Skip
        }
        Some(s) if matches!(s.as_str(), "overwrite" | "replace" | "always") => {
            CaptionWriteMode::Overwrite
        }
        Some(s) if matches!(s.as_str(), "short" | "replace_short" | "replace-short") => {
            CaptionWriteMode::ReplaceShort
        }
        Some(s)
            if matches!(
                s.as_str(),
                "missing" | "missing_only" | "missing-only" | "fill_missing" | "fill-missing"
            ) =>
        {
            CaptionWriteMode::MissingOnly
        }
        _ if overwrite_legacy.unwrap_or(false) => CaptionWriteMode::Overwrite,
        _ => CaptionWriteMode::MissingOnly,
    }
}

pub fn caption_task_from_name(task: Option<&str>) -> &'static str {
    match task.map(|s| s.trim().to_ascii_lowercase()) {
        Some(s) if matches!(s.as_str(), "caption" | "short") => TASK_CAPTION,
        Some(s) if matches!(s.as_str(), "more" | "more_detailed" | "more-detailed") => {
            TASK_MORE_DETAILED_CAPTION
        }
        _ => TASK_DETAILED_CAPTION,
    }
}

pub fn should_write_caption(
    mode: CaptionWriteMode,
    existing: &str,
    generated: &str,
    min_chars: usize,
) -> bool {
    if generated.trim().is_empty() {
        return false;
    }
    match mode {
        CaptionWriteMode::MissingOnly => existing.trim().is_empty(),
        CaptionWriteMode::ReplaceShort => existing.trim().chars().count() < min_chars,
        CaptionWriteMode::Overwrite => true,
        CaptionWriteMode::Skip => false,
    }
}

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VisionExecutionMode {
    Auto,
    Cpu,
    DirectMl,
    Cuda,
}

impl VisionExecutionMode {
    pub fn parse(s: Option<&str>) -> Self {
        match s.map(|s| s.to_ascii_lowercase()) {
            Some(s) if s == "cpu" => Self::Cpu,
            Some(s) if s == "directml" || s == "dml" => Self::DirectMl,
            Some(s) if s == "cuda" => Self::Cuda,
            _ => Self::Auto,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "cpu",
            Self::DirectMl => "directml",
            Self::Cuda => "cuda",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VisionLoadOptions {
    pub precision: Precision,
    pub mode: VisionExecutionMode,
    pub cpu_instances: usize,
    pub gpu_instances_per_gpu: usize,
    pub max_total_instances: usize,
    pub reserved_vram_gb: f64,
    pub allow_cpu_fallback: bool,
    pub cpu_threads_per_instance: Option<usize>,
}

impl VisionLoadOptions {
    pub fn legacy(precision: Precision, count: usize) -> Self {
        Self {
            precision,
            mode: VisionExecutionMode::Cpu,
            cpu_instances: count,
            gpu_instances_per_gpu: 0,
            max_total_instances: count,
            reserved_vram_gb: 0.0,
            allow_cpu_fallback: true,
            cpu_threads_per_instance: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionDeviceStatus {
    pub provider: String,
    pub device_id: u32,
    pub name: String,
    pub dedicated_vram_gb: f64,
    pub shared_system_gb: f64,
    pub selected_instances: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionInstanceStatus {
    pub index: usize,
    pub provider: String,
    pub device_id: Option<u32>,
    pub device_name: Option<String>,
    pub precision: String,
    pub intra_threads: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionStatus {
    pub loaded: bool,
    pub instances: usize,
    pub precision: Option<String>,
    pub model_dir: Option<String>,
    pub runtime: Option<String>,
    pub mode: Option<String>,
    pub devices: Vec<VisionDeviceStatus>,
    pub workers: Vec<VisionInstanceStatus>,
    pub warnings: Vec<String>,
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

#[derive(Debug, Clone)]
pub struct VisionAnalysisOptions {
    pub caption_task: Option<String>,
    pub detect_objects: bool,
}

#[derive(Debug, Clone)]
pub enum ExecutionTarget {
    Cpu,
    DirectMl { device_id: u32, device_name: String },
    Cuda { device_id: u32, device_name: String },
}

impl ExecutionTarget {
    pub fn provider(&self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::DirectMl { .. } => "directml",
            Self::Cuda { .. } => "cuda",
        }
    }

    pub fn device_id(&self) -> Option<u32> {
        match self {
            Self::Cpu => None,
            Self::DirectMl { device_id, .. } | Self::Cuda { device_id, .. } => Some(*device_id),
        }
    }

    pub fn device_name(&self) -> Option<&str> {
        match self {
            Self::Cpu => None,
            Self::DirectMl { device_name, .. } | Self::Cuda { device_name, .. } => {
                Some(device_name)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub index: usize,
    pub target: ExecutionTarget,
    pub intra_threads: usize,
}

pub struct VisionRegistry {
    inner: Mutex<RegistryState>,
    load_lock: Mutex<()>,
    next_idx: AtomicUsize,
}

struct RegistryState {
    instances: Vec<Arc<VisionEngine>>,
    precision: Option<Precision>,
    cache_dir: Option<PathBuf>,
    runtime: Option<RuntimeFlavor>,
    mode: Option<VisionExecutionMode>,
    devices: Vec<VisionDeviceStatus>,
    warnings: Vec<String>,
}

impl VisionRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RegistryState {
                instances: Vec::new(),
                precision: None,
                cache_dir: None,
                runtime: None,
                mode: None,
                devices: Vec::new(),
                warnings: Vec::new(),
            }),
            load_lock: Mutex::new(()),
            next_idx: AtomicUsize::new(0),
        }
    }

    pub fn status(&self) -> VisionStatus {
        let st = recover_lock(&self.inner);
        VisionStatus {
            loaded: !st.instances.is_empty(),
            instances: st.instances.len(),
            precision: st.precision.map(|p| format!("{p:?}").to_lowercase()),
            model_dir: st
                .cache_dir
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            runtime: st.runtime.map(|r| r.as_str().to_string()),
            mode: st.mode.map(|m| m.as_str().to_string()),
            devices: st.devices.clone(),
            workers: st.instances.iter().map(|engine| engine.status()).collect(),
            warnings: st.warnings.clone(),
        }
    }

    pub fn unload_all(&self) {
        let _load_guard = recover_lock(&self.load_lock);
        let mut st = recover_lock(&self.inner);
        st.instances.clear();
        st.precision = None;
        st.cache_dir = None;
        st.runtime = None;
        st.mode = None;
        st.devices.clear();
        st.warnings.clear();
        self.next_idx.store(0, Ordering::Relaxed);
    }

    /// Download (if needed) and load the Florence-2 ONNX engines.
    /// `cache_dir` is the root for HF's cache layout — typically
    /// `<data>/models/`.
    pub fn load(&self, cache_dir: &Path, precision: Precision, count: usize) -> Result<usize> {
        self.load_with_options(cache_dir, VisionLoadOptions::legacy(precision, count))
    }

    pub fn load_with_options(&self, cache_dir: &Path, options: VisionLoadOptions) -> Result<usize> {
        let _load_guard = recover_lock(&self.load_lock);

        if !matches!(options.precision, Precision::Fp32) {
            return Err(AppError::other(format!(
                "Florence-2 precision {:?} is not wired end-to-end yet. Use `fp32`; \
                 fp16/int8/q4 variants need provider-specific tensor handling.",
                options.precision
            )));
        }

        let mut plan = plan_load(&options)?;
        let planned_runtime = plan.runtime;
        let active_runtime = match ensure_ort_initialized(cache_dir, planned_runtime) {
            Ok(runtime) => runtime,
            Err(err) if planned_runtime != RuntimeFlavor::Cpu && options.allow_cpu_fallback => {
                let accelerator_err = err.to_string();
                let (fallback_workers, fallback_warnings) = plan_cpu_fallback_workers(&options);

                plan.runtime = RuntimeFlavor::Cpu;
                plan.workers = fallback_workers;
                plan.devices.clear();
                plan.warnings.push(format!(
                    "{} runtime init failed: {}. Falling back to CPU runtime.",
                    planned_runtime.as_str(),
                    accelerator_err
                ));
                plan.warnings.extend(fallback_warnings);

                ensure_ort_initialized(cache_dir, RuntimeFlavor::Cpu).map_err(|cpu_err| {
                    AppError::other(format!(
                        "{} runtime init failed ({accelerator_err}); CPU fallback init also failed ({cpu_err})",
                        planned_runtime.as_str()
                    ))
                })?
            }
            Err(err) => return Err(err),
        };
        let paths = download::ensure_florence2_models(cache_dir, options.precision)?;

        let worker_runtime = plan.runtime;
        let planned_workers = std::mem::take(&mut plan.workers);
        let instances = match open_instances(&paths, options.precision, planned_workers) {
            Ok(instances) => instances,
            Err(err) if worker_runtime != RuntimeFlavor::Cpu && options.allow_cpu_fallback => {
                let accelerator_err = err.to_string();
                let (fallback_workers, fallback_warnings) = plan_cpu_fallback_workers(&options);

                plan.devices.clear();
                tracing::warn!(
                    "{} worker load failed: {}",
                    worker_runtime.as_str(),
                    accelerator_err
                );
                plan.warnings
                    .push(fallback_warning(worker_runtime, &accelerator_err));
                plan.warnings.extend(fallback_warnings);

                open_instances(&paths, options.precision, fallback_workers).map_err(|cpu_err| {
                    AppError::other(format!(
                        "accelerator worker load failed ({accelerator_err}); CPU fallback also failed ({cpu_err})"
                    ))
                })?
            }
            Err(err) => return Err(err),
        };

        let mut st = recover_lock(&self.inner);
        st.instances = instances;
        st.precision = Some(options.precision);
        st.cache_dir = Some(cache_dir.to_path_buf());
        st.runtime = Some(active_runtime);
        st.mode = Some(options.mode);
        st.devices = plan.devices;
        st.warnings = plan.warnings;
        self.next_idx.store(0, Ordering::Relaxed);
        Ok(st.instances.len())
    }

    pub fn analyse(&self, image_path: &Path, need_objects: bool) -> Result<AnalysisResult> {
        self.analyse_with_options(
            image_path,
            VisionAnalysisOptions {
                caption_task: Some(TASK_DETAILED_CAPTION.to_string()),
                detect_objects: need_objects,
            },
        )
    }

    pub fn analyse_with_options(
        &self,
        image_path: &Path,
        options: VisionAnalysisOptions,
    ) -> Result<AnalysisResult> {
        let instances = {
            let st = recover_lock(&self.inner);
            if st.instances.is_empty() {
                return Err(AppError::other(
                    "Florence-2 model is not loaded. Call vision_load (or POST /api/v1/vision/load) first.",
                ));
            }
            st.instances.clone()
        };

        let start = self.next_idx.fetch_add(1, Ordering::Relaxed) % instances.len();
        let mut last_error = None;
        for offset in 0..instances.len() {
            let engine = instances[(start + offset) % instances.len()].clone();
            match analyze_with_engine(&engine, image_path, &options) {
                Ok(result) => return Ok(result),
                Err(err) => {
                    let status = engine.status();
                    tracing::warn!(
                        "Florence-2 worker #{} ({}) failed analyzing {}: {}",
                        status.index,
                        status.provider,
                        image_path.display(),
                        err
                    );
                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| AppError::other("Florence-2 analysis failed")))
    }
}

fn analyze_with_engine(
    engine: &VisionEngine,
    image_path: &Path,
    options: &VisionAnalysisOptions,
) -> Result<AnalysisResult> {
    let caption = match options.caption_task.as_deref() {
        Some(task) => engine.caption(image_path, task)?,
        None => String::new(),
    };
    let objects = if options.detect_objects {
        engine.detect_objects(image_path, TASK_OBJECT_DETECTION)?
    } else {
        Vec::new()
    };
    Ok(AnalysisResult { caption, objects })
}

fn open_instances(
    paths: &download::ModelPaths,
    precision: Precision,
    workers: Vec<EngineConfig>,
) -> Result<Vec<Arc<VisionEngine>>> {
    let mut instances = Vec::with_capacity(workers.len());
    for config in workers {
        instances.push(Arc::new(VisionEngine::open(paths, precision, config)?));
    }
    Ok(instances)
}

fn runtime_display_name(runtime: RuntimeFlavor) -> &'static str {
    match runtime {
        RuntimeFlavor::Cpu => "CPU",
        RuntimeFlavor::DirectMl => "DirectML",
        RuntimeFlavor::Cuda => "CUDA",
    }
}

fn fallback_warning(failed_runtime: RuntimeFlavor, accelerator_err: &str) -> String {
    let failed_name = runtime_display_name(failed_runtime);
    let reason = if failed_runtime == RuntimeFlavor::DirectMl
        && (accelerator_err.contains("887A0004")
            || accelerator_err.contains("feature level is not supported"))
    {
        "this GPU or driver does not support the DirectML feature level required by ONNX Runtime"
            .to_string()
    } else if failed_runtime == RuntimeFlavor::Cuda {
        "the CUDA provider could not open the Florence-2 model on the selected GPU".to_string()
    } else {
        accelerator_err.to_string()
    };

    format!("{failed_name} worker load failed because {reason}. Loaded CPU worker instead.")
}

fn plan_cpu_fallback_workers(options: &VisionLoadOptions) -> (Vec<EngineConfig>, Vec<String>) {
    let host = hardware::host_resources();
    let mut workers = Vec::new();
    let mut warnings = Vec::new();

    allocate_cpu_workers(
        options,
        &host,
        options.max_total_instances.clamp(1, MAX_TOTAL_INSTANCES),
        &mut workers,
        &mut warnings,
    );

    for (index, worker) in workers.iter_mut().enumerate() {
        worker.index = index;
    }

    (workers, warnings)
}

struct LoadPlan {
    runtime: RuntimeFlavor,
    workers: Vec<EngineConfig>,
    devices: Vec<VisionDeviceStatus>,
    warnings: Vec<String>,
}

fn plan_load(options: &VisionLoadOptions) -> Result<LoadPlan> {
    let max_total = options.max_total_instances.clamp(1, MAX_TOTAL_INSTANCES);
    let host = hardware::host_resources();
    let adapters = hardware::detect_gpu_adapters();
    let cuda_deps = hardware::cuda_dependency_status();
    let mut workers = Vec::new();
    let mut devices = Vec::new();
    let mut warnings = Vec::new();

    match options.mode {
        VisionExecutionMode::Auto => {
            if adapters.iter().any(|adapter| adapter.vendor_id == 0x10DE) {
                if cuda_deps.ready {
                    allocate_cuda_workers(
                        options,
                        &adapters,
                        max_total,
                        &mut workers,
                        &mut devices,
                        &mut warnings,
                    );
                } else {
                    warnings.push(cuda_dependency_warning(&cuda_deps.missing));
                }
            }

            if workers.is_empty() {
                allocate_directml_workers(
                    options,
                    &adapters,
                    max_total,
                    &mut workers,
                    &mut devices,
                    &mut warnings,
                );
            }
        }
        VisionExecutionMode::DirectMl => {
            allocate_directml_workers(
                options,
                &adapters,
                max_total,
                &mut workers,
                &mut devices,
                &mut warnings,
            );
            if options.mode == VisionExecutionMode::DirectMl && workers.is_empty() {
                warnings
                    .push("DirectML was requested, but no DXGI GPU capacity was selected.".into());
            }
        }
        VisionExecutionMode::Cuda => {
            if cuda_deps.ready {
                allocate_cuda_workers(
                    options,
                    &adapters,
                    max_total,
                    &mut workers,
                    &mut devices,
                    &mut warnings,
                );
            } else {
                warnings.push(cuda_dependency_warning(&cuda_deps.missing));
            }
            if workers.is_empty() {
                warnings.push(
                    "CUDA was requested, but no CUDA-capable GPU workers could be selected.".into(),
                );
            }
        }
        VisionExecutionMode::Cpu => {}
    }

    let should_add_cpu =
        should_plan_cpu_workers(options.mode, options.allow_cpu_fallback, workers.len());
    if should_add_cpu && workers.len() < max_total {
        allocate_cpu_workers(options, &host, max_total, &mut workers, &mut warnings);
    }

    if workers.is_empty() {
        let details = if warnings.is_empty() {
            String::new()
        } else {
            format!(" {}", warnings.join(" "))
        };
        return Err(AppError::other(format!(
            "No Florence-2 workers could be planned. Enable CPU fallback, lower reserved VRAM, install missing GPU runtime dependencies, or reduce requested instance limits.{details}",
        )));
    }

    for (index, worker) in workers.iter_mut().enumerate() {
        worker.index = index;
    }

    let runtime = if workers
        .iter()
        .any(|w| matches!(w.target, ExecutionTarget::Cuda { .. }))
    {
        RuntimeFlavor::Cuda
    } else if workers
        .iter()
        .any(|w| matches!(w.target, ExecutionTarget::DirectMl { .. }))
    {
        RuntimeFlavor::DirectMl
    } else {
        RuntimeFlavor::Cpu
    };

    Ok(LoadPlan {
        runtime,
        workers,
        devices,
        warnings,
    })
}

fn cuda_dependency_warning(missing: &[String]) -> String {
    format!(
        "NVIDIA CUDA GPUs were detected, but CUDA execution is not ready because these CUDA 12 runtime DLLs are missing from PATH: {}. Install CUDA 12.x, add its bin folder to PATH, then restart Media Buddy. Media Buddy downloads cuDNN 9 into its own runtime cache.",
        missing.join(", ")
    )
}

fn allocate_directml_workers(
    options: &VisionLoadOptions,
    adapters: &[hardware::GpuAdapter],
    max_total: usize,
    workers: &mut Vec<EngineConfig>,
    devices: &mut Vec<VisionDeviceStatus>,
    warnings: &mut Vec<String>,
) {
    let per_gpu = options.gpu_instances_per_gpu.min(MAX_GPU_INSTANCES_PER_GPU);
    if per_gpu == 0 {
        return;
    }

    for adapter in adapters {
        if workers.len() >= max_total {
            break;
        }
        let remaining = max_total - workers.len();
        let slots =
            gpu_slots(adapter.dedicated_vram_gb, options.reserved_vram_gb, per_gpu).min(remaining);
        if slots == 0 {
            warnings.push(format!(
                "{} skipped: {:.1} GB dedicated VRAM leaves no capacity after {:.1} GB reserve.",
                adapter.name, adapter.dedicated_vram_gb, options.reserved_vram_gb
            ));
        }
        devices.push(VisionDeviceStatus {
            provider: "directml".into(),
            device_id: adapter.dml_device_id,
            name: adapter.name.clone(),
            dedicated_vram_gb: adapter.dedicated_vram_gb,
            shared_system_gb: adapter.shared_system_gb,
            selected_instances: slots,
        });
        for _ in 0..slots {
            workers.push(EngineConfig {
                index: 0,
                target: ExecutionTarget::DirectMl {
                    device_id: adapter.dml_device_id,
                    device_name: adapter.name.clone(),
                },
                intra_threads: 1,
            });
        }
    }
}

fn should_plan_cpu_workers(
    mode: VisionExecutionMode,
    allow_cpu_fallback: bool,
    accelerator_worker_count: usize,
) -> bool {
    match mode {
        VisionExecutionMode::Cpu => true,
        VisionExecutionMode::Auto | VisionExecutionMode::DirectMl | VisionExecutionMode::Cuda => {
            allow_cpu_fallback && accelerator_worker_count == 0
        }
    }
}

fn allocate_cuda_workers(
    options: &VisionLoadOptions,
    adapters: &[hardware::GpuAdapter],
    max_total: usize,
    workers: &mut Vec<EngineConfig>,
    devices: &mut Vec<VisionDeviceStatus>,
    warnings: &mut Vec<String>,
) {
    let per_gpu = options.gpu_instances_per_gpu.min(MAX_GPU_INSTANCES_PER_GPU);
    if per_gpu == 0 {
        return;
    }

    let nvidia = adapters
        .iter()
        .filter(|adapter| adapter.vendor_id == 0x10DE);
    for (cuda_id, adapter) in nvidia.enumerate() {
        if workers.len() >= max_total {
            break;
        }
        let remaining = max_total - workers.len();
        let slots =
            gpu_slots(adapter.dedicated_vram_gb, options.reserved_vram_gb, per_gpu).min(remaining);
        if slots == 0 {
            warnings.push(format!(
                "{} skipped: {:.1} GB dedicated VRAM leaves no CUDA capacity after {:.1} GB reserve.",
                adapter.name, adapter.dedicated_vram_gb, options.reserved_vram_gb
            ));
        }
        devices.push(VisionDeviceStatus {
            provider: "cuda".into(),
            device_id: cuda_id as u32,
            name: adapter.name.clone(),
            dedicated_vram_gb: adapter.dedicated_vram_gb,
            shared_system_gb: adapter.shared_system_gb,
            selected_instances: slots,
        });
        for _ in 0..slots {
            workers.push(EngineConfig {
                index: 0,
                target: ExecutionTarget::Cuda {
                    device_id: cuda_id as u32,
                    device_name: adapter.name.clone(),
                },
                intra_threads: 1,
            });
        }
    }
}

fn allocate_cpu_workers(
    options: &VisionLoadOptions,
    host: &hardware::HostResources,
    max_total: usize,
    workers: &mut Vec<EngineConfig>,
    warnings: &mut Vec<String>,
) {
    let remaining = max_total.saturating_sub(workers.len());
    if remaining == 0 {
        return;
    }

    let requested = options.cpu_instances.clamp(1, MAX_CPU_INSTANCES);
    let by_threads = (host.logical_cpus / 2).max(1);
    let usable_ram = (host.ram_total_gb - CPU_RAM_RESERVE_GB).max(0.0);
    let by_ram = (usable_ram / EST_CPU_RAM_PER_INSTANCE_GB).floor().max(1.0) as usize;
    let count = requested.min(by_threads).min(by_ram).min(remaining).max(1);

    if count < requested {
        warnings.push(format!(
            "CPU workers capped from {requested} to {count} by host resources ({:.1} GB RAM, {} logical CPUs).",
            host.ram_total_gb, host.logical_cpus
        ));
    }

    let threads = options
        .cpu_threads_per_instance
        .unwrap_or_else(|| (host.logical_cpus / count.max(1)).max(1))
        .clamp(1, host.logical_cpus.max(1));
    for _ in 0..count {
        workers.push(EngineConfig {
            index: 0,
            target: ExecutionTarget::Cpu,
            intra_threads: threads,
        });
    }
}

fn gpu_slots(dedicated_vram_gb: f64, reserved_vram_gb: f64, requested: usize) -> usize {
    if requested == 0 {
        return 0;
    }
    if dedicated_vram_gb < 1.0 {
        return 1.min(requested);
    }
    let available = (dedicated_vram_gb - reserved_vram_gb.max(0.0)).max(0.0);
    ((available / EST_GPU_VRAM_PER_INSTANCE_GB).floor() as usize).min(requested)
}

/// Download `onnxruntime.dll` if missing and run `ort::init_from()` once.
/// ONNX Runtime is process-wide, so successful initialization fixes the DLL
/// flavor for this app process. Failed attempts are left retryable.
fn ensure_ort_initialized(cache_dir: &Path, flavor: RuntimeFlavor) -> Result<RuntimeFlavor> {
    let state = ORT_INIT_STATE.get_or_init(|| Mutex::new(None));
    let mut initialized = recover_lock(state);

    if let Some(existing) = *initialized {
        if existing == flavor || flavor == RuntimeFlavor::Cpu {
            return Ok(existing);
        }

        return Err(AppError::other(format!(
            "ONNX Runtime is already initialized with the {} runtime. Restart the app before switching to {}.",
            existing.as_str(),
            flavor.as_str()
        )));
    }

    let install = runtime::ensure_onnxruntime_runtime(cache_dir, flavor)?;
    runtime::add_runtime_dll_directory(&install.runtime_dir)?;

    // Pass the DLL path explicitly to ort rather than mutating the
    // process-wide ORT_DYLIB_PATH env var. `set_var` is unsound in a
    // multi-threaded process (this runs after the server/UI threads are
    // up); `init_from(path)` already points ort at the right runtime.
    match ort::init_from(install.dll_path.to_string_lossy().to_string())
        .with_name("mediabuddy")
        .commit()
    {
        Ok(_) => {
            *initialized = Some(install.flavor);
            Ok(install.flavor)
        }
        Err(e) => Err(AppError::other(format!("ort init failed: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::{should_plan_cpu_workers, VisionExecutionMode};

    #[test]
    fn cpu_mode_always_plans_cpu_workers() {
        assert!(should_plan_cpu_workers(VisionExecutionMode::Cpu, false, 0));
        assert!(should_plan_cpu_workers(VisionExecutionMode::Cpu, true, 4));
    }

    #[test]
    fn fallback_does_not_add_cpu_when_accelerator_workers_exist() {
        for mode in [
            VisionExecutionMode::Auto,
            VisionExecutionMode::Cuda,
            VisionExecutionMode::DirectMl,
        ] {
            assert!(!should_plan_cpu_workers(mode, true, 1));
            assert!(!should_plan_cpu_workers(mode, true, 4));
        }
    }

    #[test]
    fn fallback_can_plan_cpu_when_no_accelerator_workers_exist() {
        for mode in [
            VisionExecutionMode::Auto,
            VisionExecutionMode::Cuda,
            VisionExecutionMode::DirectMl,
        ] {
            assert!(should_plan_cpu_workers(mode, true, 0));
            assert!(!should_plan_cpu_workers(mode, false, 0));
        }
    }
}
