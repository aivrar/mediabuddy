import { invoke as tauriInvoke } from "@tauri-apps/api/core";

const NATIVE_BACKEND_UNAVAILABLE =
  "Native backend unavailable. Open Media Buddy through the desktop app instead of the Vite browser preview.";

function hasTauriInvoke() {
  return (
    typeof window !== "undefined" &&
    typeof (
      window as unknown as {
        __TAURI_INTERNALS__?: { invoke?: unknown };
      }
    ).__TAURI_INTERNALS__?.invoke === "function"
  );
}

function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!hasTauriInvoke()) {
    return Promise.reject(new Error(NATIVE_BACKEND_UNAVAILABLE));
  }
  return tauriInvoke<T>(cmd, args);
}

export type MediaKind = "photo" | "video" | "illustration" | "vector";

export type Image = {
  id: string;
  source: string;
  source_id: string;
  kind: MediaKind | string;
  source_page_url: string;
  filename: string;
  path: string;
  thumb_path: string;
  url: string;
  urls: Record<string, string>;
  width: number;
  height: number;
  duration_secs: number | null;
  file_size: number | null;
  query: string;
  alt: string;
  tags: string[];
  color: string | null;
  blur_hash: string | null;
  author_name: string;
  author_url: string;
  author_avatar: string;
  views: number | null;
  downloads: number | null;
  likes: number | null;
  comments: number | null;
  preview_only: boolean;
  vision_processed: boolean;
  ai_generated: boolean | null;
  created_at_provider: string | null;
  downloaded_at: string;
  source_data?: unknown;
};

export type SearchResult = {
  source: string;
  source_id: string;
  kind: MediaKind | string;
  source_page_url: string;
  url: string;
  urls: Record<string, string>;
  query: string;
  tags: string[];
  alt: string;
  width: number | null;
  height: number | null;
  duration_secs: number | null;
  file_size: number | null;
  color: string | null;
  blur_hash: string | null;
  author_name: string;
  author_url: string;
  author_avatar: string;
  views: number | null;
  downloads: number | null;
  likes: number | null;
  comments: number | null;
  ai_generated: boolean | null;
  created_at_provider: string | null;
  source_data?: unknown;
};

export type SearchKindParam = "photo" | "video" | "both";

export type DeleteResult = { deleted: number; failed: number };

export type Settings = {
  pixabay_key: string;
  pexels_key: string;
  unsplash_key: string;
  theme: string;
  vision_auto_load: boolean;
  vision_auto_unload: boolean;
  vision_allow_cpu: boolean;
  vision_execution_mode: string;
  vision_cpu_instances: number;
  vision_cpu_threads_per_instance: number;
  vision_max_per_gpu: number;
  vision_max_total: number;
  vision_reserved_vram: number;
  api_host: string;
  api_port: number;
  api_auto_start: boolean;
  api_cors_enabled: boolean;
  unsplash_detail_threshold: number;
  api_token: string;
};

export type VisionStatus = {
  loaded: boolean;
  instances: number;
  precision: string | null;
  model_dir: string | null;
  runtime: string | null;
  mode: string | null;
  devices: VisionDeviceStatus[];
  workers: VisionInstanceStatus[];
  warnings: string[];
};

export type VisionLoadParams = {
  precision?: "fp32" | "fp16" | "int8" | "q4f16";
  mode?: string;
  count?: number;
  cpuInstances?: number;
  gpuInstancesPerGpu?: number;
  maxTotalInstances?: number;
  reservedVramGb?: number;
  allowCpuFallback?: boolean;
  cpuThreadsPerInstance?: number;
};

export type VisionDeviceStatus = {
  provider: string;
  device_id: number;
  name: string;
  dedicated_vram_gb: number;
  shared_system_gb: number;
  selected_instances: number;
};

export type VisionInstanceStatus = {
  index: number;
  provider: string;
  device_id: number | null;
  device_name: string | null;
  precision: string;
  intra_threads: number;
};

