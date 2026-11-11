// Per-tab editor state and its conversions to/from the wire (SavedAuth /
// SavedRequest). Unlike the egui GUI's `RequestTab` (which needed a
// take/restore swap trick because egui redraws everything from scratch
// every frame — see crates/curly-gui/src/app.rs's own doc comment on that),
// a tab here is just an object in a React state array: "the active tab" is
// simply `tabs[activeTabIdx]`, no swap machinery needed at all — and for
// the same reason, `breadcrumbDraft` below needs none of the reactive
// resync trick the egui GUI's single global field needed either: it's
// already a field on this tab's own object, so switching tabs and back
// finds it untouched for free.

import type { Extraction, HistoryEntry, KvPair, SavedAuth, SavedRequest } from "./types";

/** Matches `curly_core::storage::REDACTED` (NFR-4) — a history entry's
 * credential-looking headers are stored as this literal placeholder, never
 * the real value, so it must be filtered out on reload rather than loaded
 * back as if it were real. */
const REDACTED = "***redacted***";

export interface AuthEditorState {
  kind: "none" | "basic" | "bearer";
  username: string;
  password: string;
  token: string;
}

export function blankAuth(): AuthEditorState {
  return { kind: "none", username: "", password: "", token: "" };
}

export function authEditorToSaved(auth: AuthEditorState): SavedAuth | null {
  switch (auth.kind) {
    case "basic":
      return { type: "basic", username: auth.username, password: auth.password };
    case "bearer":
      return { type: "bearer", token: auth.token };
    default:
      return null;
  }
}

export function savedAuthToEditor(auth: SavedAuth | null): AuthEditorState {
  if (!auth) return blankAuth();
  if (auth.type === "basic") {
    return { kind: "basic", username: auth.username, password: auth.password, token: "" };
  }
  return { kind: "bearer", username: "", password: "", token: auth.token };
}

export interface LoadedRequestRef {
  collection: string;
  path: string;
}

/** Everything that makes a request "unsaved" — captured whenever a tab's
 * content is known to match disk (right after loading or saving) and
 * compared against the live fields to decide whether the tab needs a "*".
 * Deliberately narrower than `RequestTabState` itself: no response/
 * in-flight/breadcrumb-draft fields, none of which affect whether there's
 * unsaved *request* content. Mirrors the egui GUI's `EditableSnapshot`. */
export interface EditableSnapshot {
  method: string;
  url: string;
  headers: KvPair[];
  body: string;
  auth: AuthEditorState;
  extractRules: Extraction[];
}

function snapshotOf(fields: {
  method: string;
  url: string;
  headers: KvPair[];
  body: string;
  auth: AuthEditorState;
  extractRules: Extraction[];
}): EditableSnapshot {
  return {
    method: fields.method,
    url: fields.url,
    headers: fields.headers,
    body: fields.body,
    auth: fields.auth,
    extractRules: fields.extractRules,
  };
}

function blankSnapshot(): EditableSnapshot {
  return { method: "GET", url: "", headers: [emptyHeader()], body: "", auth: blankAuth(), extractRules: [] };
}

export interface RequestTabState {
  /** Client-only React key — never sent to Rust, never persisted. */
  key: string;
  method: string;
  url: string;
  headers: KvPair[];
  body: string;
  auth: AuthEditorState;
  extractRules: Extraction[];
  /** Ad-hoc `{{variable}}` overrides for this tab's own sends only — the
   * GUI's equivalent of `curly run --var key=value`. Highest precedence in
   * `send_request`'s variable merge. Never saved anywhere (not part of
   * `EditableSnapshot`/dirty-tracking either, matching the egui GUI: these
   * are editor state, not request content). */
  overrides: KvPair[];
  loadedRequest: LoadedRequestRef | null;
  /** The breadcrumb's editable trailing segment — the loaded request's
   * name, editable independent of the rest of the request; saving it
   * renames the request. Empty when nothing's loaded. */
  breadcrumbDraft: string;
  /** The editor content as of the last load or save — `null` means "never
   * saved anywhere" (a history re-run, or a load/save that hasn't happened
   * yet), which `isTabDirty` always treats as dirty. */
  savedSnapshot: EditableSnapshot | null;
  response: import("./types").ResponseSummary | null;
  responseError: string | null;
  inFlight: boolean;
  /** Set when a response (or error) lands for this tab while it wasn't the
   * active one — drives a "response ready" marker on the tab strip.
   * Cleared when the tab becomes active. */
  hasUnseenResponse: boolean;
}

