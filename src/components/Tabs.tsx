import { For, type Component } from "solid-js";
import "./Tabs.css";

export type TabDef = { id: string; label: string };

type Props = {
  active: string;
  onChange: (id: string) => void;
  tabs: TabDef[];
};

const Tabs: Component<Props> = (props) => {
  return (
    <div class="tabs" role="tablist">
      <For each={props.tabs}>
        {(tab) => (
          <button
            class="tab"
            classList={{ active: props.active === tab.id }}
            role="tab"
            aria-selected={props.active === tab.id}
            onClick={() => props.onChange(tab.id)}
          >
            {tab.label}
          </button>
        )}
      </For>
    </div>
  );
};

export default Tabs;