export type DetectedObject = {
  label: string;
  bbox: [number, number, number, number];
};

export type VisionAnalyzeItem = {
  id: string;
  ok: boolean;
  skipped: boolean;
  error: string | null;
  caption: string | null;
  caption_written: boolean;
  tags_added: string[];
  objects: DetectedObject[];
  image: Image | null;
};

export type VisionAnalyzeSummary = {
  total: number;
  analyzed: number;
  skipped: number;
  failed: number;
  results: VisionAnalyzeItem[];
};

export type SourcePages = Partial<Record<"pixabay" | "pexels" | "unsplash", number>>;

export type SearchFilters = {
  orientation?: string;
  color?: string;
  min_width?: number;
  min_height?: number;
  category?: string;
  order?: string;
  image_type?: string;
  video_type?: string;
  size?: string;
  safesearch?: boolean;
  editors_choice?: boolean;
  exclude_ai?: boolean;
  count_per_source?: number;
};

export type GpuStats = {
  index: number;
  name: string;
  util_percent: number;
  vram_used_gb: number;
  vram_total_gb: number;
  vram_percent: number;
  temp_c: number | null;
};

export type SystemStats = {
  cpu_percent: number;
  ram_percent: number;
  ram_used_gb: number;
  ram_total_gb: number;
  gpus: GpuStats[];
};

export type ApiServerStatus = {
  running: boolean;
  host: string | null;
  port: number | null;
  uptime_seconds: number | null;
};

export type LogEntry = {
  timestamp: number;
  level: string;
  target: string;
  message: string;
};

export type ApiProvider = "pixabay" | "pexels" | "unsplash";

export type KeyProbe = {
  provider: string;
  valid: boolean;
  status_code: number | null;
  message: string;
  rate_limit: number | null;
  rate_remaining: number | null;
  reset_seconds: number | null;
};

export type QuotaSlot = {
  remaining: number | null;
  limit: number | null;
  reset_epoch: number | null;
  last_status: number | null;
  last_seen: number | null;
  total_calls: number;
};

export type QuotaSnapshot = {
  pixabay: QuotaSlot;
  pexels: QuotaSlot;
  unsplash: QuotaSlot;
};

export type Topic = {
  id: string;
  name: string | null;
  query: string;
  filters: SearchFilters;
  kind: string;
  enabled_sources: string[];
  created_at: string;
  last_fetched_at: string | null;
};

export type TopicCursor = {
  source: string;
  media_kind: string;
  next_page: number;
  total_seen: number;
  last_status: string;
  last_fetched_at: string | null;
};

export type TopicStatus = {
  topic: Topic;
  cursors: TopicCursor[];
  seen_count: number;
  saved_count: number;
};

export type TopicSummary = {
  id: string;
  name: string | null;
  query: string;
  kind: string;
  enabled_sources: string[];
  created_at: string;
  last_fetched_at: string | null;
  seen_count: number;
  saved_count: number;
};

export type TopicProgress = {
  source: string;
  media_kind: string;
  page_fetched: number;
  raw_count: number;
  kept_count: number;
  status: string;
};

export type TopicGetMoreResult = {
  topic_id: string;
  results: SearchResult[];
  progress: TopicProgress[];
};

