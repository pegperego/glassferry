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
  uid: number | null;
  gid: number | null;
  /** Names of `uid`/`gid` when known (remote: from the server's /etc/passwd and /etc/group). */
  owner: string | null;
  group: string | null;
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
  bookmarks: Bookmark[];
}

/** A folder to go back to, on a connection or local. */
export interface Bookmark {
  id: string;
  name: string;
  /** null: a local folder. */
  connectionId: string | null;
  /** Absolute path. */
  path: string;
}

export const listHosts = () => invoke<HostEntry[]>("list_hosts");

export interface AppInfo {
  name: string;
  version: string;
  /** Short hash and date of the commit built; null outside a git checkout. */
  commit: string | null;
  commitDate: string | null;
  os: string;
  arch: string;
  repository: string;
  license: string;
  licenseText: string;
}

export const appInfo = () => invoke<AppInfo>("app_info");
export const openRepository = () => invoke<void>("open_repository");

/** macOS menu items handled by the UI: "about", "settings". */
export const onMenu = (handler: (id: string) => void): Promise<UnlistenFn> =>
  listen<string>("menu", (event) => handler(event.payload));

/** Settings the backend needs (`settings.json`); UI preferences stay in localStorage. */
export interface Settings {
  /** Absolute path; null: /usr/bin/ssh. */
  sshBin: string | null;
  terminal: TerminalSettings;
}

/** A terminal the app knows, installed on this machine. */
export interface TerminalApp {
  id: string;
  name: string;
}

export const terminalList = () => invoke<TerminalApp[]>("terminal_list");

/** How "Apri nel terminale" opens the terminal. */
export interface TerminalSettings {
  /** Id of an installed terminal (`terminalList`); null: the default. */
  app: string | null;
  /** Command run with `sh -c`, `{script}` replaced by the script path; overrides `app`. */
  command: string | null;
}

export interface SshCheck {
  /** First line of `ssh -V`. */
  version: string;
  /** OpenSSH 8.4 or newer (needed for the in-app prompts). */
  supported: boolean;
}

export const settingsLoad = () => invoke<Settings>("settings_load");

/** Validates and saves; returns the normalized settings. */
export const settingsSave = (settings: Settings) => invoke<Settings>("settings_save", { settings });

/** `ssh -V` of `path`, or of the configured binary when null. */
export const sshCheck = (path: string | null) => invoke<SshCheck>("ssh_check", { path });

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
  /** Remote side; the source of a remote copy. Null for a job restored after a restart. */
  sessionId: number | null;
  connectionId: string;
  host: string;
  /** Destination of a remote copy. */
  dstSessionId: number | null;
  dstConnectionId: string | null;
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
  /** Failed or cancelled: `transferResume` continues it. */
  resumable: boolean;
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

/** Resumes an interrupted job on open sessions of its connections; returns the new job id. */
export const transferResume = (jobId: number, sessionId: number, dstSessionId: number | null) =>
  invoke<number>("transfer_resume", { jobId, sessionId, dstSessionId });

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

/** chmod / chown / chgrp; null fields stay unchanged. */
export interface AttrChange {
  mode: number | null;
  owner: string | null;
  group: string | null;
  /** Also the content of the selected folders. */
  recursive: boolean;
}

/** `dryRun`: only the commands that would run (preview). */
export const fsAttrs = (target: Target, dir: string, names: string[], change: AttrChange, dryRun: boolean) =>
  invoke<string[]>("fs_attrs", { target, dir, names, change, dryRun });

export interface DirSize {
  name: string;
  bytes: number;
  /** Disk usage (`du -k`) rather than the sum of file sizes. */
  approx: boolean;
  /** Some content could not be read. */
  partial: boolean;
}

/** Sizes of folders in `dir`: du on the server, or a walk. */
export const fsDirSize = (target: Target, dir: string, names: string[]) =>
  invoke<DirSize[]>("fs_dir_size", { target, dir, names });

export type SearchKind = "any" | "file" | "dir";

export interface SearchQuery {
  /** Part of the name, or a pattern with `*` and `?`. */
  text: string;
  kind: SearchKind;
}

export interface SearchResult {
  matches: { path: string; dir: boolean }[];
  /** More than 1000 matches, or the walk stopped at its folder limit. */
  truncated: boolean;
  /** Some folders could not be read. */
  partial: boolean;
  method: "find" | "sftp" | "local";
}

/** Names matching below `dir`; `searchId` (chosen by the caller) cancels it. */
export const fsSearch = (target: Target, dir: string, query: SearchQuery, searchId: number) =>
  invoke<SearchResult>("fs_search", { target, dir, query, searchId });

export const fsSearchCancel = (searchId: number) => invoke<void>("fs_search_cancel", { searchId });

/** Compresses `names` into `dir/archive` (.tar.gz); never overwrites. */
export const fsCompress = (target: Target, dir: string, names: string[], archive: string) =>
  invoke<void>("fs_compress", { target, dir, names, archive });

/** Extracts `dir/name` into a new folder named after it; returns that folder's name. */
export const fsExtract = (target: Target, dir: string, name: string) => invoke<string>("fs_extract", { target, dir, name });

/** Opens the system terminal in `path` (remote: a new ssh connection that changes to it). */
export const openTerminal = (target: Target, path: string) => invoke<void>("open_terminal", { target, path });

export const fsRename = (target: Target, dir: string, from: string, to: string) =>
  invoke<void>("fs_rename", { target, dir, from, to });

export const fsMove = (target: Target, srcDir: string, names: string[], dstDir: string) =>
  invoke<void>("fs_move", { target, srcDir, names, dstDir });

export const fsDelete = (target: Target, dir: string, names: string[]) =>
  invoke<DeleteMethod>("fs_delete", { target, dir, names });

/** `detached`: changes kept while the connection is closed, uploaded when it opens again. */
export type EditState = "synced" | "uploading" | "conflict" | "error" | "unconfirmed" | "detached";

export interface EditInfo {
  id: number;
  /** Null while detached. */
  sessionId: number | null;
  connectionId: string;
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
