import { createSignal, onMount, Show, type Component } from "solid-js";
import { api, type Settings } from "../lib/api";
import "./SettingsTab.css";

const SettingsTab: Component = () => {
  const [settings, setSettings] = createSignal<Settings | null>(null);
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal<string | null>(null);
  const [saved, setSaved] = createSignal(false);
  const [revealKeys, setRevealKeys] = createSignal(false);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      setSettings(await api.getSettings());
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
    setError(null);
    try {
      await api.saveSettings(s);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div class="settings">
      <header class="settings-header">
        <div>
          <h2>Settings</h2>
          <p class="hint">
            API keys, REST server config, and theme. Changes apply on save; the
            REST server picks up new host/port on its next start.
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
        fallback={<div class="placeholder-line">Loading…</div>}
      >
        {(s) => (
          <div class="settings-grid">
            <Section title="Stock image API keys">
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
              <Field label="Pixabay key">
                <input
                  type={revealKeys() ? "text" : "password"}
                  value={s().pixabay_key}
                  onInput={(e) => update("pixabay_key", e.currentTarget.value)}
                  placeholder="33929247-..."
                />
              </Field>
              <Field label="Pexels key">
                <input
                  type={revealKeys() ? "text" : "password"}
                  value={s().pexels_key}
                  onInput={(e) => update("pexels_key", e.currentTarget.value)}
                />
              </Field>
              <Field label="Unsplash key">
                <input
                  type={revealKeys() ? "text" : "password"}
                  value={s().unsplash_key}
                  onInput={(e) => update("unsplash_key", e.currentTarget.value)}
                />
              </Field>
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
                </select>
              </Field>
            </Section>

            <Section title="REST API server">
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
                  Allow requests from any origin
                </label>
              </Field>
            </Section>

            <Section title="AI vision (Florence-2)">
              <p class="hint">
                Vision/captioning is not yet enabled in this Rust build —
                Florence-2 ONNX integration ships in the next phase. The
                settings below will become active then.
              </p>
              <Field label="Auto-load on first use">
                <label class="checkbox">
                  <input
                    type="checkbox"
                    checked={s().vision_auto_load}
                    onChange={(e) =>
                      update("vision_auto_load", e.currentTarget.checked)
                    }
                  />
                  Load Florence-2 the first time it's needed
                </label>
              </Field>
              <Field label="Auto-unload when idle">
                <label class="checkbox">
                  <input
                    type="checkbox"
                    checked={s().vision_auto_unload}
                    onChange={(e) =>
                      update("vision_auto_unload", e.currentTarget.checked)
                    }
                  />
                  Free GPU memory when no analysis is queued
                </label>
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
                  Run inference on CPU if no compatible GPU is available
                </label>
              </Field>
              <Field label="Max instances per GPU">
                <input
                  type="number"
                  min="1"
                  max="16"
                  value={s().vision_max_per_gpu}
                  onInput={(e) =>
                    update("vision_max_per_gpu", Number(e.currentTarget.value) || 4)
                  }
                />
              </Field>
              <Field label="Max instances total">
                <input
                  type="number"
                  min="1"
                  max="32"
                  value={s().vision_max_total}
                  onInput={(e) =>
                    update("vision_max_total", Number(e.currentTarget.value) || 8)
                  }
                />
              </Field>
              <Field label="Reserved VRAM (GB)">
                <input
                  type="number"
                  min="0"
                  step="0.1"
                  value={s().vision_reserved_vram}
                  onInput={(e) =>
                    update(
                      "vision_reserved_vram",
                      Number(e.currentTarget.value) || 0.5
                    )
                  }
                />
              </Field>
            </Section>
          </div>
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

const Field: Component<{ label: string; children: any }> = (props) => (
  <label class="field">
    <span class="field-label">{props.label}</span>
    <span class="field-input">{props.children}</span>
  </label>
);

export default SettingsTab;
