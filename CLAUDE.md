# SSH File Manager

Open source (MIT) SFTP client with a GUI, macOS first, Linux supported. Tauri 2 + Rust backend, React + TypeScript frontend.

## Core principle

Never parse or reinterpret the user's `~/.ssh/config`. All connections go through the system `ssh` binary (default `/usr/bin/ssh`), so Include/Match/ProxyJump/IdentityAgent work exactly as in the terminal. Nothing is ever installed on servers.

- Connections (`connections.rs`): the app's list in `~/Library/Application Support/sshfilemanager/connections.json` (Linux: `$XDG_CONFIG_HOME/sshfilemanager/`), with categories (color, `protected`). A connection is either an alias of `~/.ssh/config` (only the alias is stored) or configured in the app: those are written as `Host sfm-<id>` blocks to a generated `ssh_config` (0600) ending with `Match all` + `Include ~/.ssh/config` + `Include /etc/ssh/ssh_config`, and ssh runs with `-F <file>`. We write the config, ssh interprets it. Extra options are validated (`Host`/`Match`/`Include` refused) and checked with `ssh -G -F <temp file>` (`connection_check`).
- Host aliases for the import: collected by `ssh_config.rs` (follows `Include`, skips wildcard/negated patterns). Effective settings only via `ssh -G` (`connections::resolve`).
- Connection (`session.rs`): the app owns a ControlMaster (`ssh [-F file] -M -N -f -o ControlPersist=15m -S <sock> -E <log> -- alias`); channels reuse it with `-S <sock> -o ControlMaster=no -o RemoteCommand=none -T`. `SshLauncher.alias` is the ssh target, `label` (the connection name) is what the log, transfers, edits and prompts show.
- SFTP: `ssh -s -- alias sftp`, stdin/stdout handed to `openssh_sftp_client::Sftp::new`. We do NOT use the `openssh` crate (it injects its own options).
- Shell commands are an optional accelerator: `probe()` detects shell access and available commands; SFTP must always work alone (SFTP-only accounts). Remote commands are built only with `sh_command(script, args)` (paths as positional params, `env LC_ALL=C sh -c`), never by string concatenation.
- Only add ssh options that are strictly required (listed above). Any new override needs a justification in a comment.

## Askpass

The app binary is also the `SSH_ASKPASS` helper (`askpass.rs`): when started with `SFM_ASKPASS_SOCKET` it forwards the prompt to the running app over a Unix socket (token-authenticated) and prints the answer. The app emits `askpass-request` to the UI; the UI answers with the `askpass_respond` command. Prompt kinds: `secret`, `confirm`, `notice`, `hostKey`.

Saved passwords (`savePassword` per connection) live in the system keychain (`keyring` crate, service `sshfilemanager`, account = connection id), never in files. `connect` registers an askpass *attempt*: the saved password answers the first password prompt only (no dialog); if ssh asks again the dialog says it was rejected. A password typed with "Salva nel Portachiavi" checked is stored only after the connection succeeded. Passphrases, OTP and confirmations are never saved. Removing a connection (or turning saving off) deletes its keychain item.

Runtime files (sockets, ssh logs) live in a short private dir (`runtime_dir.rs`): `/tmp/sfm-<uid>` or `$XDG_RUNTIME_DIR/sfm`, mode 0700. Socket paths must stay short (~104 bytes on macOS).

## Activity log

Every ssh process, SFTP request, exec command, askpass prompt and local operation goes through `log.rs` (`Logger::start(...)` returns a `LogOp`; finish it with `ok`/`fail`/`result`, dropping it unfinished records an error). Entries carry the exact command line (`shell_line`) and, when useful, a terminal equivalent (`sftp_equivalent`). Ring buffer (default 1000, set from the UI), batched `log-batch` events every 100 ms, rotating text file in `~/Library/Logs/SSH File Manager/` (Linux: `$XDG_STATE_HOME/sshfilemanager/logs`). Never log askpass answers, tokens or the ssh environment. New backend operations must be logged.

