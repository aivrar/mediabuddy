import { openUrl } from "@tauri-apps/plugin-opener";
import { createSignal, onMount, Show, type Component } from "solid-js";
import {
  api,
  type ApiProvider,
  type KeyProbe,
  type Settings,
  type VisionLoadParams,
  type VisionStatus,
} from "../lib/api";
import "./SettingsTab.css";

type ProviderHelp = {
  name: string;
  getKeyUrl: string;
  docsUrl: string;
  keyHint: string;
  placeholder?: string;
};

const PROVIDER_HELP: Record<ApiProvider, ProviderHelp> = {
  pixabay: {
    name: "Pixabay",
    getKeyUrl: "https://pixabay.com/api/docs/",
    docsUrl: "https://pixabay.com/api/docs/",
    keyHint:
      "Sign in or join, then copy the API key shown in the docs next to the key parameter.",
    placeholder: "33929247-...",
  },
  pexels: {
    name: "Pexels",
    getKeyUrl: "https://www.pexels.com/api/?locale=en-US",
    docsUrl: "https://www.pexels.com/api/documentation/",
    keyHint:
      "Create a Pexels account, request the API key from the API page, then paste that single account key here.",
  },
  unsplash: {
    name: "Unsplash",
    getKeyUrl: "https://unsplash.com/oauth/applications",
    docsUrl: "https://unsplash.com/documentation",
    keyHint:
      "Create or open an Unsplash developer application and paste the Access Key. Do not paste the Secret Key.",
  },
};

