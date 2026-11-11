import { useEffect, useState } from "react";
import * as api from "../api";
import type { Collection, SavedRequest } from "../types";

interface CollectionTabViewProps {
  collection: string;
  refreshKey: number;
  onOpenFolder: (collection: string, path: string) => void;
  onLoadRequest: (collection: string, path: string, saved: SavedRequest) => void;
  onNewFolder: (collection: string, parentPath: string) => void;
  onNewRequest: (collection: string, parentPath: string) => void;
  onSaveAll: (collection: string) => void;
}

export function CollectionTabView({
  collection,
  refreshKey,
  onOpenFolder,
  onLoadRequest,
  onNewFolder,
  onNewRequest,
  onSaveAll,
}: CollectionTabViewProps) {
  const [data, setData] = useState<Collection | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .loadCollection(collection)
      .then(setData)
      .catch((e) => setError(String(e)));
  }, [collection, refreshKey]);

  return (
    <div className="extra-tab-view">
      <h2>{collection}</h2>
      <div className="extra-tab-actions">
        <button className="btn-save" onClick={() => onSaveAll(collection)}>
          Save All
        </button>
        <button onClick={() => onNewFolder(collection, "")}>New Folder</button>
        <button onClick={() => onNewRequest(collection, "")}>New Request</button>
      </div>
      {error && <div className="response-error">{error}</div>}
      {data && (
        <div className="extra-tab-list">
          {data.folders.map((f) => (
            <div key={f.id} className="tree-row" onClick={() => onOpenFolder(collection, f.name)}>
              {f.name}/
            </div>
          ))}
          {data.requests.map((r) => (
            <div key={r.id} className="tree-row" onClick={() => onLoadRequest(collection, r.name, r)}>
              <span className="method-tag">{r.method}</span> {r.name}
            </div>
          ))}
          {data.folders.length === 0 && data.requests.length === 0 && <div className="weak">(empty)</div>}
        </div>
      )}
    </div>
  );
}