Diagnostic mode per connection (UI toggle, stored in localStorage) passes `verbose` to `connect`: the master runs with `-v` and its debug output is attached to the connect entry.

## Transfers

`transfer.rs` (`TransferManager`): a job copies a selection between a local and a remote directory, recursively (symlinks to directories are not followed). Files go to `<name>.sfm-part` and are renamed into place (remote rename uses posix-rename when available); mtime is preserved, permissions only for new files. Each file keeps 16 requests of 256 KiB in flight (`File` clones with `start_seek`); at most 3 files run at once across all jobs. Conflicts are asked one at a time per job (`transfer-conflict` event, answered with `transfer_conflict_respond`, optional apply-to-all). Progress goes to the UI as `transfer-update` every 200 ms. Every scan, mkdir, file and job is logged.

Remote → remote copies (`Direction::Remote`, `transfer_start(..., dstSessionId)`): within one session the server copies (`copy-data` SFTP extension when `support_copy()`, else `cp -p` through the shell), between two sessions the data streams through the app (`relay_to`: reads and writes both pipelined, no local file). Copying onto itself or into its own subtree within one session is refused (`check_same_session_copy`). Conflicts, `.sfm-part`, mtime and progress work as for uploads; the conflict's `host` is the destination. Check: `cargo run --release --example remote_copy -- "$PWD/../.dev-sshd/ssh" segreta <work-dir>` (`SFM_COPY_STRATEGY=shell|stream` forces the fallbacks, test hook).

Benchmark with latency: `scripts/test-sshd.sh start`, then `BIG_MB=50 cargo run --release --example transfer -- "$PWD/../.dev-sshd/ssh" sfm-slow segreta <work-dir>` (`sfm-slow` adds 20 ms each way through `scripts/delay-relay.py`). Reference: ~17 MiB/s pipelined vs ~5 MiB/s sequential; OpenSSH `sftp` ~22 MiB/s on the same path.

UI triggers: button Carica/Scarica and F5 in the active pane (from the local pane: the remote pane used last; ⇧F5 and the context menu list every destination), drag & drop between any two panes (onto a folder row = into that folder; between remote panes it copies, never moves; ⇧F5 in a remote pane offers "Copia su <altro>"), files dropped from the Finder on a remote pane (Tauri `onDragDropEvent`, pane found through `data-pane`). Pane-to-pane drags do NOT use HTML5 drag & drop: in the native macOS window Tauri's drag-drop handler claims every drag, so `dragover`/`drop` never reach the page (it works in Chromium, which hides the bug). `pointerDrag.ts` tracks the pointer instead (`beginPointerDrag` on row `onPointerDown`, drop zones registered per pane, hit test via `data-pane` / `data-entry` / `data-dir`, `DragGhost` label). Playwright checks must drag with `page.mouse` and block HTML5 drag events in the init script. Progress is shown in the bottom panel's Trasferimenti tab, with the host's category color and a host filter.

## Panes

The local pane is always on the left; on the right up to two remote panes (slots `r0`, `r1`, see `dnd.ts` `Side`), managed by `useRemoteSlots.ts`. Opening a connection uses a free slot, otherwise a dialog asks which one to replace. The same connection in both slots shares one session (no new connect; "⧉ Apri nell'altro pannello" opens the current folder there); a session is disconnected only when no slot uses it. Layout toggle in the toolbar: remotes side by side or stacked (localStorage `layout.remotes`); dividers via `SplitView` (sizes in localStorage `split.*`, double click = equal parts). Window minimum 900×560. Panes are cards on the window background (6 px gaps, which are the dividers): the local pane is neutral, remote panes take the category color for the whole border and a `color-mix` tint for background (6%) and header (14%, 24% when active); the active pane has a thicker border. `color-scheme: light dark` keeps native scrollbars in theme.

## Previewed operations and protected connections

