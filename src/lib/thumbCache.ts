import { api } from "./api";

const cache = new Map<string, string>();
const inflight = new Map<string, Promise<string>>();

export async function getThumbUrl(id: string): Promise<string> {
  const cached = cache.get(id);
  if (cached) return cached;
  const pending = inflight.get(id);
  if (pending) return pending;
  const promise = (async () => {
    const bytes = await api.readThumbBytes(id);
    const blob = new Blob([new Uint8Array(bytes)], { type: "image/jpeg" });
    const url = URL.createObjectURL(blob);
    cache.set(id, url);
    inflight.delete(id);
    return url;
  })();
  inflight.set(id, promise);
  return promise;
}

export function dropThumb(id: string) {
  const url = cache.get(id);
  if (url) {
    URL.revokeObjectURL(url);
    cache.delete(id);
  }
}

export function clearAllThumbs() {
  for (const url of cache.values()) URL.revokeObjectURL(url);
  cache.clear();
}
