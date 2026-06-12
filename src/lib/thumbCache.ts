import { convertFileSrc } from "@tauri-apps/api/core";

import { api } from "./api";

const cache = new Map<string, string>();
const inflight = new Map<string, Promise<string>>();
const imageCache = new Map<string, string>();
const imageInflight = new Map<string, Promise<string>>();
const mediaFileCache = new Map<string, string>();
const mediaFileInflight = new Map<string, Promise<string>>();

export async function getThumbUrl(id: string): Promise<string> {
  const cached = cache.get(id);
  if (cached) return cached;
  const pending = inflight.get(id);
  if (pending) return pending;
  const promise = (async () => {
    try {
      const bytes = await api.readThumbBytes(id);
      const blob = new Blob([new Uint8Array(bytes)], { type: "image/jpeg" });
      const url = URL.createObjectURL(blob);
      cache.set(id, url);
      return url;
    } finally {
      // Always clear the in-flight entry so a failed load can be retried.
      // (Previously a rejected promise stayed in `inflight` forever, so a
      // thumbnail could never load again after one transient error.)
      inflight.delete(id);
    }
  })();
  inflight.set(id, promise);
  return promise;
}

export async function getImageUrl(id: string): Promise<string> {
  const cached = imageCache.get(id);
  if (cached) return cached;
  const pending = imageInflight.get(id);
  if (pending) return pending;
  const promise = (async () => {
    try {
      const bytes = await api.readImageBytes(id);
      const blob = new Blob([new Uint8Array(bytes)]);
      const url = URL.createObjectURL(blob);
      imageCache.set(id, url);
      return url;
    } finally {
      imageInflight.delete(id);
    }
  })();
  imageInflight.set(id, promise);
  return promise;
}

export async function getMediaFileUrl(id: string): Promise<string> {
  const cached = mediaFileCache.get(id);
  if (cached) return cached;
  const pending = mediaFileInflight.get(id);
  if (pending) return pending;
  const promise = (async () => {
    try {
      const url = convertFileSrc(id, "mediabuddy-media");
      mediaFileCache.set(id, url);
      return url;
    } finally {
      mediaFileInflight.delete(id);
    }
  })();
  mediaFileInflight.set(id, promise);
  return promise;
}

export function dropThumb(id: string) {
  const url = cache.get(id);
  if (url) {
    URL.revokeObjectURL(url);
    cache.delete(id);
  }
  const imageUrl = imageCache.get(id);
  if (imageUrl) {
    URL.revokeObjectURL(imageUrl);
    imageCache.delete(id);
  }
  mediaFileCache.delete(id);
}

// NOTE: we intentionally do NOT auto-evict cached object URLs. The same image
// id can be mounted in more than one place at once (a grid card and the
// Inspector preview), so any global LRU/size-based eviction risks revoking a
// blob URL that is still bound to a live <img>. Growth is bounded by the
// library size and entries are revoked on delete (dropThumb). A safe cap would
// require per-id reference counting.
export function clearAllThumbs() {
  for (const url of cache.values()) URL.revokeObjectURL(url);
  cache.clear();
  for (const url of imageCache.values()) URL.revokeObjectURL(url);
  imageCache.clear();
  mediaFileCache.clear();
}
