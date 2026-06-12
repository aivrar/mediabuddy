import { openUrl } from "@tauri-apps/plugin-opener";
import { createSignal, For, onCleanup, onMount, Show, type Component } from "solid-js";
import { api, type ApiServerStatus, type Settings } from "../lib/api";
import "./APITab.css";

type RouteEntry = { method: string; path: string; description: string; ready: boolean };

const ROUTES: RouteEntry[] = [
  { method: "GET", path: "/api/v1/status", description: "Health check", ready: true },
  { method: "GET", path: "/api/v1/stats", description: "Library + disk stats", ready: true },
  { method: "GET", path: "/api/v1/docs", description: "Route docs, auth pattern, limits", ready: true },
  { method: "GET", path: "/api/v1/openapi.json", description: "Minimal OpenAPI document", ready: true },
  { method: "GET", path: "/api/v1/settings", description: "Read redacted app settings", ready: true },
  { method: "PUT", path: "/api/v1/settings", description: "Patch settings / provider keys", ready: true },
  { method: "POST", path: "/api/v1/api-keys/validate", description: "Validate and optionally save provider keys", ready: true },
  { method: "GET", path: "/api/v1/quota", description: "Provider quota snapshot", ready: true },
  { method: "GET", path: "/api/v1/logs", description: "Read app logs", ready: true },
  { method: "DELETE", path: "/api/v1/logs", description: "Clear app logs", ready: true },
  { method: "POST", path: "/api/v1/app/shutdown", description: "Shut down Media Buddy", ready: true },
  { method: "GET", path: "/api/v1/images", description: "List media (paginated, filter by source/query/kind)", ready: true },
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
  { method: "GET", path: "/api/v1/topics", description: "List persistent search topics", ready: true },
  { method: "POST", path: "/api/v1/topics", description: "Create or resume a search topic", ready: true },
  { method: "GET", path: "/api/v1/topics/{id}", description: "Topic cursor status", ready: true },
  { method: "POST", path: "/api/v1/topics/{id}/more", description: "Fetch the next topic result round", ready: true },
  { method: "POST", path: "/api/v1/topics/{id}/reset", description: "Reset topic cursors and seen list", ready: true },
  { method: "PUT", path: "/api/v1/topics/{id}", description: "Rename a topic", ready: true },
  { method: "DELETE", path: "/api/v1/topics/{id}", description: "Delete a topic, keeping saved media", ready: true },
  { method: "GET", path: "/api/v1/topics/{id}/images", description: "List saved library IDs touched by a topic", ready: true },
  { method: "GET", path: "/api/v1/vision/status", description: "Vision engine status", ready: true },
  { method: "POST", path: "/api/v1/vision/load", description: "Load Florence-2 instances", ready: true },
  { method: "POST", path: "/api/v1/vision/unload", description: "Unload all instances", ready: true },
  { method: "POST", path: "/api/v1/vision/analyze/{id}", description: "Analyze single image", ready: true },
  { method: "POST", path: "/api/v1/vision/analyze", description: "Batch analyze", ready: true },
  { method: "POST", path: "/api/v1/combo/search-download", description: "Search + download chain", ready: true },
  { method: "POST", path: "/api/v1/combo/download-analyze", description: "Download one URL, then analyze it", ready: true },
  { method: "POST", path: "/api/v1/combo/analyze-unprocessed", description: "Analyze unprocessed library images", ready: true },
  { method: "POST", path: "/api/v1/combo/smart-analyze", description: "Auto-load, analyze, then optionally unload", ready: true },
  { method: "POST", path: "/api/v1/combo/search-download-analyze", description: "Search, download, then analyze saved images", ready: true },
];