export function emptyHeader(): KvPair {
  return { name: "", value: "", enabled: true };
}

let nextTabKey = 1;

export function blankTab(): RequestTabState {
  return {
    key: `tab-${nextTabKey++}`,
    method: "GET",
    url: "",
    headers: [emptyHeader()],
    body: "",
    auth: blankAuth(),
    extractRules: [],
    overrides: [],
    loadedRequest: null,
    breadcrumbDraft: "",
    savedSnapshot: blankSnapshot(),
    response: null,
    responseError: null,
    inFlight: false,
    hasUnseenResponse: false,
  };
}

function splitFolderAndName(path: string): [string, string] {
  const idx = path.lastIndexOf("/");
  if (idx === -1) return ["", path];
  return [path.slice(0, idx), path.slice(idx + 1)];
}

export function tabFromSavedRequest(saved: SavedRequest, loadedRequest: LoadedRequestRef): RequestTabState {
  const body = saved.body && saved.body.type === "raw" ? saved.body.content : "";
  const headers = saved.headers.length > 0 ? saved.headers : [emptyHeader()];
  const auth = savedAuthToEditor(saved.auth);
  const [, name] = splitFolderAndName(loadedRequest.path);
  return {
    key: `tab-${nextTabKey++}`,
    method: saved.method,
    url: saved.url,
    headers,
    body,
    auth,
    extractRules: saved.extract,
    overrides: [],
    loadedRequest,
    breadcrumbDraft: name,
    savedSnapshot: snapshotOf({ method: saved.method, url: saved.url, headers, body, auth, extractRules: saved.extract }),
    response: null,
    responseError: null,
    inFlight: false,
    hasUnseenResponse: false,
  };
}

/** Build the `SavedRequest` a tab's current editor fields describe — used
 * both for sending (via `send_request`) and saving. `id`/`name` are
 * placeholders the caller overwrites (send doesn't care about either;
 * saving fills `id` in from the existing request via `withRequestReplaced`,
 * or lets the server assign one fresh via `addRequest`). */
export function tabToSavedRequest(tab: RequestTabState): SavedRequest {
  return {
    id: crypto.randomUUID(),
    name: "",
    method: tab.method,
    url: tab.url,
    query_params: [],
    headers: tab.headers.filter((h) => h.name.trim() !== ""),
    body: tab.body.trim() === "" ? null : { type: "raw", content: tab.body },
    auth: authEditorToSaved(tab.auth),
    extract: tab.extractRules,
  };
}

/** What a tab looks like immediately after a successful save (plain Save,
 * Save As, or a breadcrumb rename) — call this to build the patch applied
 * to the tab afterward, so its dirty flag clears and (for Save/Save As)
 * `loadedRequest` points at the new location. */
export function markSaved(tab: RequestTabState, loadedRequest: LoadedRequestRef): Partial<RequestTabState> {
  const [, name] = splitFolderAndName(loadedRequest.path);
  return {
    loadedRequest,
    breadcrumbDraft: name,
    savedSnapshot: snapshotOf(tab),
  };
}

