import { createSignal, For, onCleanup, onMount, Show, type Component } from "solid-js";
import { api, type LogEntry } from "../lib/api";
import "./LogTab.css";

const LEVELS = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];
const POLL_MS = 1000;

const LogTab: Component = () => {
  const [entries, setEntries] = createSignal<LogEntry[]>([]);
  const [level, setLevel] = createSignal("INFO");
  const [autoScroll, setAutoScroll] = createSignal(true);
  const [error, setError] = createSignal<string | null>(null);
  let scrollEl: HTMLDivElement | undefined;
  let timer: number | undefined;

  const tick = async () => {
    try {
      const fresh = await api.getLogs(undefined, level());
      setEntries(fresh);
      setError(null);
      if (autoScroll() && scrollEl) {
        queueMicrotask(() => {
          if (scrollEl) scrollEl.scrollTop = scrollEl.scrollHeight;
        });
      }
    } catch (e) {
      setError(String(e));
    }
  };

  onMount(() => {
    tick();
    timer = window.setInterval(tick, POLL_MS);
  });
  onCleanup(() => {
    if (timer !== undefined) window.clearInterval(timer);
  });

  const clear = async () => {
    try {
      await api.clearLogs();
      await tick();
    } catch (e) {
      setError(String(e));
    }
  };

  const fmtTs = (unix: number) => {
    const d = new Date(unix * 1000);
    const hh = String(d.getHours()).padStart(2, "0");
    const mm = String(d.getMinutes()).padStart(2, "0");
    const ss = String(d.getSeconds()).padStart(2, "0");
    return `${hh}:${mm}:${ss}`;
  };

  return (
    <div class="logtab">
      <header class="log-header">
        <div>
          <h2>Log</h2>
          <p class="hint">
            Live in-process log buffer (last 1000 entries). Pulled every second.
          </p>
        </div>
        <div class="log-actions">
          <label class="inline">
            <span class="inline-label">Level</span>
            <select
              value={level()}
              onChange={(e) => setLevel(e.currentTarget.value)}
            >
              <For each={LEVELS}>
                {(l) => <option value={l}>{l}</option>}
              </For>
            </select>
          </label>
          <label class="checkbox">
            <input
              type="checkbox"
              checked={autoScroll()}
              onChange={(e) => setAutoScroll(e.currentTarget.checked)}
            />
            Auto-scroll
          </label>
          <button type="button" onClick={tick}>
            Refresh
          </button>
          <button type="button" class="danger" onClick={clear}>
            Clear
          </button>
        </div>
      </header>

      <Show when={error()}>
        <div class="log-error">Error: {error()}</div>
      </Show>

      <div class="log-window" ref={scrollEl}>
        <For
          each={entries()}
          fallback={<div class="log-empty">No log entries at this level.</div>}
        >
          {(entry) => (
            <div
              class="log-row"
              classList={{
                [`level-${entry.level.toLowerCase()}`]: true,
              }}
            >
              <span class="log-time">{fmtTs(entry.timestamp)}</span>
              <span class="log-level">{entry.level}</span>
              <span class="log-target">{shortTarget(entry.target)}</span>
              <span class="log-message">{entry.message}</span>
            </div>
          )}
        </For>
      </div>
    </div>
  );
};

const shortTarget = (target: string) =>
  target.replace(/^mediabuddy_lib::/, "").replace(/^mediabuddy::/, "");

export default LogTab;
