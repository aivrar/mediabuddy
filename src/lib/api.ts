import { invoke } from "@tauri-apps/api/core";

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
  vision_cpu_instances: number;
  vision_max_per_gpu: number;
  vision_max_total: number;
  vision_reserved_vram: number;
  api_host: string;
  api_port: number;
  api_auto_start: boolean;
  api_cors_enabled: boolean;
};

export type SourcePages = Partial<Record<"pixabay" | "pexels" | "unsplash", number>>;

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

export const api = {
  listImages: () => invoke<Image[]>("list_images"),
  deleteImages: (ids: string[]) => invoke<DeleteResult>("delete_images", { ids }),
  isUrlSaved: (url: string) => invoke<boolean>("is_url_saved", { url }),
  searchImages: (
    query: string,
    sources: SourcePages,
    kind: SearchKindParam = "photo"
  ) => invoke<SearchResult[]>("search_images", { query, sources, kind }),
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
  getSystemStats: () => invoke<SystemStats>("get_system_stats"),
  apiStatus: () => invoke<ApiServerStatus>("api_status"),
  apiStart: () => invoke<ApiServerStatus>("api_start"),
  apiStop: () => invoke<ApiServerStatus>("api_stop"),
  getLogs: (since?: number, level?: string) =>
    invoke<LogEntry[]>("get_logs", { query: { since, level } }),
  clearLogs: () => invoke<void>("clear_logs"),
  readThumbBytes: (id: string) => invoke<number[]>("read_thumb_bytes", { id }),
  getDataRoot: () => invoke<string>("get_data_root"),
};
