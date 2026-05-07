import { createSignal, Switch, Match, type Component } from "solid-js";
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

  return (
    <div class="app">
      <Tabs
        active={active()}
        onChange={(id) => setActive(id as TabId)}
        tabs={TABS}
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
