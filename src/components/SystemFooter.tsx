import { createSignal, For, onCleanup, onMount, Show, type Component } from "solid-js";
import { api, type SystemStats } from "../lib/api";
import "./SystemFooter.css";

const POLL_MS = 1500;

const SystemFooter: Component = () => {
  const [stats, setStats] = createSignal<SystemStats | null>(null);
  const [error, setError] = createSignal(false);
  let timer: number | undefined;

  const tick = async () => {
    try {
      setStats(await api.getSystemStats());
      setError(false);
    } catch {
      setError(true);
    }
  };

  onMount(() => {
    tick();
    timer = window.setInterval(tick, POLL_MS);
  });

  onCleanup(() => {
    if (timer !== undefined) window.clearInterval(timer);
  });

  const cpu = () => stats()?.cpu_percent ?? null;
  const ram = () => stats()?.ram_percent ?? null;
  const ramUsed = () => stats()?.ram_used_gb ?? null;
  const ramTotal = () => stats()?.ram_total_gb ?? null;
  const gpus = () => stats()?.gpus ?? [];

  const fmtPct = (n: number | null) =>
    n == null ? "--" : `${n.toFixed(0)}%`;

  return (
    <footer class="system-footer">
      <div class="footer-meta">
        <span class="footer-app-name">Media Buddy</span>
        <span class="footer-version">v0.1.0</span>
      </div>

      <div class="footer-stats">
        <Stat label="CPU" value={fmtPct(cpu())} bar={cpu()} />
        <Stat
          label="RAM"
          value={
            ramUsed() != null && ramTotal() != null
              ? `${ramUsed()!.toFixed(1)} / ${ramTotal()!.toFixed(0)} GB (${fmtPct(ram())})`
              : "--"
          }
          bar={ram()}
        />

        <Show
          when={gpus().length > 0}
          fallback={
            <span class="stat">
              <span class="stat-label">GPU</span>
              <span class="stat-value">{error() ? "n/a" : "no GPU"}</span>
            </span>
          }
        >
          <For each={gpus()}>
            {(g) => (
              <Stat
                label={g.name.length > 18 ? `GPU ${g.index}` : g.name}
                value={`${fmtPct(g.util_percent)}  ${g.vram_used_gb.toFixed(1)}/${g.vram_total_gb.toFixed(0)} GB${g.temp_c != null ? `  ${g.temp_c}°C` : ""}`}
                bar={g.util_percent}
              />
            )}
          </For>
        </Show>
      </div>
    </footer>
  );
};

const Stat: Component<{ label: string; value: string; bar?: number | null }> = (
  props
) => {
  return (
    <span class="stat" title={props.label}>
      <span class="stat-label">{props.label}</span>
      <span class="stat-value">{props.value}</span>
      <Show when={props.bar != null}>
        <span class="stat-bar">
          <span
            class="stat-bar-fill"
            style={{ width: `${Math.min(100, Math.max(0, props.bar!))}%` }}
          />
        </span>
      </Show>
    </span>
  );
};

export default SystemFooter;
