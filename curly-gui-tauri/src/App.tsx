import { useEffect, useRef, useState } from "react";
import * as api from "./api";
import { centralViewEquals, extraTabKey, extraTabToCentralView, type CentralView, type ExtraTab } from "./centralView";
import { withRequestReplaced } from "./collectionTree";
import { CollectionTabView } from "./components/CollectionTabView";
import { EnvironmentTabView } from "./components/EnvironmentTabView";
import { FolderDialog, type FolderDialogState } from "./components/FolderDialog";
import { FolderTabView } from "./components/FolderTabView";
import { OpenProjectDialog, type OpenProjectDialogState } from "./components/OpenProjectDialog";
import { PathDialog, type PathDialogState } from "./components/PathDialog";
import { RequestEditor } from "./components/RequestEditor";
import { ResponsePane } from "./components/ResponsePane";
import { SaveAsDialog, type SaveAsDialogState } from "./components/SaveAsDialog";
import { Sidebar } from "./components/Sidebar";
import { TabStrip } from "./components/TabStrip";
import { Toolbar } from "./components/Toolbar";
import { loadPersistedState, savePersistedState, tabsFromPersisted, type ThemeMode } from "./persistence";
import { blankTab, isTabDirty, markSaved, tabFromHistoryEntry, tabFromSavedRequest, tabToSavedRequest } from "./tabState";
import type { RequestTabState } from "./tabState";
import type { HistoryEntry, ProjectInfo, SavedRequest } from "./types";
import "./App.css";

const MIN_ZOOM = 0.5;
const MAX_ZOOM = 3;
const ZOOM_STEP = 1.1;

// Third slice of the Tauri+React rewrite: Collection/Folder/Environment
// main tabs (with environment create/edit/delete, previously entirely
// missing from this rewrite) and the tab strip's "Active environment"
// selector, on top of the second slice's dirty tracking, breadcrumb
// rename, and hover "⋯" menus. Still deferred: response search/JSON-
// syntax-highlighting/History sub-tab, ad-hoc variable overrides, and the
// extraction-rule editor.

function splitFolderAndName(path: string): [string, string] {
  const idx = path.lastIndexOf("/");
  if (idx === -1) return ["", path];
  return [path.slice(0, idx), path.slice(idx + 1)];
}