const APITab: Component = () => {
  const [status, setStatus] = createSignal<ApiServerStatus | null>(null);
  const [settings, setSettings] = createSignal<Settings | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [tokenVisible, setTokenVisible] = createSignal(false);
  const [copied, setCopied] = createSignal<string | null>(null);
  let timer: number | undefined;

  const refresh = async () => {
    try {
      const [nextStatus, nextSettings] = await Promise.all([
        api.apiStatus(),
        api.getSettings(),
      ]);
      setStatus(nextStatus);
      setSettings(nextSettings);
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

  const apiToken = () => settings()?.api_token?.trim() ?? "";
  const authHeader = () =>
    apiToken() ? `Authorization: Bearer ${apiToken()}` : "No REST token configured";
  const tokenDisplay = () => {
    const token = apiToken();
    if (!token) return "No token";
    if (tokenVisible()) return token;
    return `${"*".repeat(Math.max(12, Math.min(token.length, 24)))}${token.slice(-4)}`;
  };
  const curlListImages = () => {
    const url = baseUrl() ?? "http://127.0.0.1:5000";
    const token = apiToken();
    const auth = token ? ` -H "Authorization: Bearer ${token}"` : "";
    return `curl${auth} "${url}/api/v1/images?per_page=10"`;
  };
  const curlSearch = () => {
    const url = baseUrl() ?? "http://127.0.0.1:5000";
    const token = apiToken();
    const auth = token ? ` -H "Authorization: Bearer ${token}"` : "";
    return `curl${auth} -H "Content-Type: application/json" -d "{\\"query\\":\\"manta ray\\",\\"kind\\":\\"photo\\",\\"sources\\":{\\"pixabay\\":1,\\"pexels\\":1,\\"unsplash\\":1}}" "${url}/api/v1/search"`;
  };
  const copyText = async (label: string, text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(label);
      window.setTimeout(() => setCopied(null), 1500);
    } catch (e) {
      setError(String(e));
    }
  };

  const openDocs = async () => {
    const url = baseUrl();
    if (!url) return;
    try {
      await openUrl(`${url}/api/v1/docs`);
    } catch (e) {
      setError(String(e));
    }
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
          <Show when={baseUrl()}>
            <button type="button" onClick={openDocs}>
              Open docs
            </button>
          </Show>
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

      <section class="api-auth">
        <div class="api-auth-head">
          <div>
            <h3>Security</h3>
            <p class="hint">Authenticated routes accept either header shown below. Status is public.</p>
          </div>
          <button type="button" onClick={() => setTokenVisible((v) => !v)}>
            {tokenVisible() ? "Hide token" : "Show token"}
          </button>
        </div>
        <div class="api-token-row">
          <span class="token-label">REST token</span>
          <code class="mono-inline token-value">{tokenDisplay()}</code>
          <button
            type="button"
            disabled={!apiToken()}
            onClick={() => copyText("token", apiToken())}
          >
            {copied() === "token" ? "Copied" : "Copy token"}
          </button>
          <button
            type="button"
            disabled={!apiToken()}
            onClick={() => copyText("auth", authHeader())}
          >
            {copied() === "auth" ? "Copied" : "Copy header"}
          </button>
        </div>
        <div class="api-example-grid">
          <div class="api-example">
            <div class="example-title">List library</div>
            <code>{curlListImages()}</code>
            <button type="button" onClick={() => copyText("curl-list", curlListImages())}>
              {copied() === "curl-list" ? "Copied" : "Copy"}
            </button>
          </div>
          <div class="api-example">
            <div class="example-title">Search providers</div>
            <code>{curlSearch()}</code>
            <button type="button" onClick={() => copyText("curl-search", curlSearch())}>
              {copied() === "curl-search" ? "Copied" : "Copy"}
            </button>
          </div>
        </div>
      </section>

      <Show when={error()}>
        <div class="api-error">Error: {error()}</div>
      </Show>

      <section class="api-routes">
        <h3>Endpoints</h3>
        <div class="route-list">
          <For each={ROUTES}>
            {(r) => (
              <div class="route-row" classList={{ pending: !r.ready }}>
                <span class="route-method" classList={{ [`method-${r.method.toLowerCase()}`]: true }}>
                  {r.method}
                </span>
                <code class="route-path">{r.path}</code>
                <span class="route-desc">{r.description}</span>
                <span class="route-state">{r.ready ? "ready" : "manual"}</span>
              </div>
            )}
          </For>
        </div>
      </section>
    </div>
  );
};

export default APITab;