export const api = {
  listImages: () => invoke<Image[]>("list_images"),
  deleteImages: (ids: string[]) => invoke<DeleteResult>("delete_images", { ids }),
  isUrlSaved: (url: string) => invoke<boolean>("is_url_saved", { url }),
  updateImage: (id: string, patch: { alt?: string; tags?: string[] }) =>
    invoke<boolean>("update_image", { id, alt: patch.alt, tags: patch.tags }),
  // searchImages: legacy single-shot search; superseded by Topics
  // (topicFindOrCreate + topicGetMore). The underlying Tauri command is
  // still registered for backwards-compat with external automation scripts.
  downloadImages: (
    results: SearchResult[],
    options?: { previewOnly?: boolean; concurrency?: number }
  ) =>
    invoke<Image[]>("download_images", {
      results,
      previewOnly: options?.previewOnly ?? false,
      concurrency: options?.concurrency ?? 8,
    }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  validateApiKey: (provider: ApiProvider, key: string) =>
    invoke<KeyProbe>("validate_api_key", { provider, key }),
  getQuotaStatus: () => invoke<QuotaSnapshot>("get_quota_status"),
  topicFindOrCreate: (params: {
    query: string;
    filters?: SearchFilters;
    kind?: SearchKindParam;
    sources?: string[];
  }) => invoke<Topic>("topic_find_or_create", { params }),
  topicStatus: (topicId: string) =>
    invoke<TopicStatus | null>("topic_status", { topicId }),
  topicList: () => invoke<TopicSummary[]>("topic_list"),
  topicGetMore: (topicId: string, countPerSource?: number) =>
    invoke<TopicGetMoreResult>("topic_get_more", {
      topicId,
      countPerSource,
    }),
  topicReset: (topicId: string) => invoke<void>("topic_reset", { topicId }),
  topicDelete: (topicId: string) => invoke<void>("topic_delete", { topicId }),
  topicImageIds: (topicId: string) =>
    invoke<string[]>("topic_image_ids", { topicId }),
  topicRename: (topicId: string, name: string | null) =>
    invoke<void>("topic_rename", { topicId, name }),
  getSystemStats: () => invoke<SystemStats>("get_system_stats"),
  apiStatus: () => invoke<ApiServerStatus>("api_status"),
  apiStart: () => invoke<ApiServerStatus>("api_start"),
  apiStop: () => invoke<ApiServerStatus>("api_stop"),
  shutdownApp: () => invoke<void>("shutdown_app"),
  getLogs: (since?: number, level?: string) =>
    invoke<LogEntry[]>("get_logs", { query: { since, level } }),
  clearLogs: () => invoke<void>("clear_logs"),
  readThumbBytes: (id: string) => invoke<number[]>("read_thumb_bytes", { id }),
  readImageBytes: (id: string) => invoke<number[]>("read_image_bytes", { id }),
  getDataRoot: () => invoke<string>("get_data_root"),
  visionStatus: () => invoke<VisionStatus>("vision_status"),
  visionLoad: (params?: VisionLoadParams) =>
    invoke<VisionStatus>("vision_load", {
      params: {
        precision: params?.precision ?? "fp32",
        mode: params?.mode,
        count: params?.count,
        cpu_instances: params?.cpuInstances,
        gpu_instances_per_gpu: params?.gpuInstancesPerGpu,
        max_total_instances: params?.maxTotalInstances,
        reserved_vram_gb: params?.reservedVramGb,
        allow_cpu_fallback: params?.allowCpuFallback,
        cpu_threads_per_instance: params?.cpuThreadsPerInstance,
      },
    }),
  visionUnload: () => invoke<VisionStatus>("vision_unload"),
  visionAnalyzeImages: (
    ids: string[],
    options?: {
      detectObjects?: boolean;
      overwriteCaption?: boolean;
      captionMode?: "missing" | "short" | "overwrite" | "skip";
      captionTask?: "caption" | "detailed" | "more_detailed";
      captionMinChars?: number;
      loadIfNeeded?: boolean;
      concurrency?: number;
    }
  ) =>
    invoke<VisionAnalyzeSummary>("vision_analyze_images", {
      params: {
        ids,
        detect_objects: options?.detectObjects ?? true,
        overwrite_caption: options?.overwriteCaption ?? false,
        caption_mode: options?.captionMode,
        caption_task: options?.captionTask,
        caption_min_chars: options?.captionMinChars,
        load_if_needed: options?.loadIfNeeded ?? true,
        concurrency: options?.concurrency,
      },
    }),
};
