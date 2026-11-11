import {
  createSignal,
  onCleanup,
  onMount,
  Switch,
  Match,
  type Component,
} from "solid-js";
import { api, onCloseBlocked, type ActiveWork } from "./lib/api";
import "./App.css";
import "./styles/theme.css";
import Tabs from "./components/Tabs";
import SystemFooter from "./components/SystemFooter";
import ImagesTab from "./tabs/ImagesTab";
import SettingsTab from "./tabs/SettingsTab";
import LogTab from "./tabs/LogTab";
import APITab from "./tabs/APITab";

type TabId = "images" | "settings" | "log" | "api";

const TABS = [
  { id: "images", label: "Images" },
  { id: "settings", label: "Settings" },
  { id: "log", label: "Log" },
  { id: "api", label: "API" },
];

const App: Component = () => {
  const [active, setActive] = createSignal<TabId>("images");
  const [shuttingDown, setShuttingDown] = createSignal(false);

  // Apply the saved theme on startup (previously it was only applied when the
  // Settings dropdown changed, so a saved "light" theme didn't take effect
  // until the user re-selected it). Default to dark to match the window.
  document.documentElement.setAttribute("data-theme", "dark");
  onMount(async () => {
    try {
      const s = await api.getSettings();
      document.documentElement.setAttribute("data-theme", s.theme || "dark");
    } catch {
      // keep the dark default
    }
  });

  const quitQuestion = (work: ActiveWork) => {
    const parts: string[] = [];
    if (work.downloads > 0) parts.push("downloads");
    if (work.analyses > 0) parts.push("AI analysis");
    return `Media Buddy is still working (${parts.join(" and ")}). Quitting now stops it part-way: unfinished downloads are discarded and images not yet analyzed stay unanalyzed.\n\nQuit anyway?`;
  };

  const quitNow = () => {
    if (shuttingDown()) return;
    setShuttingDown(true);
    void api.shutdownApp().catch(() => {
      window.close();
    });
  };

  // Shutdown button: check for running work first.
  const shutdown = async () => {
    if (shuttingDown()) return;
    try {
      const work = await api.activeWork();
      if (work.downloads + work.analyses > 0 && !confirm(quitQuestion(work))) {
        return;
      }
    } catch {
      // If the check itself fails, don't block quitting.
    }
    quitNow();
  };

  // Window close button: the backend holds the window open and sends this
  // event when work is running. Acknowledge first so it knows the question
  // reached the user; if it never hears back, a second close forces a quit.
  onMount(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onCloseBlocked((work) => {
      void api.closeWarningAck().catch(() => {});
      if (confirm(quitQuestion(work))) quitNow();
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    onCleanup(() => {
      disposed = true;
      unlisten?.();
    });
  });

  return (
    <div class="app">
      <Tabs
        active={active()}
        onChange={(id) => setActive(id as TabId)}
        tabs={TABS}
        actions={
          <button
            type="button"
            class="shutdown-button"
            onClick={() => void shutdown()}
            disabled={shuttingDown()}
            title="Shut down Media Buddy"
          >
            {shuttingDown() ? "Shutting down" : "Shutdown"}
          </button>
        }
      />
      <main class="tab-panel">
        <div class="tab-panel-inner">
          <Switch>
            <Match when={active() === "images"}>
              <ImagesTab />
            </Match>
            <Match when={active() === "settings"}>
              <SettingsTab />
            </Match>
            <Match when={active() === "log"}>
              <LogTab />
            </Match>
            <Match when={active() === "api"}>
              <APITab />
            </Match>
          </Switch>
        </div>
      </main>
      <SystemFooter />
    </div>
  );
};

export default App;
