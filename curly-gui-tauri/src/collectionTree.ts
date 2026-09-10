// Small client-side helpers for navigating/mutating a Collection's nested
// folder tree by slash-separated path — the JS-side mirror of
// curly-core::storage's private navigate_create/navigate_existing helpers
// (find_request/add_request/etc.), needed here because "overwrite this
// saved request in place, preserving its id" isn't its own Tauri command —
// the frontend already has the full Collection (from `loadCollection`), so
// it's simpler to mutate that client-side and call `saveCollection` once
// than to add a server-side command for every possible in-place edit.

import type { Collection, Folder, SavedRequest } from "./types";

interface Container {
  folders: Folder[];
  requests: SavedRequest[];
}

function splitPath(path: string): [string[], string] {
  const segments = path.split("/").filter(Boolean);
  const leaf = segments.pop();
  if (leaf === undefined) throw new Error(`empty request path`);
  return [segments, leaf];
}

function navigateContainer(root: Container, segments: string[]): Container | null {
  let current: Container = root;
  for (const seg of segments) {
    const found = current.folders.find((f) => f.name === seg);
    if (!found) return null;
    current = found;
  }
  return current;
}

export function findRequestByPath(collection: Collection, path: string): SavedRequest | undefined {
  const [folderSegs, leaf] = splitPath(path);
  const container = navigateContainer(collection, folderSegs);
  return container?.requests.find((r) => r.name === leaf);
}

/** Navigate to the `Folder` at `path` (slash-separated, e.g. "Auth/OAuth")
 * — used by the Folder main tab to find its own sub-folders/requests. */
export function findFolderByPath(collection: Collection, path: string): Folder | undefined {
  const segments = path.split("/").filter(Boolean);
  let current: Folder[] = collection.folders;
  let found: Folder | undefined;
  for (const seg of segments) {
    found = current.find((f) => f.name === seg);
    if (!found) return undefined;
    current = found.folders;
  }
  return found;
}

/// Returns a new Collection with the request at `path` replaced by
/// `updated`, preserving the original's `id` (identity) and `name` (the
/// path's own leaf, matching `Collection::add_request`'s "name comes from
/// the path" convention) — everything else in `updated` wins. Throws if
/// nothing exists at `path`.
export function withRequestReplaced(collection: Collection, path: string, updated: SavedRequest): Collection {
  const [folderSegs, leaf] = splitPath(path);
  const next: Collection = structuredClone(collection);
  const container = navigateContainer(next, folderSegs);
  if (!container) throw new Error(`no folder at "${folderSegs.join("/")}"`);
  const idx = container.requests.findIndex((r) => r.name === leaf);
  if (idx === -1) throw new Error(`no request at "${path}"`);
  const original = container.requests[idx];
  container.requests[idx] = { ...updated, id: original.id, name: leaf };
  return next;
}
