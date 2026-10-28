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
  /** Connection name. */
  alias: string;
  prompt: string;
  kind: PromptKind;
  /** Password prompt of a connection that saves passwords in the keychain. */
  canSave: boolean;
  /** The saved password was rejected. */
  savedInvalid: boolean;
}

export interface Category {
  id: string;
  name: string;
  color: string;
  /** Destructive operations ask for a stronger confirmation. */
  protected: boolean;
}

export type Source =
  | { kind: "sshConfig"; alias: string }
  | {
      kind: "app";
      host: string;
      user: string | null;
      port: number | null;
      identityFile: string | null;
      proxyJump: string | null;
      /** `Keyword value` lines. */
      extraOptions: string[];
    };

export interface Connection {
  id: string;
  name: string;
  categoryId: string;
  source: Source;
  remoteDir: string | null;
  localDir: string | null;
  savePassword: boolean;
}

export interface Store {
  version: number;
  categories: Category[];
  connections: Connection[];
}

export const listHosts = () => invoke<HostEntry[]>("list_hosts");

/** `warning`: connections.json could not be read and was set aside. */
export const connectionsLoad = () => invoke<{ store: Store; warning: string | null }>("connections_load");

/** Saves the whole store (and regenerates the app's ssh_config). */
export const connectionsSave = (store: Store) => invoke<Store>("connections_save", { store });

export const connectionForgetPassword = (id: string) => invoke<void>("connection_forget_password", { id });

/** Effective settings resolved by `ssh -G`, also for an unsaved source. */
export const connectionCheck = (source: Source) =>
  invoke<Record<string, string>>("connection_check", { source });

export const connectionsExport = (path: string) => invoke<void>("connections_export", { path });

/** Reads an exported file without saving it. */
export const connectionsImport = (path: string) => invoke<Store>("connections_import", { path });

/** `verbose` enables the diagnostic mode (`ssh -v` on the master). */
export const connect = (connectionId: string, verbose: boolean) =>
  invoke<ConnectResult>("connect", { connectionId, verbose });

export const listDir = (sessionId: number, path: string) =>
  invoke<FileEntry[]>("list_dir", { sessionId, path });

export const localHome = () => invoke<string>("local_home");

export const localListDir = (path: string) => invoke<FileEntry[]>("local_list_dir", { path });

export const disconnect = (sessionId: number) => invoke<void>("disconnect", { sessionId });

/** `value === null` cancels the prompt; `save` keeps a password in the keychain once connected. */
export const askpassRespond = (id: number, value: string | null, save = false) =>
  invoke<void>("askpass_respond", { id, value, save });

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

/** `remote`: from one remote pane to another (same or different session). */
export type Direction = "upload" | "download" | "remote";
export type JobState = "scanning" | "running" | "waitingConflict" | "done" | "failed" | "cancelled";

export interface JobInfo {
  id: number;
  /** Remote side; the source of a remote copy. */
  sessionId: number;
  host: string;
  /** Destination of a remote copy. */
  dstSessionId: number | null;
  dstHost: string | null;
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
  src: string;
  /** How the overwrite is carried out (preview). */
  commands: string[];
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
  dstSessionId: number | null = null,
) => invoke<number>("transfer_start", { sessionId, direction, srcDir, names, dstDir, dstSessionId });

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

/** Side an operation applies to. */
export type Target = { kind: "local" } | { kind: "remote"; sessionId: number };
export type DeleteMethod = "trash" | "shell" | "sftp";

export const fsMkdir = (target: Target, dir: string, name: string) => invoke<void>("fs_mkdir", { target, dir, name });

export const fsRename = (target: Target, dir: string, from: string, to: string) =>
  invoke<void>("fs_rename", { target, dir, from, to });

export const fsMove = (target: Target, srcDir: string, names: string[], dstDir: string) =>
  invoke<void>("fs_move", { target, srcDir, names, dstDir });

export const fsDelete = (target: Target, dir: string, names: string[]) =>
  invoke<DeleteMethod>("fs_delete", { target, dir, names });

export type EditState = "synced" | "uploading" | "conflict" | "error" | "unconfirmed";

export interface EditInfo {
  id: number;
  sessionId: number;
  host: string;
  remotePath: string;
  localPath: string;
  state: EditState;
  uploads: number;
  lastSyncAt: number;
  error: string | null;
}

export interface EditConflict {
  id: number;
  host: string;
  remotePath: string;
  openedSize: number;
  openedModified: number | null;
  currentSize: number;
  currentModified: number | null;
}

export type EditChoice = "overwrite" | "reload" | "keep";

/** `confirmSaves`: the first upload waits for a confirmation (protected connection). */
export const editOpen = (sessionId: number, path: string, confirmSaves: boolean) =>
  invoke<EditInfo>("edit_open", { sessionId, path, confirmSaves });

/** Confirmation asked before the first upload on a protected connection. */
export interface EditConfirm {
  id: number;
  host: string;
  remotePath: string;
  size: number;
  commands: string[];
}

/** Answers a pending confirmation; without one, `allow` uploads changes left unconfirmed. */
export const editConfirmSave = (id: number, allow: boolean) => invoke<void>("edit_confirm_save", { id, allow });

export const onEditConfirm = (handler: (confirm: EditConfirm) => void): Promise<UnlistenFn> =>
  listen<EditConfirm>("edit-confirm", (event) => handler(event.payload));
export const editList = () => invoke<EditInfo[]>("edit_list");
export const editClose = (id: number) => invoke<void>("edit_close", { id });
export const editResolve = (id: number, choice: EditChoice) => invoke<void>("edit_resolve", { id, choice });
export const editReopen = (id: number) => invoke<void>("edit_reopen", { id });
export const editReveal = (id: number) => invoke<void>("edit_reveal", { id });
export const localOpen = (path: string) => invoke<void>("local_open", { path });

export const onEditUpdate = (handler: (info: EditInfo) => void): Promise<UnlistenFn> =>
  listen<EditInfo>("edit-update", (event) => handler(event.payload));

export const onEditConflict = (handler: (conflict: EditConflict) => void): Promise<UnlistenFn> =>
  listen<EditConflict>("edit-conflict", (event) => handler(event.payload));

export const isDirLike = (entry: FileEntry) =>
  entry.kind === "dir" || (entry.kind === "symlink" && entry.targetIsDir);

/** A previewed delete, move or rename (see src-tauri/src/plan.rs). */
export type PlanRequest =
  | { kind: "delete"; dir: string; names: string[] }
  | { kind: "move"; srcDir: string; names: string[]; dstDir: string }
  | { kind: "rename"; dir: string; from: string; to: string };

export type PlanMethod = "trash" | "shell" | "sftp" | "localRename" | "sftpRename";

export interface PlanPreview {
  id: number;
  method: PlanMethod;
  explanation: string;
  /** Commands as they will run (first 20). */
  lines: string[];
  /** Lines not shown (all of them are in the activity log). */
  more: number;
  total: number;
}

export type PlanOutcome = "done" | { deleted: { method: DeleteMethod } };

export const fsPlan = (target: Target, request: PlanRequest) => invoke<PlanPreview>("fs_plan", { target, request });

/** Runs exactly the previewed plan (single use). */
export const fsPlanExecute = (id: number) => invoke<PlanOutcome>("fs_plan_execute", { id });

export const fsPlanDiscard = (id: number) => invoke<void>("fs_plan_discard", { id });
