// TypeScript mirrors of the curly-core serde JSON shapes each Tauri command
// sends/receives. Field names and enum tagging must match the Rust side
// exactly (`#[serde(tag = "...", rename_all = "snake_case")]` etc.) — see
// crates/curly-core/src/{storage,exec,extraction}.rs for the source of
// truth. Kept as one file for now (the surface is still small); split if it
// grows unwieldy.

export interface KvPair {
  name: string;
  value: string;
  enabled: boolean;
}

export type SavedBody =
  | { type: "raw"; content: string }
  | { type: "binary"; path: string }
  | { type: "form"; fields: [string, string][] }
  | { type: "multipart"; fields: SavedMultipartField[] };

export type SavedMultipartField =
  | { type: "text"; name: string; value: string }
  | { type: "file"; name: string; path: string };

export type SavedAuth =
  | { type: "basic"; username: string; password: string }
  | { type: "bearer"; token: string };

export type Extraction =
  | { source: "body"; name: string; path: string; secret: boolean }
  | { source: "header"; name: string; header: string; secret: boolean }
  | { source: "template"; name: string; template: string; secret: boolean };

export interface SavedRequest {
  id: string;
  name: string;
  method: string;
  url: string;
  query_params: KvPair[];
  headers: KvPair[];
  body: SavedBody | null;
  auth: SavedAuth | null;
  extract: Extraction[];
}

export interface Folder {
  id: string;
  name: string;
  folders: Folder[];
  requests: SavedRequest[];
}

export interface Collection {
  id: string;
  name: string;
  folders: Folder[];
  requests: SavedRequest[];
}

export interface Variable {
  key: string;
  value: string;
  secret: boolean;
}

export interface Environment {
  name: string;
  variables: Variable[];
}

export type HistoryAuthKind = "Basic" | "Bearer";

export interface HistoryEntry {
  sent_at: string;
  method: string;
  url: string;
  request_headers: [string, string][];
  request_body: string | null;
  request_body_truncated: boolean;
  request_auth_kind: HistoryAuthKind | null;
  status: number;
  elapsed_ms: number;
  response_headers: [string, string][];
  response_body: string;
  response_body_truncated: boolean;
}

export interface ResponseSummary {
  status: number;
  headers: [string, string][];
  body: string;
  body_bytes: number[];
  elapsed: { secs: number; nanos: number };
}

export interface Extracted {
  name: string;
  value: string;
  secret: boolean;
}

export interface SendOutcome {
  response: ResponseSummary;
  extracted: Extracted[];
}

export interface ProjectInfo {
  label: string;
  root: string;
}
