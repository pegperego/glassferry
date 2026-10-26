// Typed wrappers around the Tauri commands exposed by src-tauri/src/lib.rs.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface HostEntry {
  alias: string;
  source: string;
}

export interface Capabilities {
  shell: boolean;
  os: string | null;
  commands: string[];
}

export interface ConnectResult {
  sessionId: number;
  home: string;
  capabilities: Capabilities;
}

export type EntryKind = "dir" | "file" | "symlink" | "other";

export interface RemoteEntry {
  name: string;
  kind: EntryKind;
  targetIsDir: boolean;
  size: number | null;
  /** Seconds since the Unix epoch. */
  modified: number | null;
  permissions: number | null;
}

export type PromptKind = "secret" | "confirm" | "notice" | "hostKey";

export interface AskpassRequest {
  id: number;
  alias: string;
  prompt: string;
  kind: PromptKind;
}

export const listHosts = () => invoke<HostEntry[]>("list_hosts");

export const resolveHost = (alias: string) =>
  invoke<Record<string, string>>("resolve_host", { alias });

export const connect = (alias: string) => invoke<ConnectResult>("connect", { alias });

export const listDir = (sessionId: number, path: string) =>
  invoke<RemoteEntry[]>("list_dir", { sessionId, path });

export const disconnect = (sessionId: number) => invoke<void>("disconnect", { sessionId });

/** `value === null` cancels the prompt. */
export const askpassRespond = (id: number, value: string | null) =>
  invoke<void>("askpass_respond", { id, value });

export const onAskpassRequest = (handler: (request: AskpassRequest) => void): Promise<UnlistenFn> =>
  listen<AskpassRequest>("askpass-request", (event) => handler(event.payload));

export const isDirLike = (entry: RemoteEntry) =>
  entry.kind === "dir" || (entry.kind === "symlink" && entry.targetIsDir);
