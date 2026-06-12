import { For, type Component, type JSX } from "solid-js";
import "./Tabs.css";

export type TabDef = { id: string; label: string };

type Props = {
  active: string;
  onChange: (id: string) => void;
  tabs: TabDef[];
  actions?: JSX.Element;
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
      <div class="tabs-spacer" aria-hidden="true" />
      {props.actions}
    </div>
  );
};

export default Tabs;
