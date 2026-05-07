import { createSignal, For, onCleanup, onMount, Show, type Component } from "solid-js";
import { api, type ApiServerStatus } from "../lib/api";
import "./APITab.css";

type RouteEntry = { method: string; path: string; description: string; ready: boolean };

const ROUTES: RouteEntry[] = [
  { method: "GET", path: "/api/v1/status", description: "Health check", ready: true },
  { method: "GET", path: "/api/v1/stats", description: "Library + disk stats", ready: true },
  { method: "GET", path: "/api/v1/images", description: "List images (paginated)", ready: true },
  { method: "GET", path: "/api/v1/images/{id}", description: "Get one image", ready: true },
  { method: "DELETE", path: "/api/v1/images/{id}", description: "Delete one image", ready: true },
  { method: "PUT", path: "/api/v1/images/{id}", description: "Update tags / alt", ready: true },
  { method: "POST", path: "/api/v1/images/delete", description: "Batch delete", ready: true },
  { method: "GET", path: "/api/v1/images/{id}/file", description: "Original image file", ready: true },
  { method: "GET", path: "/api/v1/images/{id}/thumb", description: "Thumbnail", ready: true },
  { method: "POST", path: "/api/v1/images/query", description: "Advanced query", ready: true },
  { method: "GET", path: "/api/v1/search/pixabay", description: "Pixabay search", ready: true },
  { method: "GET", path: "/api/v1/search/pexels", description: "Pexels search", ready: true },
  { method: "GET", path: "/api/v1/search/unsplash", description: "Unsplash search", ready: true },
  { method: "POST", path: "/api/v1/search", description: "Search all sources", ready: true },
  { method: "POST", path: "/api/v1/download", description: "Download a single URL", ready: true },
  { method: "POST", path: "/api/v1/download/batch", description: "Batch download (async task)", ready: true },
  { method: "GET", path: "/api/v1/tasks/{id}", description: "Async task status", ready: true },
  { method: "GET", path: "/api/v1/vision/status", description: "Vision engine status", ready: true },
  { method: "POST", path: "/api/v1/vision/load", description: "Load Florence-2 instances", ready: false },
  { method: "POST", path: "/api/v1/vision/unload", description: "Unload all instances", ready: false },
  { method: "POST", path: "/api/v1/vision/analyze/{id}", description: "Analyze single image", ready: false },
  { method: "POST", path: "/api/v1/vision/analyze", description: "Batch analyze", ready: false },
  { method: "POST", path: "/api/v1/combo/search-download", description: "Search + download chain", ready: true },
  { method: "POST", path: "/api/v1/combo/download-analyze", description: "Download then caption", ready: false },
  { method: "POST", path: "/api/v1/combo/analyze-unprocessed", description: "Analyze unprocessed images", ready: false },
  { method: "POST", path: "/api/v1/combo/smart-analyze", description: "Auto-load + analyze + auto-unload", ready: false },
  { method: "POST", path: "/api/v1/combo/search-download-analyze", description: "Full pipeline", ready: false },
];

const APITab: Component = () => {
  const [status, setStatus] = createSignal<ApiServerStatus | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let timer: number | undefined;

  const refresh = async () => {
    try {
      setStatus(await api.apiStatus());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  onMount(() => {
    refresh();
    timer = window.setInterval(refresh, 2000);
  });
  onCleanup(() => {
    if (timer !== undefined) window.clearInterval(timer);
  });

  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.apiStart());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.apiStop());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const baseUrl = () => {
    const s = status();
    if (!s?.running || !s.host || !s.port) return null;
    const host = s.host === "0.0.0.0" ? "127.0.0.1" : s.host;
    return `http://${host}:${s.port}`;
  };

  const fmtUptime = (sec: number | null) => {
    if (sec == null) return "--";
    const h = Math.floor(sec / 3600);
    const m = Math.floor((sec % 3600) / 60);
    const s = sec % 60;
    return h > 0 ? `${h}h ${m}m ${s}s` : m > 0 ? `${m}m ${s}s` : `${s}s`;
  };

  return (
    <div class="apitab">
      <header class="api-header">
        <div>
          <h2>API</h2>
          <p class="hint">
            Built-in REST server for automation. Same JSON shape as the original
            Python API, hosted by axum in this process.
          </p>
        </div>
      </header>

      <div
        class="api-status"
        classList={{ running: !!status()?.running, stopped: !status()?.running }}
      >
        <div class="api-status-info">
          <div class="api-status-label">
            {status()?.running ? "RUNNING" : "STOPPED"}
          </div>
          <Show when={status()?.running} fallback={<div class="api-status-meta">Server is not listening.</div>}>
            <div class="api-status-meta">
              {status()!.host}:{status()!.port} · uptime {fmtUptime(status()!.uptime_seconds ?? 0)}
            </div>
          </Show>
        </div>
        <div class="api-status-actions">
          <Show
            when={status()?.running}
            fallback={
              <button type="button" class="primary" onClick={start} disabled={busy()}>
                {busy() ? "Starting..." : "Start server"}
              </button>
            }
          >
            <button type="button" class="danger" onClick={stop} disabled={busy()}>
              {busy() ? "Stopping..." : "Stop server"}
            </button>
          </Show>
        </div>
      </div>

      <Show when={baseUrl()}>
        <div class="api-baseurl">
          Base URL:{" "}
          <code class="mono-inline">{baseUrl()}</code>
          <span class="hint" style={{ "margin-left": "10px" }}>
            host/port/CORS configured in the Settings tab; takes effect on next start.
          </span>
        </div>
      </Show>

      <Show when={error()}>
        <div class="api-error">Error: {error()}</div>
      </Show>

      <section class="api-routes">
        <h3>Endpoints</h3>
        <div class="route-list">
          <For each={ROUTES}>
            {(r) => (
              <div class="route-row" classList={{ stub: !r.ready }}>
                <span class="route-method" classList={{ [`method-${r.method.toLowerCase()}`]: true }}>
                  {r.method}
                </span>
                <code class="route-path">{r.path}</code>
                <span class="route-desc">{r.description}</span>
                <span class="route-state">
                  {r.ready ? "ready" : "stub (vision phase)"}
                </span>
              </div>
            )}
          </For>
        </div>
      </section>
    </div>
  );
};

export default APITab;