`plan.rs` (`Planner`): deletes (always) and, on protected connections, renames and moves are first planned with `fs_plan(target, request)`; the preview lists the exact commands (Trash `mv`, the `rm -rf` exec line, every SFTP `rm`/`rmdir`, `sftp> rename` plus the `mv` fallback) and the full list goes to the log. `fs_plan_execute(id)` runs exactly that plan (single use, 10 min TTL): an SFTP delete runs only the listed requests, so files added after the preview make it fail instead of being deleted unseen. `fs_plan_discard` drops it. Check: the `fileops` example covers both account types.

Protected categories (`Category.protected`) require typing the connection name (`NameGate`, `confirmationMatches` in `connections.ts`) for: delete, rename, move (`PlanDialog`), overwrite in an upload conflict including "Sovrascrivi tutti" (`ConflictDialog`, commands from `ConflictRequest.commands`), and the first upload of each edited file (`edit_open(..., confirmSaves)`, `edit-confirm` event, `edit_confirm_save`; state `unconfirmed` when refused, "Carica sul server" asks again). "Non chiedere più per questo file" is stored in localStorage `edits.noConfirm` (`connectionId:path`). Non-protected confirmations show the same commands collapsed. Uploads of new files and downloads are never gated.

## File operations

`fileops.rs`, commands `fs_mkdir`/`fs_rename`/`fs_move`/`fs_delete` with a `Target` (`{kind:"local"}` or `{kind:"remote", sessionId}`). Names are validated (`validate_name`: no `/`, `.`, `..`, empty). Rename and move never overwrite (destination checked first). Local delete = Trash (`trash` crate, NSFileManager method: no Finder permission prompt). Remote delete is permanent: `rm -rf -- "$@"` via `sh_command` when the probe found `rm`, recursive SFTP walk otherwise (symlinks removed, never followed); `/` and the home dir are refused. Remote move falls back to `mv` when SFTP rename fails (e.g. across filesystems). Check against the fixture: `cargo run --example fileops -- "$PWD/../.dev-sshd/ssh" sfm-local segreta <work-dir>` (and `sfm-sftponly`).

UI: context menu, F2 inline rename, F7/⇧⌘N new folder, ⌘⌫/Delete delete with confirmation, "Sposta in…", drag onto a folder of the same pane = move.

## Editing remote files

`edit.rs` (`EditManager`): `edit_open` downloads into `~/Library/Caches/sshfilemanager/edit/<hash>/<name>` (max 100 MB), opens it with `open`/`xdg-open` and watches the directory with `notify` (editors save atomically). After a 500 ms debounce, a content change (SHA-256) is uploaded by writing the remote file in place (owner/mode preserved). Before uploading, the remote file is compared with the opened version (size, mtime, and SHA-256 via `sha256sum` when the probe found it): on mismatch an `edit-conflict` asks overwrite / reload (local copy kept as "<name> (mia versione)") / keep. Local copies are removed on close, on disconnect, and on exit only when synced. Check: `cargo run --example edit -- "$PWD/../.dev-sshd/ssh" sfm-local segreta <work-dir>`.

UI: double click / Enter on a remote file = edit, on a local file = open with the default app; the bottom panel's "File aperti" tab lists open files.

## Layout

Backend (`src-tauri/src/`):
- `lib.rs` — Tauri state, commands, exit cleanup
- `connections.rs` (store, categories, generated ssh_config, `ssh -G`, keychain), `ssh_config.rs` (host aliases for the import), `session.rs` (master, channels, probe, `sh_command`), `askpass.rs`, `runtime_dir.rs`, `error.rs`
- `log.rs` — activity log (ring buffer, events, rotating file)
- `entry.rs` (`FileEntry`, shared by both panes), `local.rs` (local listing)
- `transfer.rs`, `fileops.rs`, `edit.rs`, `plan.rs` — see the sections above
- `src-tauri/examples/` — headless checks against the fixture: `spike.rs` (connect/probe/list/throughput), `transfer.rs`, `remote_copy.rs`, `fileops.rs`, `edit.rs`, `connections.rs` (`-F`, ProxyJump between app connections, `ssh -G` errors, keychain round trip: `cargo run --example connections -- "$PWD/../.dev-sshd" segreta <work-dir>`)

