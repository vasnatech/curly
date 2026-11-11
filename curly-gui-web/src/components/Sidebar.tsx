import { useEffect, useState } from "react";
import * as api from "../api";
import type { Collection, Folder, HistoryEntry, ProjectInfo, SavedRequest } from "../types";
import { RowMenu } from "./RowMenu";

interface SidebarProps {
  project: ProjectInfo | null;
  onOpenProject: () => void;
  collections: string[];
  environments: string[];
  activeEnvironment: string | null;
  sidebarTab: "project" | "history";
  onSidebarTabChange: (tab: "project" | "history") => void;
  history: HistoryEntry[];
  /** Bumped after any mutation (save/rename/duplicate/delete/new
   * folder/new request/save all) so an already-open `CollectionSection`
   * re-fetches instead of showing stale tree data. */
  refreshKey: number;
  onLoadRequest: (collection: string, path: string, saved: SavedRequest) => void;
  onSelectEnvironment: (name: string | null) => void;
  onOpenEnvironmentTab: (name: string) => void;
  onCreateEnvironment: (name: string) => void;
  onLoadHistoryEntry: (entry: HistoryEntry) => void;
  onOpenCollectionTab: (collection: string) => void;
  onOpenFolderTab: (collection: string, path: string) => void;
  onRenameRequest: (collection: string, path: string) => void;
  onDuplicateRequest: (collection: string, path: string) => void;
  onDeleteRequest: (collection: string, path: string) => void;
  onNewFolder: (collection: string, parentPath: string) => void;
  onNewRequest: (collection: string, parentPath: string) => void;
  onDeleteFolder: (collection: string, path: string) => void;
  onSaveAllCollection: (collection: string) => void;
}

type TreeActions = Pick<
  SidebarProps,
  | "onLoadRequest"
  | "onOpenFolderTab"
  | "onRenameRequest"
  | "onDuplicateRequest"
  | "onDeleteRequest"
  | "onNewFolder"
  | "onNewRequest"
  | "onDeleteFolder"
>;

/** One collection's tree — lazily loads its full `Collection` (folders +
 * requests) the first time it's expanded, and again whenever `refreshKey`
 * changes while still open (see that prop's own doc comment). The arrow
 * toggles expand/collapse; clicking the collection's own name opens it as
 * its own main tab (see `onOpenCollectionTab`) — two separate click
 * targets in the same row, matching the egui GUI's M8 pass. */
function CollectionSection({
  name,
  refreshKey,
  actions,
  onOpenCollectionTab,
  onSaveAllCollection,
}: {
  name: string;
  refreshKey: number;
  actions: TreeActions;
  onOpenCollectionTab: SidebarProps["onOpenCollectionTab"];
  onSaveAllCollection: SidebarProps["onSaveAllCollection"];
}) {
  const [open, setOpen] = useState(false);
  const [collection, setCollection] = useState<Collection | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    api
      .loadCollection(name)
      .then(setCollection)
      .catch((e) => setError(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, refreshKey, name]);

  return (
    <div className="tree-section">
      <div className="tree-row tree-row-collection">
        <span className="tree-arrow" onClick={() => setOpen((o) => !o)}>
          {open ? "▾" : "▸"}
        </span>
        <span className="tree-label" onClick={() => onOpenCollectionTab(name)}>
          {name}
        </span>
        <RowMenu
          actions={[
            { label: "New Folder", onClick: () => actions.onNewFolder(name, "") },
            { label: "New Request", onClick: () => actions.onNewRequest(name, "") },
            { label: "Save All", onClick: () => onSaveAllCollection(name) },
          ]}
        />
      </div>
      {open && error && <div className="tree-error">{error}</div>}
      {open && collection && (
        <FolderContents folders={collection.folders} requests={collection.requests} collection={name} prefix="" actions={actions} />
      )}
    </div>
  );
}

function FolderContents({
  folders,
  requests,
  collection,
  prefix,
  actions,
}: {
  folders: Folder[];
  requests: SavedRequest[];
  collection: string;
  prefix: string;
  actions: TreeActions;
}) {
  return (
    <div className="tree-children">
      {folders.map((f) => (
        <FolderNode key={f.id} folder={f} collection={collection} prefix={prefix} actions={actions} />
      ))}
      {requests.map((r) => {
        const path = prefix ? `${prefix}/${r.name}` : r.name;
        return (
          <div key={r.id} className="tree-row tree-row-request" onClick={() => actions.onLoadRequest(collection, path, r)}>
            <span className="method-tag">{r.method}</span>
            <span>{r.name}</span>
            <RowMenu
              actions={[
                { label: "Rename", onClick: () => actions.onRenameRequest(collection, path) },
                { label: "Duplicate", onClick: () => actions.onDuplicateRequest(collection, path) },
                { label: "Delete", onClick: () => actions.onDeleteRequest(collection, path) },
              ]}
            />
          </div>
        );
      })}
    </div>
  );
}

