// Persist zoom/theme/open tabs across restarts (M9 gap-fill — the egui GUI
// had this via persistence.rs writing to ~/.config/curly/gui_state.json;
// here `localStorage` is the direct equivalent — Tauri's webview keeps a
// real persistent profile per app, not an incognito one, so it survives
// restarts the same way a browser's own localStorage would). Wrapped in
// try/catch throughout: a restricted/ephemeral webview profile should
// degrade to "nothing persists," never crash the app.

import { tabFromPersisted, tabToPersisted, type PersistedTabData, type RequestTabState } from "./tabState";

const STORAGE_KEY = "curly-gui-prefs-v1";

export type ThemeMode = "system" | "light" | "dark";

export interface PersistedState {
  zoom: number;
  theme: ThemeMode;
  tabs: PersistedTabData[];
  activeTabIdx: number;
}

export function loadPersistedState(): PersistedState | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return null;
    return parsed as PersistedState;
  } catch {
    return null;
  }
}

export function savePersistedState(zoom: number, theme: ThemeMode, tabs: RequestTabState[], activeTabIdx: number): void {
  try {
    const state: PersistedState = {
      zoom,
      theme,
      tabs: tabs.map(tabToPersisted),
      activeTabIdx,
    };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {
    // best-effort only
  }
}

/** Rebuild tabs from a `PersistedState`, guarding `activeTabIdx` against
 * being out of range (e.g. the persisted file is hand-edited or from a
 * future version with more tabs than got saved). */
export function tabsFromPersisted(state: PersistedState): { tabs: RequestTabState[]; activeTabIdx: number } {
  const tabs = state.tabs.length > 0 ? state.tabs.map(tabFromPersisted) : [tabFromPersisted({ method: "GET", url: "", headers: [], body: "", auth: { kind: "none", username: "", password: "", token: "" }, loadedRequest: null })];
  const activeTabIdx = Math.min(Math.max(state.activeTabIdx, 0), tabs.length - 1);
  return { tabs, activeTabIdx };
}
