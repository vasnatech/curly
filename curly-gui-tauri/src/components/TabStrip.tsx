import { centralViewEquals, extraTabKey, extraTabLabel, extraTabToCentralView, type CentralView, type ExtraTab } from "../centralView";
import { tabLabel, type RequestTabState } from "../tabState";

interface TabStripProps {
  tabs: RequestTabState[];
  activeTabIdx: number;
  centralView: CentralView;
  extraTabs: ExtraTab[];
  environments: string[];
  activeEnvironment: string | null;
  onSwitch: (idx: number) => void;
  onClose: (idx: number) => void;
  onNew: () => void;
  onFocusExtraTab: (tab: ExtraTab) => void;
  onCloseExtraTab: (tab: ExtraTab) => void;
  onOpenEnvironmentTab: () => void;
  onSelectActiveEnvironment: (name: string | null) => void;
}

export function TabStrip({
  tabs,
  activeTabIdx,
  centralView,
  extraTabs,
  environments,
  activeEnvironment,
  onSwitch,
  onClose,
  onNew,
  onFocusExtraTab,
  onCloseExtraTab,
  onOpenEnvironmentTab,
  onSelectActiveEnvironment,
}: TabStripProps) {
  const isRequestActive = centralView.kind === "request";
  return (
    <div className="tab-strip-row">
      <div className="tab-strip">
        {tabs.map((tab, i) => (
          <div
            key={tab.key}
            className={isRequestActive && i === activeTabIdx ? "tab active" : "tab"}
            onClick={() => onSwitch(i)}
          >
            <span className="tab-label">{tabLabel(tab)}</span>
            <button
              className="tab-close"
              onClick={(e) => {
                e.stopPropagation();
                onClose(i);
              }}
            >
              ✕
            </button>
          </div>
        ))}
        <button className="tab-new" onClick={onNew} title="New tab (Ctrl/Cmd+T)">
          +
        </button>

        {extraTabs.map((et) => (
          <div
            key={extraTabKey(et)}
            className={centralViewEquals(centralView, extraTabToCentralView(et)) ? "tab active" : "tab"}
            onClick={() => onFocusExtraTab(et)}
          >
            <span className="tab-label">{extraTabLabel(et)}</span>
            <button
              className="tab-close"
              onClick={(e) => {
                e.stopPropagation();
                onCloseExtraTab(et);
              }}
            >
              ✕
            </button>
          </div>
        ))}

        <div className={centralView.kind === "environment" ? "tab active" : "tab"} onClick={onOpenEnvironmentTab}>
          <span className="tab-label">Environment</span>
        </div>
      </div>

      <div className="active-env-selector">
        <span className="weak">Active environment:</span>
        <select
          value={activeEnvironment ?? ""}
          onChange={(e) => onSelectActiveEnvironment(e.target.value === "" ? null : e.target.value)}
        >
          <option value="">(none)</option>
          {environments.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      </div>
    </div>
  );
}
