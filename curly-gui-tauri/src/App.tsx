import { useEffect, useState } from "react";
import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
import * as api from "./api";
import { centralViewEquals, extraTabKey, extraTabToCentralView, type CentralView, type ExtraTab } from "./centralView";
import { withRequestReplaced } from "./collectionTree";
import { CollectionTabView } from "./components/CollectionTabView";
import { EnvironmentTabView } from "./components/EnvironmentTabView";
import { FolderDialog, type FolderDialogState } from "./components/FolderDialog";
import { FolderTabView } from "./components/FolderTabView";
import { PathDialog, type PathDialogState } from "./components/PathDialog";
import { RequestEditor } from "./components/RequestEditor";
import { ResponsePane } from "./components/ResponsePane";
import { SaveAsDialog, type SaveAsDialogState } from "./components/SaveAsDialog";
import { Sidebar } from "./components/Sidebar";
import { TabStrip } from "./components/TabStrip";
import { blankTab, isTabDirty, markSaved, tabFromHistoryEntry, tabFromSavedRequest, tabToSavedRequest } from "./tabState";
import type { RequestTabState } from "./tabState";
import type { HistoryEntry, ProjectInfo, SavedRequest } from "./types";
import "./App.css";

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
  const [tabs, setTabs] = useState<RequestTabState[]>([blankTab()]);
  const [activeTabIdx, setActiveTabIdx] = useState(0);
  const [notice, setNotice] = useState<string | null>(null);
  const [saveAsDialog, setSaveAsDialog] = useState<SaveAsDialogState | null>(null);
  const [pathDialog, setPathDialog] = useState<PathDialogState | null>(null);
  const [folderDialog, setFolderDialog] = useState<FolderDialogState | null>(null);
  const [collectionsVersion, setCollectionsVersion] = useState(0);
  const [environmentsVersion, setEnvironmentsVersion] = useState(0);
  const [centralView, setCentralView] = useState<CentralView>({ kind: "request" });
  const [extraTabs, setExtraTabs] = useState<ExtraTab[]>([]);

  const activeTab = tabs[activeTabIdx];

  function bumpCollectionsVersion() {
    setCollectionsVersion((v) => v + 1);
  }

  function refreshHistoryIfShown() {
    if (sidebarTab === "history") {
      api.listHistory(50).then(setHistory).catch(() => {});
    }
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
  async function onOpenProject() {
    let picked: string | string[] | null;
    try {
      picked = await openFolderDialog({ directory: true, multiple: false });
    } catch (e) {
      setNotice(`failed to open the folder picker: ${e}`);
      return;
    }
    if (!picked || Array.isArray(picked)) return;
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
      setNotice(`opened project "${info.label}"`);
    } catch (e) {
      setNotice(`failed to open project: ${e}`);
    }
  }

  function updateActiveTab(patch: Partial<RequestTabState>) {
    setTabs((prev) => prev.map((t, i) => (i === activeTabIdx ? { ...t, ...patch } : t)));
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

  async function onSend() {
    updateActiveTab({ inFlight: true, responseError: null });
    try {
      const saved = tabToSavedRequest(activeTab);
      const overrides: [string, string][] = activeTab.overrides
        .filter((o) => o.enabled && o.name.trim() !== "")
        .map((o) => [o.name, o.value]);
      const outcome = await api.sendRequest(saved, activeEnvironment, overrides);
      updateActiveTab({ response: outcome.response, inFlight: false });
      refreshHistoryIfShown();
      if (outcome.extracted.length > 0) {
        setNotice(`extracted ${outcome.extracted.map((e) => e.name).join(", ")}`);
      }
    } catch (e) {
      updateActiveTab({ inFlight: false, responseError: String(e), response: null });
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
  );
}
