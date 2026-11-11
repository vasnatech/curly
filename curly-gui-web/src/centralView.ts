// What the central panel shows (M8) — `request` defers to `activeTabIdx`
// exactly as the first slice already did; the other three are additional
// non-request views that leave whichever request tab is active completely
// untouched underneath (no take/restore needed here either, same reason
// as tabState.ts's own note: each is just a plain object, not something
// swapped in and out of a shared set of fields).

export type ExtraTab = { kind: "collection"; name: string } | { kind: "folder"; collection: string; path: string };

export type CentralView =
  | { kind: "request" }
  | { kind: "collection"; name: string }
  | { kind: "folder"; collection: string; path: string }
  | { kind: "environment" };

export function extraTabKey(tab: ExtraTab): string {
  return tab.kind === "collection" ? `collection:${tab.name}` : `folder:${tab.collection}:${tab.path}`;
}

export function extraTabLabel(tab: ExtraTab): string {
  return tab.kind === "collection" ? tab.name : `${tab.path}/`;
}

export function extraTabToCentralView(tab: ExtraTab): CentralView {
  return tab.kind === "collection" ? { kind: "collection", name: tab.name } : { kind: "folder", collection: tab.collection, path: tab.path };
}

export function centralViewEquals(a: CentralView, b: CentralView): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === "collection" && b.kind === "collection") return a.name === b.name;
  if (a.kind === "folder" && b.kind === "folder") return a.collection === b.collection && a.path === b.path;
  return true;
}
