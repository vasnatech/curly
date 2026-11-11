// Typed wrappers around every command `curly serve` exposes. One function
// per command, same name, so the mapping to
// ziman/src/main/ziman/server/api.zm stays obvious.
//
// **These used to be Tauri `invoke` calls and the bodies are unchanged.**
// The Ziman port replaced the Tauri bridge with an HTTP server, and because
// that server kept `invoke`'s exact shape — `POST /api/<command>` carrying
// the same object of named arguments, answering the same serde JSON — the
// whole move is the `post` helper below. Argument names therefore keep
// their JavaScript spelling (`envName`, `oldPath`): Tauri converted
// camelCase to its snake_case Rust parameter, and the server has no such
// layer, so these names ARE the wire names.
//
// Errors arrive as a plain string, exactly as they did through Tauri (every
// command is `Result<T, String>` on the Rust side) — a refusal is a non-2xx
// whose whole body is the message, rethrown as an `Error` so callers can
// show `e` directly, as they already do.
import type {
  Collection,
  Environment,
  HistoryEntry,
  ProjectInfo,
  SavedRequest,
  SendOutcome,
} from "./types";

/** Where `curly serve` is.
 *
 * Relative on purpose, so there is one answer in both arrangements: in
 * production the server serves this page as well as the API, and in
 * development Vite's `server.proxy` forwards `/api` from the dev server's
 * own origin (see vite.config.ts). An absolute URL would make every call
 * cross-origin, which the server refuses — deliberately, since these
 * commands read and write the user's files and a server that accepted them
 * from anywhere would hand every open tab a file-writing API. */
const API = "/api";

const post = async <T>(command: string, args: unknown = {}): Promise<T> => {
  const answer = await fetch(`${API}/${command}`, {
    method: "POST",
    // Not negotiable: the server requires it. A JSON content type is not a
    // CORS "simple request", so it cannot be sent cross-origin without a
    // preflight the server never answers.
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args),
  });
  if (!answer.ok) {
    // The body IS the message. `.text()` and not `.json()`: a refusal is
    // plain text, and a server that fell over entirely would not be JSON
    // either.
    throw new Error((await answer.text()).trim() || `${command} failed (${answer.status})`);
  }
  return (await answer.json()) as T;
};

export const openProject = (path: string) => post<ProjectInfo>("open_project", { path });

export const resolveDefaultProject = () => post<ProjectInfo>("resolve_default_project");

export const listCollections = () => post<string[]>("list_collections");

export const loadCollection = (name: string) => post<Collection>("load_collection", { name });

export const saveCollection = (collection: Collection) => post<void>("save_collection", { collection });

export const deleteCollection = (name: string) => post<void>("delete_collection", { name });

export const addRequest = (collection: string, path: string, request: SavedRequest) =>
  post<void>("add_request", { collection, path, request });

export const renameOrMoveRequest = (collection: string, oldPath: string, newPath: string) =>
  post<void>("rename_or_move_request", { collection, oldPath, newPath });

export const duplicateRequest = (collection: string, path: string, newPath: string) =>
  post<void>("duplicate_request", { collection, path, newPath });

export const removeRequest = (collection: string, path: string) =>
  post<boolean>("remove_request", { collection, path });

export const addFolder = (collection: string, path: string) => post<void>("add_folder", { collection, path });

export const removeFolder = (collection: string, path: string, force: boolean) =>
  post<boolean>("remove_folder", { collection, path, force });

export const listEnvironments = () => post<string[]>("list_environments");

export const loadEnvironment = (name: string) => post<Environment>("load_environment", { name });

export const saveEnvironment = (environment: Environment) => post<void>("save_environment", { environment });

export const deleteEnvironment = (name: string) => post<void>("delete_environment", { name });

export const loadSession = (envName: string) => post<Environment>("load_session", { envName });

export const saveSession = (session: Environment) => post<void>("save_session", { session });

export const clearSession = (envName: string) => post<void>("clear_session", { envName });

export const listHistory = (limit: number) => post<HistoryEntry[]>("list_history", { limit });

export const sendRequest = (saved: SavedRequest, envName: string | null, overrides: [string, string][]) =>
  post<SendOutcome>("send_request", { saved, envName, overrides });

/** Hand the user `bytes` to keep, under the name `filename`.
 *
 * **The one command with no endpoint, on purpose.** It used to be
 * `save_bytes`, which wrote to a path a native save dialog had picked — an
 * affordance of a desktop shell. A server that wrote to an arbitrary path
 * on the host because a web page asked it to would be a straightforward
 * hole, so `curly serve` does not expose one, and in a browser the
 * operation is a download instead.
 *
 * The caller therefore supplies a *name*, not a path: where it lands is
 * between the user and their browser. Still byte-exact — `body_bytes`, the
 * server's own bytes, never the lossy UTF-8-decoded `body` string, which
 * would silently corrupt any binary response. */
export async function saveBytes(filename: string, bytes: number[]): Promise<void> {
  const url = URL.createObjectURL(new Blob([new Uint8Array(bytes)]));
  try {
    const link = document.createElement("a");
    link.href = url;
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    link.remove();
  } finally {
    // Revoked on the next tick rather than immediately: revoking while the
    // click is still being handled cancels the download in some browsers.
    setTimeout(() => URL.revokeObjectURL(url), 0);
  }
}