const SettingsTab: Component = () => {
  const [settings, setSettings] = createSignal<Settings | null>(null);
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal<string | null>(null);
  const [saved, setSaved] = createSignal(false);
  const [revealKeys, setRevealKeys] = createSignal(false);
  const [visionStatus, setVisionStatus] = createSignal<VisionStatus | null>(null);
  const [visionBusy, setVisionBusy] = createSignal(false);
  const [visionError, setVisionError] = createSignal<string | null>(null);
  const [visionNotice, setVisionNotice] = createSignal<string | null>(null);

  const errorMessage = (e: unknown) => (e instanceof Error ? e.message : String(e));

  const visionParams = (
    s: Settings | null,
    allowCpuFallback?: boolean
  ): VisionLoadParams => {
    const cpuInstances = Math.max(
      1,
      Math.min(16, Number(s?.vision_cpu_instances) || 1)
    );
    const cpuThreads = Math.max(
      0,
      Math.min(128, Number(s?.vision_cpu_threads_per_instance) || 0)
    );

    return {
      precision: "fp32",
      mode: s?.vision_execution_mode ?? "auto",
      cpuInstances,
      gpuInstancesPerGpu: Math.max(
        0,
        Math.min(16, Number(s?.vision_max_per_gpu) || 0)
      ),
      maxTotalInstances: Math.max(
        1,
        Math.min(32, Number(s?.vision_max_total) || 1)
      ),
      reservedVramGb: Math.max(0, Number(s?.vision_reserved_vram) || 0),
      allowCpuFallback: allowCpuFallback ?? !!s?.vision_allow_cpu,
      cpuThreadsPerInstance: cpuThreads || undefined,
    };
  };

  const refreshVision = async () => {
    try {
      setVisionStatus(await api.visionStatus());
      setVisionError(null);
    } catch (e) {
      setVisionError(errorMessage(e));
    }
  };

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      setSettings(await api.getSettings());
      await refreshVision();
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  onMount(load);

  const update = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    const cur = settings();
    if (!cur) return;
    setSettings({ ...cur, [key]: value });
    setSaved(false);
  };

  const save = async () => {
    const s = settings();
    if (!s) return;
    await saveSettingsSnapshot(s);
  };

  const saveSettingsSnapshot = async (s: Settings) => {
    setError(null);
    try {
      await api.saveSettings(s);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      setError(String(e));
    }
  };

  const openExternal = async (label: string, url: string) => {
    setError(null);
    try {
      await openUrl(url);
    } catch (e) {
      setError(`Could not open ${label}: ${errorMessage(e)}`);
    }
  };

  const loadVision = async () => {
    const s = settings();
    const params = visionParams(s);
    setVisionBusy(true);
    setVisionError(null);
    setVisionNotice(null);
    try {
      setVisionStatus(await api.visionLoad(params));
    } catch (e) {
      const firstError = errorMessage(e);
      if (s && !s.vision_allow_cpu && s.vision_execution_mode !== "cpu") {
        try {
          setVisionStatus(await api.visionLoad(visionParams(s, true)));
          const savedFallback = { ...s, vision_allow_cpu: true };
          setSettings(savedFallback);
          await saveSettingsSnapshot(savedFallback);
          setVisionNotice(
            "GPU load failed. CPU fallback was enabled and saved for future loads."
          );
        } catch (retryError) {
          setVisionError(
            `GPU load failed: ${firstError}. CPU fallback retry failed: ${errorMessage(
              retryError
            )}`
          );
        }
      } else {
        setVisionError(firstError);
      }
    } finally {
      setVisionBusy(false);
    }
  };

  const unloadVision = async () => {
    setVisionBusy(true);
    setVisionError(null);
    setVisionNotice(null);
    try {
      setVisionStatus(await api.visionUnload());
    } catch (e) {
      setVisionError(errorMessage(e));
    } finally {
      setVisionBusy(false);
    }
  };

  return (
    <div class="settings">
      <header class="settings-header">
        <div>
          <h2>Settings</h2>
          <p class="hint">
            API keys, REST server config, and theme. Valid API keys save after
            a successful test; other changes apply on save. The REST server
            picks up new host/port on its next start.
          </p>
        </div>
        <div class="settings-actions">
          <Show when={saved()}>
            <span class="saved-pill">Saved</span>
          </Show>
          <button type="button" onClick={load}>
            Reload
          </button>
          <button
            type="button"
            class="primary"
            onClick={save}
            disabled={!settings() || loading()}
          >
            Save
          </button>
        </div>
      </header>

      <Show when={error()}>
        <div class="settings-error">Error: {error()}</div>
      </Show>

      <Show
        when={settings()}
        fallback={<div class="placeholder-line">Loading...</div>}
      >
        {(s) => (
          <>
            <div class="settings-guide">
              <div class="settings-guide-item">
                <strong>Provider keys</strong>
                <span>Open a provider page, paste its key, then Test & save.</span>
              </div>
              <div class="settings-guide-item">
                <strong>Search library</strong>
                <span>Use Images for searching, downloads, topics, and inspection.</span>
              </div>
              <div class="settings-guide-item">
                <strong>Automation</strong>
                <span>Use the API tab for the REST server, token, docs, and examples.</span>
              </div>
              <div class="settings-guide-item">
                <strong>AI captions</strong>
                <span>Load Florence here, then run captioning from selected library items.</span>
              </div>
            </div>

            <div class="settings-grid">
            <Section title="Stock image API keys">
              <div class="section-intro">
                <strong>Key setup</strong>
                <span>
                  Valid tests save automatically. Failed tests can be retried after
                  checking the key or waiting for a provider to recover.
                </span>
              </div>
              <div class="row-actions">
                <label class="checkbox">
                  <input
                    type="checkbox"
                    checked={revealKeys()}
                    onChange={(e) => setRevealKeys(e.currentTarget.checked)}
                  />
                  Show keys
                </label>
              </div>
              <KeyField
                label="Pixabay key"
                provider="pixabay"
                value={s().pixabay_key}
                reveal={revealKeys()}
                meta={PROVIDER_HELP.pixabay}
                onOpen={openExternal}
                onInput={(v) => update("pixabay_key", v)}
                onValid={saveSettingsSnapshot}
                settings={s()}
              />
              <KeyField
                label="Pexels key"
                provider="pexels"
                value={s().pexels_key}
                reveal={revealKeys()}
                meta={PROVIDER_HELP.pexels}
                onOpen={openExternal}
                onInput={(v) => update("pexels_key", v)}
                onValid={saveSettingsSnapshot}
                settings={s()}
              />
              <KeyField
                label="Unsplash key"
                provider="unsplash"
                value={s().unsplash_key}
                reveal={revealKeys()}
                meta={PROVIDER_HELP.unsplash}
                onOpen={openExternal}
                onInput={(v) => update("unsplash_key", v)}
                onValid={saveSettingsSnapshot}
                settings={s()}
              />

              <Field label="Unsplash detail-fetch cutoff">
                <input
                  type="number"
                  min="0"
                  max="500"
                  step="5"
                  value={s().unsplash_detail_threshold}
                  onInput={(e) =>
                    update(
                      "unsplash_detail_threshold",
                      Math.max(
                        0,
                        Math.min(500, Number(e.currentTarget.value) || 0)
                      )
                    )
                  }
                />
              </Field>
              <p class="hint" style={{ "margin-top": "-4px" }}>
                Each Unsplash result triggers a per-photo detail call (extra metadata) - when
                a single search asks for more items than this number, the detail calls are
                skipped to protect your hourly quota. Set 0 to skip details always. Default 30.
              </p>
            </Section>

            <Section title="Appearance">
              <Field label="Theme">
                <select
                  value={s().theme}
                  onChange={(e) => {
                    update("theme", e.currentTarget.value);
                    document.documentElement.setAttribute(
                      "data-theme",
                      e.currentTarget.value
                    );
                  }}
                >
                  <option value="dark">Dark</option>
                  <option value="light">Light</option>
                  <option value="graphite">Graphite</option>
                  <option value="forest">Forest</option>
                  <option value="violet">Violet</option>
                  <option value="high-contrast">High contrast</option>
                </select>
              </Field>
            </Section>

            <Section title="REST API server">
              <div class="section-intro">
                <strong>Local automation</strong>
                <span>
                  Start the server from the API tab. Host, port, CORS, and bearer
                  token are saved here.
                </span>
              </div>
              <Field label="Host">
                <input
                  type="text"
                  value={s().api_host}
                  onInput={(e) => update("api_host", e.currentTarget.value)}
                />
              </Field>
              <Field label="Port">
                <input
                  type="number"
                  min="1"
                  max="65535"
                  value={s().api_port}
                  onInput={(e) =>
                    update("api_port", Number(e.currentTarget.value) || 5000)
                  }
                />
              </Field>
              <Field label="Auto-start on app launch">
                <label class="checkbox">
                  <input
                    type="checkbox"
                    checked={s().api_auto_start}
                    onChange={(e) =>
                      update("api_auto_start", e.currentTarget.checked)
                    }
                  />
                  Start the REST server when Media Buddy starts
                </label>
              </Field>
              <Field label="CORS">
                <label class="checkbox">
                  <input
                    type="checkbox"
                    checked={s().api_cors_enabled}
                    onChange={(e) =>
                      update("api_cors_enabled", e.currentTarget.checked)
                    }
                  />
                  Allow browser requests from loopback origins
                </label>
              </Field>
              <Field label="Bearer token">
                <div class="key-row">
                  <input
                    type={revealKeys() ? "text" : "password"}
                    value={s().api_token}
                    onInput={(e) => update("api_token", e.currentTarget.value)}
                    placeholder="API bearer token"
                    autocomplete="off"
                    spellcheck={false}
                  />
                  <button
                    type="button"
                    class="key-test-btn"
                    onClick={() => update("api_token", randomToken(40))}
                    title="Generate a fresh random token"
                  >
                    Generate
                  </button>
                  <button
                    type="button"
                    class="key-test-btn"
                    onClick={() => update("api_token", "")}
                    title="Clear token for local-only development"
                  >
                    Clear
                  </button>
                </div>
              </Field>
              <p class="hint" style={{ "margin-top": "-4px" }}>
                When set, every <code>/api/v1/*</code> request must carry{" "}
                <code>Authorization: Bearer &lt;token&gt;</code>. The{" "}
                <code>/api/v1/status</code> endpoint stays open so external
                health checks still work. A token is generated by default;
                clearing it disables auth and is intended only for local-only
                development.
              </p>
            </Section>

            <Section title="AI vision (Florence-2)">
              <div class="section-intro">
                <strong>Model loading</strong>
                <span>
                  Auto chooses the best available runtime. GPU workers are used when
                  compatible; CPU workers load only for CPU mode or if GPU planning
                  fails and fallback is allowed.
                </span>
              </div>
              <div
                class="vision-runtime"
                classList={{ loaded: !!visionStatus()?.loaded }}
              >
                <div>
                  <div class="vision-state">
                    {visionStatus()?.loaded ? "Loaded" : "Not loaded"}
                  </div>
                  <div class="vision-meta">
                    <Show when={visionStatus()} fallback="Status unavailable">
                      {visionStatus()?.loaded
                        ? `${visionWorkerSummary(visionStatus())} - ${
                            visionStatus()!.precision ?? "fp32"
                          }`
                          : "Ready to load fp32 model"
                      }
                    </Show>
                  </div>
                  <Show when={visionStatus()?.model_dir}>
                    <div class="vision-path">{visionStatus()!.model_dir}</div>
                  </Show>
                </div>
                <div class="vision-actions">
                  <button
                    type="button"
                    onClick={refreshVision}
                    disabled={visionBusy()}
                  >
                    Refresh
                  </button>
                  <Show
                    when={visionStatus()?.loaded}
                    fallback={
                      <button
                        type="button"
                        class="primary"
                        onClick={loadVision}
                        disabled={visionBusy()}
                      >
                        {visionBusy() ? "Loading..." : "Load"}
                      </button>
                    }
                  >
                    <button
                      type="button"
                      class="danger"
                      onClick={unloadVision}
                      disabled={visionBusy()}
                    >
                      {visionBusy() ? "Unloading..." : "Unload"}
                    </button>
                  </Show>
                </div>
              </div>
              <Show when={visionError()}>
                <div class="settings-error">Vision: {visionError()}</div>
              </Show>
              <Show when={visionNotice()}>
                <div class="settings-warning">{visionNotice()}</div>
              </Show>
              <Show when={(visionStatus()?.warnings?.length ?? 0) > 0}>
                <div class="settings-warning">
                  {visionStatus()!.warnings.join(" ")}
                </div>
              </Show>
              <Show when={(visionStatus()?.devices?.length ?? 0) > 0}>
                <div class="vision-detail-list">
                  {visionStatus()!.devices.map((device) => (
                    <span>
                      {device.provider.toUpperCase()} {device.device_id}:{" "}
                      {device.name} - {device.selected_instances} worker
                      {device.selected_instances === 1 ? "" : "s"} -{" "}
                      {device.dedicated_vram_gb.toFixed(1)} GB
                    </span>
                  ))}
                </div>
              </Show>
              <Show when={(visionStatus()?.workers?.length ?? 0) > 0}>
                <div class="vision-detail-list">
                  {visionStatus()!.workers.map((worker) => (
                    <span>
                      #{worker.index + 1} {worker.provider}
                      {worker.device_id != null ? `:${worker.device_id}` : ""}{" "}
                      {worker.intra_threads}t
                    </span>
                  ))}
                </div>
              </Show>
              <Field label="Execution target">
                <select
                  value={s().vision_execution_mode}
                  onChange={(e) =>
                    update("vision_execution_mode", e.currentTarget.value)
                  }
                >
                  <option value="auto">Auto</option>
                  <option value="directml">DirectML</option>
                  <option value="cuda">CUDA</option>
                  <option value="cpu">CPU</option>
                </select>
              </Field>
              <Field label="Allow CPU fallback">
                <label class="checkbox">
                  <input
                    type="checkbox"
                    checked={s().vision_allow_cpu}
                    onChange={(e) =>
                      update("vision_allow_cpu", e.currentTarget.checked)
                    }
                  />
                  Use CPU workers only if GPU planning or loading fails
                </label>
              </Field>
              <Field label="CPU instances">
                <input
                  type="number"
                  min="1"
                  max="16"
                  value={s().vision_cpu_instances}
                  onInput={(e) =>
                    update(
                      "vision_cpu_instances",
                      Math.max(
                        1,
                        Math.min(16, Number(e.currentTarget.value) || 1)
                      )
                    )
                  }
                />
              </Field>
              <Field label="CPU threads per instance">
                <input
                  type="number"
                  min="0"
                  max="128"
                  value={s().vision_cpu_threads_per_instance}
                  onInput={(e) =>
                    update(
                      "vision_cpu_threads_per_instance",
                      Math.max(
                        0,
                        Math.min(128, Number(e.currentTarget.value) || 0)
                      )
                    )
                  }
                />
              </Field>
              <Field label="GPU instances per GPU">
                <input
                  type="number"
                  min="0"
                  max="16"
                  value={s().vision_max_per_gpu}
                  onInput={(e) =>
                    update(
                      "vision_max_per_gpu",
                      Math.max(
                        0,
                        Math.min(16, Number(e.currentTarget.value) || 0)
                      )
                    )
                  }
                />
              </Field>
              <Field label="Max total instances">
                <input
                  type="number"
                  min="1"
                  max="32"
                  value={s().vision_max_total}
                  onInput={(e) =>
                    update(
                      "vision_max_total",
                      Math.max(
                        1,
                        Math.min(32, Number(e.currentTarget.value) || 1)
                      )
                    )
                  }
                />
              </Field>
              <Field label="Reserved VRAM (GB)">
                <input
                  type="number"
                  min="0"
                  max="128"
                  step="0.25"
                  value={s().vision_reserved_vram}
                  onInput={(e) =>
                    update(
                      "vision_reserved_vram",
                      Math.max(0, Number(e.currentTarget.value) || 0)
                    )
                  }
                />
              </Field>
            </Section>
          </div>
          </>
        )}
      </Show>
    </div>
  );
};

const Section: Component<{ title: string; children: any }> = (props) => (
  <section class="settings-section">
    <h3>{props.title}</h3>
    <div class="section-body">{props.children}</div>
  </section>
);

function providerLabel(provider: string): string {
  switch (provider.toLowerCase()) {
    case "cpu":
      return "CPU";
    case "directml":
      return "DirectML";
    case "cuda":
      return "CUDA";
    default:
      return provider;
  }
}

function visionWorkerSummary(status: VisionStatus | null): string {
  if (!status?.workers?.length) {
    return status?.runtime ? providerLabel(status.runtime) : "CPU";
  }

  const counts = new Map<string, number>();
  for (const worker of status.workers) {
    counts.set(worker.provider, (counts.get(worker.provider) ?? 0) + 1);
  }

  return Array.from(counts.entries())
    .map(([provider, count]) => {
      const suffix = count === 1 ? "worker" : "workers";
      return `${count} ${providerLabel(provider)} ${suffix}`;
    })
    .join(" + ");
}

function randomToken(len: number): string {
  const alphabet =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  const n = alphabet.length; // 62
  // Reject bytes >= the largest multiple of n that fits in a byte, so the
  // mapping is uniform (plain `byte % 62` slightly favours the first chars).
  const limit = 256 - (256 % n); // 248
  const buf = new Uint8Array(1);
  let out = "";
  while (out.length < len) {
    crypto.getRandomValues(buf);
    if (buf[0] >= limit) continue;
    out += alphabet[buf[0] % n];
  }
  return out;
}

const Field: Component<{ label: string; children: any }> = (props) => (
  <label class="field">
    <span class="field-label">{props.label}</span>
    <span class="field-input">{props.children}</span>
  </label>
);

const KeyField: Component<{
  label: string;
  provider: ApiProvider;
  value: string;
  reveal: boolean;
  meta: ProviderHelp;
  onOpen: (label: string, url: string) => Promise<void>;
  onInput: (v: string) => void;
  onValid: (settings: Settings) => Promise<void>;
  settings: Settings;
}> = (props) => {
  const [probing, setProbing] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [result, setResult] = createSignal<KeyProbe | null>(null);

  const test = async () => {
    setProbing(true);
    setResult(null);
    try {
      const probe = await api.validateApiKey(props.provider, props.value);
      if (probe.valid) {
        setSaving(true);
        try {
          await props.onValid(props.settings);
          setResult({
            ...probe,
            message: `${probe.message} Saved to settings.`,
          });
        } finally {
          setSaving(false);
        }
      } else {
        setResult(probe);
      }
    } catch (e) {
      setResult({
        provider: props.provider,
        valid: false,
        status_code: null,
        message: String(e),
        rate_limit: null,
        rate_remaining: null,
        reset_seconds: null,
      });
    } finally {
      setProbing(false);
    }
  };

  const fmtReset = (sec: number | null) => {
    if (sec == null) return null;
    if (sec < 60) return `resets in ${sec}s`;
    if (sec < 3600) return `resets in ${Math.round(sec / 60)}m`;
    if (sec < 86400 * 2) return `resets in ${Math.round(sec / 3600)}h`;
    return null;
  };

  const btnState = () => {
    const r = result();
    if (probing() || saving()) return "loading";
    if (!r) return "idle";
    return r.valid ? "valid" : "invalid";
  };

  const btnLabel = () => {
    if (probing()) return "Testing...";
    if (saving()) return "Saving...";
    const r = result();
    if (!r) return "Test & save";
    return r.valid ? "Saved" : "Retry test";
  };

  return (
    <div class="key-field-block">
      <div class="key-field-head">
        <div>
          <div class="key-provider-name">{props.meta.name}</div>
          <p class="key-provider-help">{props.meta.keyHint}</p>
        </div>
        <div class="provider-link-actions">
          <button
            type="button"
            class="provider-link-btn"
            onClick={() =>
              props.onOpen(`${props.meta.name} key page`, props.meta.getKeyUrl)
            }
          >
            Get key
          </button>
          <button
            type="button"
            class="provider-link-btn"
            onClick={() =>
              props.onOpen(`${props.meta.name} docs`, props.meta.docsUrl)
            }
          >
            Docs
          </button>
        </div>
      </div>
      <label class="field">
        <span class="field-label">{props.label}</span>
        <span class="field-input">
          <div class="key-row">
            <input
              type={props.reveal ? "text" : "password"}
              value={props.value}
              onInput={(e) => {
                props.onInput(e.currentTarget.value);
                if (result() !== null) setResult(null);
              }}
              placeholder={props.meta.placeholder}
              autocomplete="off"
              spellcheck={false}
            />
            <button
              type="button"
              class="key-test-btn"
              data-state={btnState()}
              onClick={test}
              disabled={probing() || saving() || !props.value.trim()}
              title={
                result()?.valid
                  ? `Retest and save the ${props.provider} key`
                  : `Test the ${props.provider} key; failed tests can be retried`
              }
            >
              {btnLabel()}
            </button>
          </div>
        </span>
      </label>
      <Show when={result()}>
        {(r) => (
          <div class="key-probe" data-state={r().valid ? "valid" : "invalid"}>
            <div>{r().message}</div>
            <Show when={!r().valid}>
              <div class="key-probe-action">
                You can press Retry after checking the key or trying again.
              </div>
            </Show>
            <Show
              when={
                r().rate_limit != null ||
                r().rate_remaining != null ||
                r().reset_seconds != null
              }
            >
              <div class="key-probe-meta">
                <Show when={r().rate_remaining != null}>
                  <span>
                    {r().rate_remaining}
                    <Show when={r().rate_limit != null}>
                      {" / "}
                      {r().rate_limit}
                    </Show>{" "}
                    remaining
                  </span>
                </Show>
                <Show when={fmtReset(r().reset_seconds)}>
                  <span>{fmtReset(r().reset_seconds)}</span>
                </Show>
                <Show when={r().status_code != null}>
                  <span>HTTP {r().status_code}</span>
                </Show>
              </div>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
};

export default SettingsTab;
