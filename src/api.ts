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

export interface FileEntry {
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

/** `verbose` enables the diagnostic mode (`ssh -v` on the master). */
export const connect = (alias: string, verbose: boolean) =>
  invoke<ConnectResult>("connect", { alias, verbose });

export const listDir = (sessionId: number, path: string) =>
  invoke<FileEntry[]>("list_dir", { sessionId, path });

export const localHome = () => invoke<string>("local_home");

export const localListDir = (path: string) => invoke<FileEntry[]>("local_list_dir", { path });

export const disconnect = (sessionId: number) => invoke<void>("disconnect", { sessionId });

/** `value === null` cancels the prompt. */
export const askpassRespond = (id: number, value: string | null) =>
  invoke<void>("askpass_respond", { id, value });

export const onAskpassRequest = (handler: (request: AskpassRequest) => void): Promise<UnlistenFn> =>
  listen<AskpassRequest>("askpass-request", (event) => handler(event.payload));

export type LogChannel = "ssh" | "sftp" | "exec" | "askpass" | "local";
export type LogStatus = "pending" | "ok" | "error";

export interface LogEntry {
  id: number;
  /** Milliseconds since the Unix epoch. */
  time: number;
  host: string | null;
  channel: LogChannel;
  op: string;
  detail: string;
  status: LogStatus;
  durationMs: number | null;
  extra: string | null;
  error: string | null;
  output: string | null;
  equivalent: string | null;
}

export const logSnapshot = () => invoke<LogEntry[]>("log_snapshot");

export const logClear = () => invoke<void>("log_clear");

export const logSetCapacity = (capacity: number) => invoke<void>("log_set_capacity", { capacity });

export const logOpenDir = () => invoke<void>("log_open_dir");

/** New and updated entries, delivered in batches. */
export const onLogBatch = (handler: (batch: LogEntry[]) => void): Promise<UnlistenFn> =>
  listen<LogEntry[]>("log-batch", (event) => handler(event.payload));

export type Direction = "upload" | "download";
export type JobState = "scanning" | "running" | "waitingConflict" | "done" | "failed" | "cancelled";

export interface JobInfo {
  id: number;
  sessionId: number;
  host: string;
  direction: Direction;
  srcDir: string;
  dstDir: string;
  names: string[];
  state: JobState;
  totalBytes: number;
  doneBytes: number;
  totalFiles: number;
  doneFiles: number;
  skippedFiles: number;
  failedFiles: number;
  current: string | null;
  bytesPerSec: number;
  startedAt: number;
  finishedAt: number | null;
  errors: string[];
}

export interface ConflictRequest {
  id: number;
  jobId: number;
  host: string;
  direction: Direction;
  path: string;
  srcSize: number;
  srcModified: number | null;
  dstSize: number | null;
  dstModified: number | null;
}

export type ConflictAction = "overwrite" | "skip" | "cancel";

export const transferStart = (
  sessionId: number,
  direction: Direction,
  srcDir: string,
  names: string[],
  dstDir: string,
) => invoke<number>("transfer_start", { sessionId, direction, srcDir, names, dstDir });

export const transferCancel = (jobId: number) => invoke<void>("transfer_cancel", { jobId });

export const transferList = () => invoke<JobInfo[]>("transfer_list");

export const transferClearFinished = () => invoke<void>("transfer_clear_finished");

export const transferConflictRespond = (id: number, action: ConflictAction, applyToAll: boolean) =>
  invoke<void>("transfer_conflict_respond", { id, action, applyToAll });

export const onTransferUpdate = (handler: (jobs: JobInfo[]) => void): Promise<UnlistenFn> =>
  listen<JobInfo[]>("transfer-update", (event) => handler(event.payload));

export const onTransferConflict = (handler: (request: ConflictRequest) => void): Promise<UnlistenFn> =>
  listen<ConflictRequest>("transfer-conflict", (event) => handler(event.payload));

export const isFinished = (job: JobInfo) =>
  job.state === "done" || job.state === "failed" || job.state === "cancelled";

export const isDirLike = (entry: FileEntry) =>
  entry.kind === "dir" || (entry.kind === "symlink" && entry.targetIsDir);