function FolderNode({
  folder,
  collection,
  prefix,
  actions,
}: {
  folder: Folder;
  collection: string;
  prefix: string;
  actions: TreeActions;
}) {
  const [open, setOpen] = useState(false);
  const folderPath = prefix ? `${prefix}/${folder.name}` : folder.name;
  return (
    <div className="tree-section">
      <div className="tree-row tree-row-folder">
        <span className="tree-arrow" onClick={() => setOpen((o) => !o)}>
          {open ? "▾" : "▸"}
        </span>
        <span className="tree-label" onClick={() => actions.onOpenFolderTab(collection, folderPath)}>
          {folder.name}/
        </span>
        <RowMenu
          actions={[
            { label: "New Folder", onClick: () => actions.onNewFolder(collection, folderPath) },
            { label: "New Request", onClick: () => actions.onNewRequest(collection, folderPath) },
            { label: "Delete Folder", onClick: () => actions.onDeleteFolder(collection, folderPath) },
          ]}
        />
      </div>
      {open && (
        <FolderContents folders={folder.folders} requests={folder.requests} collection={collection} prefix={folderPath} actions={actions} />
      )}
    </div>
  );
}

export function Sidebar(props: SidebarProps) {
  const {
    project,
    onOpenProject,
    collections,
    environments,
    activeEnvironment,
    sidebarTab,
    onSidebarTabChange,
    history,
    refreshKey,
    onSelectEnvironment,
    onOpenEnvironmentTab,
    onCreateEnvironment,
    onLoadHistoryEntry,
    onOpenCollectionTab,
    onSaveAllCollection,
  } = props;

  const [newEnvName, setNewEnvName] = useState("");

  const actions: TreeActions = {
    onLoadRequest: props.onLoadRequest,
    onOpenFolderTab: props.onOpenFolderTab,
    onRenameRequest: props.onRenameRequest,
    onDuplicateRequest: props.onDuplicateRequest,
    onDeleteRequest: props.onDeleteRequest,
    onNewFolder: props.onNewFolder,
    onNewRequest: props.onNewRequest,
    onDeleteFolder: props.onDeleteFolder,
  };

  return (
    <aside className="sidebar">
      <div className="sidebar-project">
        <div className="sidebar-project-row">
          <div className="sidebar-project-label">{project?.label ?? "No project open"}</div>
          <button className="btn-open-project" onClick={onOpenProject}>
            Open Project…
          </button>
        </div>
        {project && <div className="sidebar-project-root">{project.root}</div>}
      </div>

      <div className="sidebar-tabs">
        <button
          className={sidebarTab === "project" ? "sidebar-tab active" : "sidebar-tab"}
          onClick={() => onSidebarTabChange("project")}
        >
          Project
        </button>
        <button
          className={sidebarTab === "history" ? "sidebar-tab active" : "sidebar-tab"}
          onClick={() => onSidebarTabChange("history")}
        >
          History
        </button>
      </div>

      {sidebarTab === "history" ? (
        <div className="sidebar-section">
          {history.length === 0 && <div className="weak">(no history yet)</div>}
          {history.map((entry, i) => (
            <div key={i} className="tree-row" onClick={() => onLoadHistoryEntry(entry)}>
              <span className="method-tag">{entry.method}</span>
              <span>{entry.status}</span>
              <span className="weak"> {entry.url}</span>
            </div>
          ))}
        </div>
      ) : (
        <>
          <div className="sidebar-section">
            <div className="sidebar-heading">Collections ({collections.length})</div>
            {collections.length === 0 && <div className="weak">(none yet)</div>}
            {collections.map((name) => (
              <CollectionSection
                key={name}
                name={name}
                refreshKey={refreshKey}
                actions={actions}
                onOpenCollectionTab={onOpenCollectionTab}
                onSaveAllCollection={onSaveAllCollection}
              />
            ))}
          </div>

          <div className="sidebar-section">
            <div className="sidebar-heading">Environments ({environments.length})</div>
            <div
              className={activeEnvironment === null ? "tree-row active" : "tree-row"}
              onClick={() => onSelectEnvironment(null)}
            >
              (none selected)
            </div>
            {environments.map((name) => (
              <div
                key={name}
                className={activeEnvironment === name ? "tree-row active" : "tree-row"}
                onClick={() => {
                  onSelectEnvironment(name);
                  onOpenEnvironmentTab(name);
                }}
              >
                {name}
              </div>
            ))}
            <div className="new-env-row">
              <input
                placeholder="new environment"
                value={newEnvName}
                onChange={(e) => setNewEnvName(e.target.value)}
              />
              <button
                disabled={newEnvName.trim() === ""}
                onClick={() => {
                  onCreateEnvironment(newEnvName.trim());
                  setNewEnvName("");
                }}
              >
                + New
              </button>
            </div>
          </div>
        </>
      )}
    </aside>
  );
}