Frontend (`src/`):
- `api.ts` — typed wrappers for every Tauri command/event (keep in sync with `lib.rs`)
- `App.tsx` — connection state, wiring between toolbar, panes, transfers, edits and dialogs
- `components/` — React function components only:
  - `Toolbar` (Nuovo collegamento ⌘K, Gestisci connessioni, ⌘J), `QuickConnect` (search palette), `ConnectionManager` (modal: list, form, import from ~/.ssh/config, categories, import/export file)
  - `FilePane` (generic virtualized pane: keyboard, context menu, inline rename, drag & drop with `acceptsFrom`, `transferTargets` for ⇧F5; `color` = category border), `RemotePane` (name + category badge, open in other pane, close), `SplitView` (draggable dividers), `ContextMenu`
  - `BottomPanel` (tabs Log / Trasferimenti / File aperti, ⌘J), `TransfersView`, `EditsView`
  - dialogs: `AskpassDialog`, `ConflictDialog` (transfers), `EditConflictDialog`, `EditConfirmDialog`, `PlanDialog` (previewed operations), `Protection` (`ProtectionBanner`, `NameGate`, `CommandPreview`), `Dialogs` (`PromptDialog`, `ConfirmDialog`, `ChooseDialog`, `nameError`)
- hooks: `usePane.ts` (path, entries, sort, hidden files, selection), `useRemoteSlots.ts` (the two remote slots, shared sessions), `useConnections.ts`, `useActivityLog.ts`, `useTransfers.ts`, `useEdits.ts`
- `pointerDrag.ts` — pane-to-pane drag & drop with pointer events (see Transfers)
- pure modules (tested with vitest): `connections.ts` (search, import, merge, validation mirroring the backend), `fileList.ts` (sorting/selection), `logStore.ts` (log merge/filter), `dnd.ts` (drag payload, grouping by parent)
- `paths.ts` (remote POSIX paths), `format.ts`, `settings.ts` (`usePersistentState`, localStorage with safe fallbacks)

Other:
- `scripts/test-sshd.sh` — local unprivileged sshd fixture (hosts `sfm-local`, `sfm-sftponly`, `sfm-rc`, `sfm-slow`); `scripts/delay-relay.py` adds latency for `sfm-slow`
- `docs/architecture.md` — design decisions and their reasons; `docs/roadmap.md` — future work; `docs/connections.md` — design of connection management and multiple remote panes (all four phases implemented)

## Commands

```sh
pnpm install
pnpm tauri dev                     # run the app
pnpm build                         # tsc + vite build (frontend type-check)
pnpm test                          # vitest (pure TS modules)
cd src-tauri && cargo test && cargo clippy --all-targets
scripts/test-sshd.sh start         # test servers on 127.0.0.1:2222/2223
cd src-tauri && cargo run --release --example spike -- "$PWD/../.dev-sshd/ssh" sfm-local segreta [remote-file]
scripts/test-sshd.sh stop
```

## Conventions

- Rust: errors through `AppError` (serialized as a string to the UI); async with Tokio via `tauri::async_runtime`; Unix-only code is fine (Windows is out of scope for now).
- TypeScript strict; function components and hooks only; domain logic in plain TS modules, not in components.
- UI text in Italian for now; code, comments and identifiers in English.
- Monospace font everywhere (`--mono` in `App.css`): values must line up across panes.
- Diagnostic mode is per connection id (localStorage `connections.verbose`), toggled in the connection form.
- UI checks without the native window: run `pnpm dev` and open http://localhost:1420 with Playwright, mocking `window.__TAURI_INTERNALS__` (`invoke`, `transformCallback`, `metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } }`) and `window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} }` via `page.addInitScript`. Events are delivered by calling the callback registered through `plugin:event|listen`.

## Roadmap

`docs/roadmap.md` (Italian) tracks future work. When a new feature or improvement is proposed (by the user or as a suggestion at the end of a phase), add it there with the date and a short motivation; when one is done, move it to "Fatto" with the commit. Out-of-scope items for the MVP are listed there too.

When a design decision is taken or changed, update `docs/architecture.md`.
