// Typed wrappers around every Tauri command from src-tauri/src/commands.rs.
// One function per command, same name, so the mapping stays obvious. Errors
// from the Rust side arrive as a plain string (every command maps its
// anyhow::Error via `.to_string()`) — left as a rejected Promise<string>,
// not wrapped further, so callers can show it directly.

import { invoke } from "@tauri-apps/api/core";
import type {
  Collection,
  Environment,
  HistoryEntry,
  ProjectInfo,
  SavedRequest,
  SendOutcome,
} from "./types";

export const openProject = (path: string) => invoke<ProjectInfo>("open_project", { path });

export const resolveDefaultProject = () => invoke<ProjectInfo>("resolve_default_project");

export const listCollections = () => invoke<string[]>("list_collections");

export const loadCollection = (name: string) => invoke<Collection>("load_collection", { name });

export const saveCollection = (collection: Collection) => invoke<void>("save_collection", { collection });

export const deleteCollection = (name: string) => invoke<void>("delete_collection", { name });

export const addRequest = (collection: string, path: string, request: SavedRequest) =>
  invoke<void>("add_request", { collection, path, request });

export const renameOrMoveRequest = (collection: string, oldPath: string, newPath: string) =>
  invoke<void>("rename_or_move_request", { collection, oldPath, newPath });

export const duplicateRequest = (collection: string, path: string, newPath: string) =>
  invoke<void>("duplicate_request", { collection, path, newPath });

export const removeRequest = (collection: string, path: string) =>
  invoke<boolean>("remove_request", { collection, path });

export const addFolder = (collection: string, path: string) => invoke<void>("add_folder", { collection, path });

export const removeFolder = (collection: string, path: string, force: boolean) =>
  invoke<boolean>("remove_folder", { collection, path, force });

export const listEnvironments = () => invoke<string[]>("list_environments");

export const loadEnvironment = (name: string) => invoke<Environment>("load_environment", { name });

export const saveEnvironment = (environment: Environment) => invoke<void>("save_environment", { environment });

export const deleteEnvironment = (name: string) => invoke<void>("delete_environment", { name });

export const loadSession = (envName: string) => invoke<Environment>("load_session", { envName });

export const saveSession = (session: Environment) => invoke<void>("save_session", { session });

export const clearSession = (envName: string) => invoke<void>("clear_session", { envName });

export const listHistory = (limit: number) => invoke<HistoryEntry[]>("list_history", { limit });

export const sendRequest = (saved: SavedRequest, envName: string | null, overrides: [string, string][]) =>
  invoke<SendOutcome>("send_request", { saved, envName, overrides });