export default function App() {
  const [project, setProject] = useState<ProjectInfo | null>(null);
  const [collections, setCollections] = useState<string[]>([]);
  const [environments, setEnvironments] = useState<string[]>([]);
  const [activeEnvironment, setActiveEnvironment] = useState<string | null>(null);
  const [sidebarTab, setSidebarTab] = useState<"project" | "history">("project");
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  // Restored from localStorage on first render if present (M9 gap-fill —
  // the egui GUI's own zoom/theme/open-tabs persistence, see
  // persistence.ts). `useState`'s lazy initializer runs exactly once, so
  // this is cheap even though it means a redundant `loadPersistedState()`
  // call across a few `useState`s below.
  const [tabs, setTabs] = useState<RequestTabState[]>(() => {
    const persisted = loadPersistedState();
    return persisted ? tabsFromPersisted(persisted).tabs : [blankTab()];
  });
  const [activeTabIdx, setActiveTabIdx] = useState(() => {
    const persisted = loadPersistedState();
    return persisted ? tabsFromPersisted(persisted).activeTabIdx : 0;
  });
  const [zoom, setZoom] = useState(() => loadPersistedState()?.zoom ?? 1);
  const [theme, setTheme] = useState<ThemeMode>(() => loadPersistedState()?.theme ?? "system");
  const [notice, setNotice] = useState<string | null>(null);
  const [saveAsDialog, setSaveAsDialog] = useState<SaveAsDialogState | null>(null);
  const [pathDialog, setPathDialog] = useState<PathDialogState | null>(null);
  const [openProjectDialog, setOpenProjectDialog] = useState<OpenProjectDialogState | null>(null);
  const [folderDialog, setFolderDialog] = useState<FolderDialogState | null>(null);
  const [collectionsVersion, setCollectionsVersion] = useState(0);
  const [environmentsVersion, setEnvironmentsVersion] = useState(0);
  const [centralView, setCentralView] = useState<CentralView>({ kind: "request" });
  const [extraTabs, setExtraTabs] = useState<ExtraTab[]>([]);

  const activeTab = tabs[activeTabIdx];

  // Read inside async closures (e.g. `onSend`, below) that must know which
  // tab is *currently* active when a response lands, not which one was
  // active when the send started — `activeTabIdx` itself would be a stale
  // value captured at that earlier point.
  const activeTabIdxRef = useRef(activeTabIdx);
  useEffect(() => {
    activeTabIdxRef.current = activeTabIdx;
  }, [activeTabIdx]);

  function bumpCollectionsVersion() {
    setCollectionsVersion((v) => v + 1);
  }

  function refreshHistoryIfShown() {
    if (sidebarTab === "history") {
      api.listHistory(50).then(setHistory).catch(() => {});
    }
  }

  function updateTabAt(idx: number, patch: Partial<RequestTabState>) {
    setTabs((prev) => prev.map((t, i) => (i === idx ? { ...t, ...patch } : t)));
  }

  // Manual light/dark override — `null`/"system" defers entirely to
  // `prefers-color-scheme` (see App.css); explicit here wins over it in
  // both directions via the `data-theme` attribute.
  useEffect(() => {
    if (theme === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.setAttribute("data-theme", theme);
  }, [theme]);

  // The non-standard `zoom` CSS property (supported by both Chromium and
  // WebKitGTK, so it works in Tauri's Linux webview) scales the whole
  // page's layout and paint uniformly, including fixed-position dialogs —
  // unlike `transform: scale`, which doesn't reflow layout and behaves
  // oddly with fixed-position descendants' own coordinate systems.
  useEffect(() => {
    (document.documentElement.style as unknown as { zoom: string }).zoom = String(zoom);
  }, [zoom]);

  // Debounced (so typing doesn't hammer localStorage on every keystroke)
  // plus a `beforeunload` save as a safety net for a clean app close —
  // mirrors the egui GUI's own on-exit persistence, just via two triggers
  // instead of one real "on_exit" hook.
  useEffect(() => {
    const id = window.setTimeout(() => {
      savePersistedState(zoom, theme, tabs, activeTabIdx);
    }, 500);
    return () => window.clearTimeout(id);
  }, [zoom, theme, tabs, activeTabIdx]);

  useEffect(() => {
    const handler = () => savePersistedState(zoom, theme, tabs, activeTabIdx);
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [zoom, theme, tabs, activeTabIdx]);

  function isEffectivelyDark(): boolean {
    if (theme === "dark") return true;
    if (theme === "light") return false;
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  }

  function toggleTheme() {
    setTheme(isEffectivelyDark() ? "light" : "dark");
  }

  function zoomIn() {
    setZoom((z) => Math.min(MAX_ZOOM, z * ZOOM_STEP));
  }

  function zoomOut() {
    setZoom((z) => Math.max(MIN_ZOOM, z / ZOOM_STEP));
  }

  function zoomReset() {
    setZoom(1);
  }

  useEffect(() => {
    (async () => {
      try {
        const info = await api.resolveDefaultProject();
        setProject(info);
        setCollections(await api.listCollections());
        setEnvironments(await api.listEnvironments());
      } catch (e) {
        setNotice(String(e));
      }
    })();
  }, []);

  useEffect(() => {
    if (sidebarTab !== "history") return;
    api
      .listHistory(50)
      .then(setHistory)
      .catch((e) => setNotice(String(e)));
  }, [sidebarTab]);

  /** Native folder picker → `open_project` — the one piece missing from
   * the initial rewrite (it only ever auto-resolved a project at
   * startup). Picking a directory that already has a `.curly` opens it;
   * picking an empty one creates one on the spot (same as `curly init`),
   * matching the egui GUI's own "Open Project…" behavior. Resets
   * environment selection and any open Collection/Folder/Environment
   * tabs, since they reference the *previous* project's data — but
   * leaves request tabs alone, so in-progress edits aren't lost just for
   * switching projects. */
  // **Asks for a path rather than opening a picker**, which is the one thing that genuinely changed
  // when the Tauri shell went. A browser will not hand a page a filesystem path -- the File System
  // Access API answers an opaque handle on purpose -- and `open_project` needs a path because the
  // server is what opens it. The server runs on the same machine as the files, so typing one is
  // less convenient and no less capable.
  function onOpenProject() {
    setOpenProjectDialog({ path: project?.root ?? "", error: null });
  }

  async function confirmOpenProject() {
    if (!openProjectDialog) return;
    const picked = openProjectDialog.path.trim();
    if (!picked) {
      setOpenProjectDialog({ ...openProjectDialog, error: "a path is required" });
      return;
    }
    try {
      const info = await api.openProject(picked);
      setProject(info);
      setCollections(await api.listCollections());
      setEnvironments(await api.listEnvironments());
      setActiveEnvironment(null);
      setExtraTabs([]);
      setCentralView({ kind: "request" });
      bumpCollectionsVersion();
      setEnvironmentsVersion((v) => v + 1);
      setOpenProjectDialog(null);
      setNotice(`opened project "${info.label}"`);
    } catch (e) {
      setOpenProjectDialog({ ...openProjectDialog, error: `${e}` });
    }
  }

  function updateActiveTab(patch: Partial<RequestTabState>) {
    updateTabAt(activeTabIdx, patch);
  }

  function openNewTab() {
    setTabs((prev) => [...prev, blankTab()]);
    setActiveTabIdx(tabs.length);
    setCentralView({ kind: "request" });
  }

  function closeTab(idx: number) {
    if (tabs.length === 1) {
      setTabs([blankTab()]);
      setActiveTabIdx(0);
      return;
    }
    setTabs((prev) => prev.filter((_, i) => i !== idx));
    setActiveTabIdx((prevIdx) => {
      if (idx === prevIdx) return Math.min(idx, tabs.length - 2);
      if (idx < prevIdx) return prevIdx - 1;
      return prevIdx;
    });
  }

  function onSwitchTab(idx: number) {
    setActiveTabIdx(idx);
    setCentralView({ kind: "request" });
    updateTabAt(idx, { hasUnseenResponse: false });
  }

  function openOrFocusExtraTab(tab: ExtraTab) {
    setExtraTabs((prev) => (prev.some((t) => extraTabKey(t) === extraTabKey(tab)) ? prev : [...prev, tab]));
    setCentralView(extraTabToCentralView(tab));
  }

  function onOpenCollectionTab(collection: string) {
    openOrFocusExtraTab({ kind: "collection", name: collection });
  }

  function onOpenFolderTab(collection: string, path: string) {
    openOrFocusExtraTab({ kind: "folder", collection, path });
  }

  function onCloseExtraTab(tab: ExtraTab) {
    setExtraTabs((prev) => prev.filter((t) => extraTabKey(t) !== extraTabKey(tab)));
    setCentralView((cv) => (centralViewEquals(cv, extraTabToCentralView(tab)) ? { kind: "request" } : cv));
  }

  function onLoadRequest(collection: string, path: string, saved: SavedRequest) {
    updateActiveTab(tabFromSavedRequest(saved, { collection, path }));
    setCentralView({ kind: "request" });
  }

  function onLoadHistoryEntry(entry: HistoryEntry) {
    updateActiveTab(tabFromHistoryEntry(entry));
    setCentralView({ kind: "request" });
  }

  /** Sends from whichever tab was active when Send was clicked (`tabIdx`,
   * captured up front) — not necessarily whatever's active by the time the
   * response lands, since the user may have switched tabs while it was in
   * flight. If they have, the response still lands on the *sending* tab
   * (never bleeds into whatever you switched to), and that tab gets
   * `hasUnseenResponse` for the tab strip's "response ready" marker
   * instead of a history refresh/extraction notice, which only make sense
   * for whatever's actually on screen right now. */
  async function onSend() {
    const tabIdx = activeTabIdx;
    const tabAtSend = tabs[tabIdx];
    updateTabAt(tabIdx, { inFlight: true, responseError: null });
    try {
      const saved = tabToSavedRequest(tabAtSend);
      const overrides: [string, string][] = tabAtSend.overrides
        .filter((o) => o.enabled && o.name.trim() !== "")
        .map((o) => [o.name, o.value]);
      const outcome = await api.sendRequest(saved, activeEnvironment, overrides);
      const stillActive = activeTabIdxRef.current === tabIdx;
      updateTabAt(tabIdx, { response: outcome.response, inFlight: false, hasUnseenResponse: !stillActive });
      if (stillActive) {
        refreshHistoryIfShown();
        if (outcome.extracted.length > 0) {
          setNotice(`extracted ${outcome.extracted.map((e) => e.name).join(", ")}`);
        }
      }
    } catch (e) {
      const stillActive = activeTabIdxRef.current === tabIdx;
      updateTabAt(tabIdx, { inFlight: false, responseError: String(e), response: null, hasUnseenResponse: !stillActive });
    }
  }

  async function onSave() {
    if (!activeTab.loadedRequest) {
      onSaveAs();
      return;
    }
    try {
      const { collection, path } = activeTab.loadedRequest;
      const coll = await api.loadCollection(collection);
      const updated = tabToSavedRequest(activeTab);
      const next = withRequestReplaced(coll, path, updated);
      await api.saveCollection(next);
      updateActiveTab(markSaved(activeTab, { collection, path }));
      setNotice(`saved "${path}" in "${collection}"`);
      bumpCollectionsVersion();
    } catch (e) {
      setNotice(`failed to save: ${e}`);
    }
  }

  function onSaveAs() {
    const loaded = activeTab.loadedRequest;
    const [folder, name] = loaded ? splitFolderAndName(loaded.path) : ["", ""];
    setSaveAsDialog({
      collection: loaded?.collection ?? collections[0] ?? "",
      folder,
      name,
      error: null,
    });
  }

  async function confirmSaveAs() {
    if (!saveAsDialog) return;
    const collection = saveAsDialog.collection.trim();
    const name = saveAsDialog.name.trim();
    const folder = saveAsDialog.folder.trim().replace(/^\/+|\/+$/g, "");
    if (collection === "") {
      setSaveAsDialog({ ...saveAsDialog, error: "collection name is required" });
      return;
    }
    if (name === "") {
      setSaveAsDialog({ ...saveAsDialog, error: "request name is required" });
      return;
    }
    const path = folder ? `${folder}/${name}` : name;
    try {
      await api.addRequest(collection, path, tabToSavedRequest(activeTab));
      updateActiveTab(markSaved(activeTab, { collection, path }));
      setCollections(await api.listCollections());
      setNotice(`saved "${path}" in "${collection}"`);
      setSaveAsDialog(null);
      bumpCollectionsVersion();
    } catch (e) {
      setSaveAsDialog({ ...saveAsDialog, error: String(e) });
    }
  }

  async function onRenameBreadcrumb() {
    if (!activeTab.loadedRequest) return;
    const newName = activeTab.breadcrumbDraft.trim();
    if (newName === "") {
      setNotice("name is required");
      return;
    }
    const { collection, path } = activeTab.loadedRequest;
    const [folder] = splitFolderAndName(path);
    const newPath = folder ? `${folder}/${newName}` : newName;
    if (newPath === path) return;
    try {
      await api.renameOrMoveRequest(collection, path, newPath);
      // Only the loaded-request pointer + breadcrumb move — content dirty
      // state is untouched (a rename doesn't save content edits, matching
      // the egui GUI's `save_breadcrumb_name`/`rename_saved_request` split).
      updateActiveTab({ loadedRequest: { collection, path: newPath }, breadcrumbDraft: newName });
      setNotice(`renamed "${path}" to "${newPath}"`);
      bumpCollectionsVersion();
    } catch (e) {
      setNotice(`failed to rename: ${e}`);
    }
  }

  function onRenameRequest(collection: string, path: string) {
    const [folder, name] = splitFolderAndName(path);
    setPathDialog({ mode: "rename", collection, oldPath: path, folder, name, error: null });
  }

  function onDuplicateRequest(collection: string, path: string) {
    const [folder, name] = splitFolderAndName(path);
    setPathDialog({ mode: "duplicate", collection, oldPath: path, folder, name: `${name}-copy`, error: null });
  }

  async function confirmPathDialog() {
    if (!pathDialog) return;
    const name = pathDialog.name.trim();
    if (name === "") {
      setPathDialog({ ...pathDialog, error: "name is required" });
      return;
    }
    const folder = pathDialog.folder.trim().replace(/^\/+|\/+$/g, "");
    const newPath = folder ? `${folder}/${name}` : name;
    try {
      if (pathDialog.mode === "rename") {
        await api.renameOrMoveRequest(pathDialog.collection, pathDialog.oldPath, newPath);
        setTabs((prev) =>
          prev.map((t) =>
            t.loadedRequest?.collection === pathDialog.collection && t.loadedRequest?.path === pathDialog.oldPath
              ? { ...t, loadedRequest: { collection: pathDialog.collection, path: newPath }, breadcrumbDraft: name }
              : t,
          ),
        );
        setNotice(`renamed "${pathDialog.oldPath}" to "${newPath}"`);
      } else {
        await api.duplicateRequest(pathDialog.collection, pathDialog.oldPath, newPath);
        setNotice(`duplicated "${pathDialog.oldPath}" to "${newPath}"`);
      }
      bumpCollectionsVersion();
      setPathDialog(null);
    } catch (e) {
      setPathDialog({ ...pathDialog, error: String(e) });
    }
  }

  async function onDeleteRequest(collection: string, path: string) {
    try {
      await api.removeRequest(collection, path);
      setTabs((prev) =>
        prev.map((t) =>
          t.loadedRequest?.collection === collection && t.loadedRequest?.path === path
            ? { ...t, loadedRequest: null, breadcrumbDraft: "" }
            : t,
        ),
      );
      setNotice(`deleted "${path}" from "${collection}"`);
      bumpCollectionsVersion();
    } catch (e) {
      setNotice(`failed to delete "${path}": ${e}`);
    }
  }

  function onNewFolder(collection: string, parentPath: string) {
    setFolderDialog({ collection, parentPath, name: "", error: null });
  }

  async function confirmFolderDialog() {
    if (!folderDialog) return;
    const name = folderDialog.name.trim().replace(/^\/+|\/+$/g, "");
    if (name === "") {
      setFolderDialog({ ...folderDialog, error: "name is required" });
      return;
    }
    const path = folderDialog.parentPath ? `${folderDialog.parentPath}/${name}` : name;
    try {
      await api.addFolder(folderDialog.collection, path);
      setNotice(`created folder "${path}" in "${folderDialog.collection}"`);
      setCollections(await api.listCollections());
      bumpCollectionsVersion();
      setFolderDialog(null);
    } catch (e) {
      setFolderDialog({ ...folderDialog, error: String(e) });
    }
  }

  function onNewRequest(collection: string, parentPath: string) {
    openNewTab();
    setSaveAsDialog({ collection, folder: parentPath, name: "", error: null });
  }

  async function onDeleteFolder(collection: string, path: string) {
    try {
      await api.removeFolder(collection, path, false);
      setNotice(`deleted folder "${path}" from "${collection}"`);
      bumpCollectionsVersion();
    } catch (e) {
      setNotice(`failed to delete "${path}": ${e}`);
    }
  }

  async function onSaveAllCollection(collection: string) {
    const dirty = tabs
      .map((t, i) => ({ t, i }))
      .filter(({ t }) => t.loadedRequest?.collection === collection && isTabDirty(t));
    if (dirty.length === 0) {
      setNotice(`nothing to save in "${collection}"`);
      return;
    }
    try {
      let coll = await api.loadCollection(collection);
      for (const { t } of dirty) {
        coll = withRequestReplaced(coll, t.loadedRequest!.path, tabToSavedRequest(t));
      }
      await api.saveCollection(coll);
      setTabs((prev) =>
        prev.map((t, i) => {
          const match = dirty.find((d) => d.i === i);
          return match ? { ...t, ...markSaved(t, t.loadedRequest!) } : t;
        }),
      );
      setNotice(`saved ${dirty.length} request(s) in "${collection}"`);
      bumpCollectionsVersion();
    } catch (e) {
      setNotice(`failed to save all: ${e}`);
    }
  }

  async function onCreateEnvironment(name: string) {
    try {
      await api.saveEnvironment({ name, variables: [] });
      setEnvironments(await api.listEnvironments());
      setActiveEnvironment(name);
      setCentralView({ kind: "environment" });
      setEnvironmentsVersion((v) => v + 1);
      setNotice(`created environment "${name}"`);
    } catch (e) {
      setNotice(`failed to create environment: ${e}`);
    }
  }

  async function onEnvironmentDeleted() {
    setEnvironments(await api.listEnvironments());
    setActiveEnvironment(null);
    setEnvironmentsVersion((v) => v + 1);
  }

  return (
    <div className="app-root">
      <Toolbar
        zoom={zoom}
        onZoomIn={zoomIn}
        onZoomOut={zoomOut}
        onZoomReset={zoomReset}
        isDark={isEffectivelyDark()}
        onToggleTheme={toggleTheme}
      />
      <div className="app">
      <Sidebar
        project={project}
        onOpenProject={onOpenProject}
        collections={collections}
        environments={environments}
        activeEnvironment={activeEnvironment}
        sidebarTab={sidebarTab}
        onSidebarTabChange={setSidebarTab}
        history={history}
        refreshKey={collectionsVersion}
        onLoadRequest={onLoadRequest}
        onSelectEnvironment={setActiveEnvironment}
        onOpenEnvironmentTab={() => setCentralView({ kind: "environment" })}
        onCreateEnvironment={onCreateEnvironment}
        onLoadHistoryEntry={onLoadHistoryEntry}
        onOpenCollectionTab={onOpenCollectionTab}
        onOpenFolderTab={onOpenFolderTab}
        onRenameRequest={onRenameRequest}
        onDuplicateRequest={onDuplicateRequest}
        onDeleteRequest={onDeleteRequest}
        onNewFolder={onNewFolder}
        onNewRequest={onNewRequest}
        onDeleteFolder={onDeleteFolder}
        onSaveAllCollection={onSaveAllCollection}
      />

      <main className="main">
        <TabStrip
          tabs={tabs}
          activeTabIdx={activeTabIdx}
          centralView={centralView}
          extraTabs={extraTabs}
          environments={environments}
          activeEnvironment={activeEnvironment}
          onSwitch={onSwitchTab}
          onClose={closeTab}
          onNew={openNewTab}
          onFocusExtraTab={openOrFocusExtraTab}
          onCloseExtraTab={onCloseExtraTab}
          onOpenEnvironmentTab={() => setCentralView({ kind: "environment" })}
          onSelectActiveEnvironment={setActiveEnvironment}
        />

        {notice && (
          <div className="notice" onClick={() => setNotice(null)}>
            {notice}
          </div>
        )}

        {centralView.kind === "request" && (
          <>
            <RequestEditor
              tab={activeTab}
              onChange={updateActiveTab}
              onSend={onSend}
              onSave={onSave}
              onSaveAs={onSaveAs}
              onRenameBreadcrumb={onRenameBreadcrumb}
            />
            <ResponsePane tab={activeTab} onLoadHistoryEntry={onLoadHistoryEntry} />
          </>
        )}

        {centralView.kind === "collection" && (
          <CollectionTabView
            collection={centralView.name}
            refreshKey={collectionsVersion}
            onOpenFolder={onOpenFolderTab}
            onLoadRequest={onLoadRequest}
            onNewFolder={onNewFolder}
            onNewRequest={onNewRequest}
            onSaveAll={onSaveAllCollection}
          />
        )}

        {centralView.kind === "folder" && (
          <FolderTabView
            collection={centralView.collection}
            path={centralView.path}
            refreshKey={collectionsVersion}
            onOpenFolder={onOpenFolderTab}
            onLoadRequest={onLoadRequest}
            onNewFolder={onNewFolder}
            onNewRequest={onNewRequest}
          />
        )}

        {centralView.kind === "environment" && (
          <EnvironmentTabView
            name={activeEnvironment}
            refreshKey={environmentsVersion}
            onNotice={setNotice}
            onSaved={() => setEnvironmentsVersion((v) => v + 1)}
            onDeleted={onEnvironmentDeleted}
          />
        )}
      </main>

      {openProjectDialog && (
        <OpenProjectDialog
          state={openProjectDialog}
          onChange={(patch) => setOpenProjectDialog((d) => (d ? { ...d, ...patch } : d))}
          onConfirm={confirmOpenProject}
          onCancel={() => setOpenProjectDialog(null)}
        />
      )}

      {saveAsDialog && (
        <SaveAsDialog
          state={saveAsDialog}
          onChange={(patch) => setSaveAsDialog((d) => (d ? { ...d, ...patch } : d))}
          onConfirm={confirmSaveAs}
          onCancel={() => setSaveAsDialog(null)}
        />
      )}

      {pathDialog && (
        <PathDialog
          state={pathDialog}
          onChange={(patch) => setPathDialog((d) => (d ? { ...d, ...patch } : d))}
          onConfirm={confirmPathDialog}
          onCancel={() => setPathDialog(null)}
        />
      )}

      {folderDialog && (
        <FolderDialog
          state={folderDialog}
          onChange={(patch) => setFolderDialog((d) => (d ? { ...d, ...patch } : d))}
          onConfirm={confirmFolderDialog}
          onCancel={() => setFolderDialog(null)}
        />
      )}
      </div>
    </div>
  );
}