/** Reload a history entry back into the editor for a re-run (FR-6) —
 * mirrors the egui GUI's `load_history_entry`: a history entry is already
 * fully resolved (no `{{variable}}` templates), redacted headers are
 * dropped rather than reloaded as literal "***redacted***" text, and only
 * *which kind* of auth was used comes back (never real credentials — see
 * NFR-4) so username/password/token are left blank for the user to
 * re-enter. Not tied to any saved collection entry, so `loadedRequest`
 * stays `null` — a later Save falls back to Save As, same as any other
 * from-scratch request. `savedSnapshot` is `null` too, deliberately —
 * there's nothing on disk this matches, so it shows as needing a save. */
export function tabFromHistoryEntry(entry: HistoryEntry): RequestTabState {
  const headers = entry.request_headers
    .filter(([, v]) => v !== REDACTED)
    .map(([name, value]) => ({ name, value, enabled: true }));
  return {
    key: `tab-${nextTabKey++}`,
    method: entry.method,
    url: entry.url,
    headers: headers.length > 0 ? headers : [emptyHeader()],
    body: entry.request_body ?? "",
    auth: entry.request_auth_kind
      ? { kind: entry.request_auth_kind === "Basic" ? "basic" : "bearer", username: "", password: "", token: "" }
      : blankAuth(),
    extractRules: [],
    overrides: [],
    loadedRequest: null,
    breadcrumbDraft: "",
    savedSnapshot: null,
    response: null,
    responseError: null,
    inFlight: false,
    hasUnseenResponse: false,
  };
}

/** Whether `tab` has unsaved changes — derived, never stored, so it can't
 * drift out of sync with the actual draft (mirrors the egui GUI's
 * `is_tab_dirty`). */
export function isTabDirty(tab: RequestTabState): boolean {
  if (!tab.savedSnapshot) return true;
  const current = snapshotOf(tab);
  return JSON.stringify(tab.savedSnapshot) !== JSON.stringify(current);
}

export function tabLabel(tab: RequestTabState): string {
  const base = tab.loadedRequest
    ? tab.loadedRequest.path
    : tab.url.trim() !== ""
      ? `${tab.method} ${tab.url}`
      : "New Request";
  return isTabDirty(tab) ? `* ${base}` : base;
}

/** What gets written to `localStorage` for a tab (M9 gap-fill —
 * persistence across restarts, mirroring the egui GUI's own
 * `persistence.rs::PersistedTab`). Deliberately narrow: no response/
 * search/extraction-rules/overrides — those reset to defaults on restart,
 * matching the same "ephemeral editor state, not saved" stance the egui
 * GUI already established for this exact set of fields. */
export interface PersistedTabData {
  method: string;
  url: string;
  headers: KvPair[];
  body: string;
  auth: AuthEditorState;
  loadedRequest: LoadedRequestRef | null;
}

export function tabToPersisted(tab: RequestTabState): PersistedTabData {
  return {
    method: tab.method,
    url: tab.url,
    headers: tab.headers,
    body: tab.body,
    auth: tab.auth,
    loadedRequest: tab.loadedRequest,
  };
}

/** Rebuild a tab from persisted data — treated as freshly "clean" against
 * its own restored content (there's no cheap way to know whether it still
 * matches whatever's on disk without re-fetching, so restoring is its own
 * new baseline, same as a fresh load). */
export function tabFromPersisted(p: PersistedTabData): RequestTabState {
  const headers = p.headers.length > 0 ? p.headers : [emptyHeader()];
  const name = p.loadedRequest ? splitFolderAndName(p.loadedRequest.path)[1] : "";
  return {
    key: `tab-${nextTabKey++}`,
    method: p.method,
    url: p.url,
    headers,
    body: p.body,
    auth: p.auth,
    extractRules: [],
    overrides: [],
    loadedRequest: p.loadedRequest,
    breadcrumbDraft: name,
    savedSnapshot: snapshotOf({ method: p.method, url: p.url, headers, body: p.body, auth: p.auth, extractRules: [] }),
    response: null,
    responseError: null,
    inFlight: false,
    hasUnseenResponse: false,
  };
}
