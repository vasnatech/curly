import { useEffect, useState } from "react";
import * as api from "../api";
import { findFolderByPath } from "../collectionTree";
import type { Folder, SavedRequest } from "../types";

interface FolderTabViewProps {
  collection: string;
  path: string;
  refreshKey: number;
  onOpenFolder: (collection: string, path: string) => void;
  onLoadRequest: (collection: string, path: string, saved: SavedRequest) => void;
  onNewFolder: (collection: string, parentPath: string) => void;
  onNewRequest: (collection: string, parentPath: string) => void;
}

export function FolderTabView({
  collection,
  path,
  refreshKey,
  onOpenFolder,
  onLoadRequest,
  onNewFolder,
  onNewRequest,
}: FolderTabViewProps) {
  const [folder, setFolder] = useState<Folder | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .loadCollection(collection)
      .then((c) => setFolder(findFolderByPath(c, path) ?? null))
      .catch((e) => setError(String(e)));
  }, [collection, path, refreshKey]);

  const leaf = path.split("/").filter(Boolean).pop() ?? path;

  return (
    <div className="extra-tab-view">
      <h2>{leaf}/</h2>
      <div className="weak">
        {collection} › {path}
      </div>
      <div className="extra-tab-actions">
        <button onClick={() => onNewFolder(collection, path)}>New Folder</button>
        <button onClick={() => onNewRequest(collection, path)}>New Request</button>
      </div>
      {error && <div className="response-error">{error}</div>}
      {folder === null && <div className="weak">This folder no longer exists — it may have been renamed or deleted.</div>}
      {folder && (
        <div className="extra-tab-list">
          {folder.folders.map((f) => (
            <div key={f.id} className="tree-row" onClick={() => onOpenFolder(collection, `${path}/${f.name}`)}>
              {f.name}/
            </div>
          ))}
          {folder.requests.map((r) => (
            <div key={r.id} className="tree-row" onClick={() => onLoadRequest(collection, `${path}/${r.name}`, r)}>
              <span className="method-tag">{r.method}</span> {r.name}
            </div>
          ))}
          {folder.folders.length === 0 && folder.requests.length === 0 && <div className="weak">(empty)</div>}
        </div>
      )}
    </div>
  );
}
