# Glassferry

Open source (MIT) SFTP client with a GUI, macOS first, Linux supported. Tauri 2 + Rust backend, React + TypeScript frontend.

## Core principle

Never parse or reinterpret the user's `~/.ssh/config`. All connections go through the system `ssh` binary (default `/usr/bin/ssh`, configurable in Impostazioni), so Include/Match/ProxyJump/IdentityAgent work exactly as in the terminal. Nothing is ever installed on servers.

- Connections (`connections.rs`): the app's list in `~/Library/Application Support/glassferry/connections.json` (Linux: `$XDG_CONFIG_HOME/glassferry/`), with categories (color, `protected`). A connection is either an alias of `~/.ssh/config` (only the alias is stored) or configured in the app: those are written as `Host gf-<id>` blocks to a generated `ssh_config` (0600) ending with `Match all` + `Include ~/.ssh/config` + `Include /etc/ssh/ssh_config`, and ssh runs with `-F <file>`. We write the config, ssh interprets it. Extra options are validated (`Host`/`Match`/`Include` refused) and checked with `ssh -G -F <temp file>` (`connection_check`).
- Host aliases for the import: collected by `ssh_config.rs` (follows `Include`, skips wildcard/negated patterns). Effective settings only via `ssh -G` (`connections::resolve`).
- Connection (`session.rs`): the app owns a ControlMaster (`ssh [-F file] -M -N -f -o ControlPersist=15m -S <sock> -E <log> -- alias`); channels reuse it with `-S <sock> -o ControlMaster=no -o RemoteCommand=none -T`. `SshLauncher.alias` is the ssh target, `label` (the connection name) is what the log, transfers, edits and prompts show.
- SFTP: `ssh -s -- alias sftp`, stdin/stdout handed to `openssh_sftp_client::Sftp::new`. We do NOT use the `openssh` crate (it injects its own options).
- Shell commands are an optional accelerator: `probe()` detects shell access and available commands; SFTP must always work alone (SFTP-only accounts). Remote commands are built only with `sh_command(script, args)` (paths as positional params, `env LC_ALL=C sh -c`), never by string concatenation.
- Only add ssh options that are strictly required (listed above). Any new override needs a justification in a comment.

## Askpass

The app binary is also the `SSH_ASKPASS` helper (`askpass.rs`): when started with `GF_ASKPASS_SOCKET` it forwards the prompt to the running app over a Unix socket (token-authenticated) and prints the answer. The app emits `askpass-request` to the UI; the UI answers with the `askpass_respond` command. Prompt kinds: `secret`, `confirm`, `notice`, `hostKey`.

Saved passwords (`savePassword` per connection) live in the system keychain (`keyring` crate, service `glassferry`, account = connection id), never in files. `connect` registers an askpass *attempt*: the saved password answers the first password prompt only (no dialog); if ssh asks again the dialog says it was rejected. A password typed with "Salva nel Portachiavi" checked is stored only after the connection succeeded. Passphrases, OTP and confirmations are never saved. Removing a connection (or turning saving off) deletes its keychain item.

Runtime files (sockets, ssh logs) live in a short private dir (`runtime_dir.rs`): `/tmp/gf-<uid>` or `$XDG_RUNTIME_DIR/gf`, mode 0700. Socket paths must stay short (~104 bytes on macOS).

## Activity log

Every ssh process, SFTP request, exec command, askpass prompt and local operation goes through `log.rs` (`Logger::start(...)` returns a `LogOp`; finish it with `ok`/`fail`/`result`, dropping it unfinished records an error). Entries carry the exact command line (`shell_line`) and, when useful, a terminal equivalent (`sftp_equivalent`). Ring buffer (default 1000, set from the UI), batched `log-batch` events every 100 ms, rotating text file in `~/Library/Logs/Glassferry/` (Linux: `$XDG_STATE_HOME/glassferry/logs`). Never log askpass answers, tokens or the ssh environment. New backend operations must be logged.

Diagnostic mode per connection (UI toggle, stored in localStorage) passes `verbose` to `connect`: the master runs with `-v` and its debug output is attached to the connect entry.

## Transfers

`transfer.rs` (`TransferManager`): a job copies a selection between a local and a remote directory, recursively (symlinks to directories are not followed). Files go to `<name>.gf-part` and are renamed into place (remote rename uses posix-rename when available); mtime is preserved, permissions only for new files. Each file keeps 16 requests of 256 KiB in flight (`File` clones with `start_seek`); at most 3 files run at once across all jobs. Conflicts are asked one at a time per job (`transfer-conflict` event, answered with `transfer_conflict_respond`, optional apply-to-all). Progress goes to the UI as `transfer-update` every 200 ms. Every scan, mkdir, file and job is logged.

Resume (`transfer_resume(jobId, sessionId, dstSessionId)`, `TransferManager::resume`): a failed or cancelled job (`resumable`) runs again as a new job on open sessions of the same connections (`Endpoint` = session + connection id; jobs store `connectionId`/`dstConnectionId`). A failed file keeps its `.gf-part` with a `PartPoint` (bytes known to be written + source size and mtime): downloads write sequentially, uploads and streamed copies track the contiguous prefix (`WrittenPrefix`, writes complete out of order); nothing written → the part is removed. On resume, destinations matching the source (size + mtime) count as done (`resume-skip`), a part continues from `min(point, part length)` if the source is unchanged (log "ripreso da", equivalent `reget`/`reput`), else the file restarts; server-side copies (`copy-data`, `cp -p`) always restart. Unfinished and interrupted jobs are kept in `<config dir>/transfers.json` (rewritten on start, end, resume, clear): after a restart they come back as `failed` with no session ("interrotto: l'app è stata chiusa"). `clear_finished` removes their part files (remote ones only while the session is open). UI: "Riprendi" in the Trasferimenti tab, or "Connetti <host>" first when a connection is not open (`sessionFor` in `App.tsx`). Check: `cargo run --release --example resume -- "$PWD/../.dev-sshd/ssh" gf-slow segreta <work-dir>` (cancel midway, dropped connection, changed source, restart from the manifest, cleanup).

Remote → remote copies (`Direction::Remote`, `transfer_start(..., dstSessionId)`): within one session the server copies (`copy-data` SFTP extension when `support_copy()`, else `cp -p` through the shell), between two sessions the data streams through the app (`relay_to`: reads and writes both pipelined, no local file). Copying onto itself or into its own subtree within one session is refused (`check_same_session_copy`). Conflicts, `.gf-part`, mtime and progress work as for uploads; the conflict's `host` is the destination. Check: `cargo run --release --example remote_copy -- "$PWD/../.dev-sshd/ssh" segreta <work-dir>` (`GF_COPY_STRATEGY=shell|stream` forces the fallbacks, test hook).

Benchmark with latency: `scripts/test-sshd.sh start`, then `BIG_MB=50 cargo run --release --example transfer -- "$PWD/../.dev-sshd/ssh" gf-slow segreta <work-dir>` (`gf-slow` adds 20 ms each way through `scripts/delay-relay.py`). Reference: ~17 MiB/s pipelined vs ~5 MiB/s sequential; OpenSSH `sftp` ~22 MiB/s on the same path.

UI triggers: button Carica/Scarica and F5 in the active pane (from the local pane: the remote pane used last; ⇧F5 and the context menu list every destination), drag & drop between any two panes (onto a folder row = into that folder; between remote panes it copies, never moves; ⇧F5 in a remote pane offers "Copia su <altro>"), files dropped from the Finder on a remote pane (Tauri `onDragDropEvent`, pane found through `data-pane`). Pane-to-pane drags do NOT use HTML5 drag & drop: in the native macOS window Tauri's drag-drop handler claims every drag, so `dragover`/`drop` never reach the page (it works in Chromium, which hides the bug). `pointerDrag.ts` tracks the pointer instead (`beginPointerDrag` on row `onPointerDown`, drop zones registered per pane, hit test via `data-pane` / `data-entry` / `data-dir`, `DragGhost` label). Playwright checks must drag with `page.mouse` and block HTML5 drag events in the init script. Progress is shown in the bottom panel's Trasferimenti tab, with the host's category color and a host filter.

## Panes

The local pane is always on the left; on the right up to two remote panes (slots `r0`, `r1`, see `dnd.ts` `Side`), managed by `useRemoteSlots.ts`. Opening a connection uses a free slot, otherwise a dialog asks which one to replace. The same connection in both slots shares one session (no new connect; "⧉ Apri nell'altro pannello" opens the current folder there); a session is disconnected only when no slot uses it. Layout toggle in the toolbar: remotes side by side or stacked (localStorage `layout.remotes`); dividers via `SplitView` (sizes in localStorage `split.*`, double click = equal parts). Window minimum 900×560. Panes are cards on the window background (6 px gaps, which are the dividers): the local pane is neutral, remote panes take the category color for the whole border and a `color-mix` tint for background (6%) and header (14%, 24% when active); the active pane has a thicker border. `color-scheme: light dark` keeps native scrollbars in theme.

## Previewed operations and protected connections

`plan.rs` (`Planner`): deletes (always) and, on protected connections, renames and moves are first planned with `fs_plan(target, request)`; the preview lists the exact commands (Trash `mv`, the `rm -rf` exec line, every SFTP `rm`/`rmdir`, `sftp> rename` plus the `mv` fallback) and the full list goes to the log. `fs_plan_execute(id)` runs exactly that plan (single use, 10 min TTL): an SFTP delete runs only the listed requests, so files added after the preview make it fail instead of being deleted unseen. `fs_plan_discard` drops it. Check: the `fileops` example covers both account types.

Protected categories (`Category.protected`) require typing the connection name (`NameGate`, `confirmationMatches` in `connections.ts`) for: delete, rename, move (`PlanDialog`), permissions and owner (`PermissionsDialog`), overwrite in an upload conflict including "Sovrascrivi tutti" (`ConflictDialog`, commands from `ConflictRequest.commands`), and the first upload of each edited file (`edit_open(..., confirmSaves)`, `edit-confirm` event, `edit_confirm_save`; state `unconfirmed` when refused, "Carica sul server" asks again). "Non chiedere più per questo file" is stored in localStorage `edits.noConfirm` (`connectionId:path`). Non-protected confirmations show the same commands collapsed. Uploads of new files and downloads are never gated.

## File operations

`fileops.rs`, commands `fs_mkdir`/`fs_rename`/`fs_move`/`fs_delete` with a `Target` (`{kind:"local"}` or `{kind:"remote", sessionId}`). Names are validated (`validate_name`: no `/`, `.`, `..`, empty). Rename and move never overwrite (destination checked first). Local delete = Trash (`trash` crate, NSFileManager method: no Finder permission prompt). Remote delete is permanent: `rm -rf -- "$@"` via `sh_command` when the probe found `rm`, recursive SFTP walk otherwise (symlinks removed, never followed); `/` and the home dir are refused. Remote move falls back to `mv` when SFTP rename fails (e.g. across filesystems). Check against the fixture: `cargo run --example fileops -- "$PWD/../.dev-sshd/ssh" gf-local segreta <work-dir>` (and `gf-sftponly`).

Owner and group: `FileEntry` carries `uid`/`gid` and, when known, `owner`/`group` names (`ids.rs`): remote from the server's `/etc/passwd` and `/etc/group`, read over SFTP once per session (unreadable or LDAP users: numbers); local from the system database (`getpwuid`/`getgrgid`, cached). Column "Proprietario" (`owner:group`, hidden in narrow panes).

Permissions (`attrs.rs`, command `fs_attrs(target, dir, names, change, dryRun)`, dialog `PermissionsDialog` from "Permessi…"): mode grid + octal (special bits kept), optional recursion, owner/group prefilled with the current values and sent only when changed (editable on remote accounts with `chown`/`chgrp`, read-only otherwise). Remote: `chmod|chown|chgrp [-R] -- <operand> "$@"` through `sh_command` when the probe found the command (`--` before the operand: BSD chmod takes anything after the mode as a file), else SFTP setstat walking the tree without following symlinks (children listed before their folder changes); owner/group need the shell (SFTP v3 knows numeric ids only). Local: mode only, with std. `dryRun` returns exactly the commands `apply` runs (preview); protected connections ask for the name. Check: the `fileops` example (both accounts).

Folder size (`dirsize.rs`, command `fs_dir_size(target, dir, names)`, "Calcola dimensione" in the context menu, shown in the Size column of the folder shown, reset when it changes): remote with `du`: `du -sb` on Linux (apparent bytes), else or as fallback `du -sk` (disk usage, `approx`, shown with `~`); without a shell and locally a walk adding file sizes, symlinks not followed; unreadable folders give `partial` (`+`). Check: the `fileops` example.

Search (`search.rs`, commands `fs_search(target, dir, query, searchId)` / `fs_search_cancel`, `SearchDialog` from ⌘F or ⌕ in a pane): by name below the pane's folder, `*`/`?` patterns (plain text = `*text*`), case-insensitive, files/folders/both, at most 1000 results. Remote with `find`: `find "$1" -xdev` (one filesystem), folders printed with a trailing `/` (`-exec printf`, no GNU `-printf`), cut with `head`, 2 min timeout, cancellation drops the exec (ssh killed). Without a shell (SFTP) and locally: a breadth-first walk with the same rules (symlinks not followed, same device, at most 20,000 folders). A result opens its folder, with the cursor on the file. Check: the `fileops` example.

UI: context menu, F2 inline rename, F7/⇧⌘N new folder, ⌥⌘T terminal, ⌘D bookmark, ⌘F search, ⌘⌫/Delete delete with confirmation, "Sposta in…", drag onto a folder of the same pane = move.

## Editing remote files

`edit.rs` (`EditManager`): `edit_open` downloads into `~/Library/Caches/glassferry/edit/<hash>/<name>` (max 100 MB), opens it with `open`/`xdg-open` and watches the directory with `notify` (editors save atomically). After a 500 ms debounce, a content change (SHA-256) is uploaded to `<name>.gf-part` with the file's mode and renamed over it with posix-rename (never partial); it is written in place instead (truncate + write, `write_replacing` returns the reason, logged) when the path is a symlink, the new file would get another owner or group, the server lacks posix-rename or the folder is not writable. Before uploading, the remote file is compared with the opened version: size and mtime, then, when those match (SFTP v3 mtime has 1 s resolution), the SHA-256 of its content (`remote_hash`: `sha256sum` when the probe found it, else the file read over SFTP, logged as `edit-check`): on mismatch an `edit-conflict` asks overwrite / reload (local copy kept as "<name> (mia versione)") / keep. Local changes are never dropped: on disconnect (`close_session`) and on exit (`close_unchanged`) only copies without local changes (content hash = last synced) are removed; the others are *detached* (state `detached`, `sessionId` null, no watcher) and reattached by `connect` for the same connection id (`attach`: new watcher, then a sync with the usual conflict check and confirmation). Open files are recorded in `<cache>/edit/edits.json` on every change, so detached copies come back after a restart or a crash. Sessions map to their connection id in `AppState.sessions` (`OpenSession`). The UI asks before closing a pane that would detach files (`guardClose` in `App.tsx`); detached rows offer "Riconnetti". Check: `cargo run --example edit -- "$PWD/../.dev-sshd/ssh" gf-local segreta <work-dir>` (and `gf-sftponly`).

UI: double click / Enter on a remote file = edit, on a local file = open with the default app; the bottom panel's "File aperti" tab lists open files.

## Bookmarks

`Store.bookmarks` in `connections.json` (`Bookmark { id, name, connectionId | null = local, path }`, validated by the backend: connection must exist, absolute path), so they export/import with the connections (`mergeStores` adds those of known connections) and go away with their connection (`removeConnection`). Logic in `connections.ts` (`addBookmark` named after the folder, once per folder; `bookmarksOf`, `findBookmark`, `removeBookmark`, `searchBookmarks`). UI: ☆/★ button in each pane header (menu: add/remove this folder, list → navigate), ⌘D toggles the current folder; the ⌘K palette lists bookmarks after the connections (all, or those matching the search) and opens them (`openBookmark` in `App.tsx`: navigates the pane already showing that connection, else opens it at that folder).

## Terminal

`terminal.rs`, command `open_terminal(target, path)`: ⌥⌘T in a pane (current folder) or "Apri nel terminale" in the context menu (a folder, or the current one). Local: the login shell starts in the folder. Remote: a new, independent `ssh [-F file] -t -o RemoteCommand=none -- alias '<cd + exec $SHELL -l>'` (path as a positional parameter of `sh -c`), authenticated in the terminal, not through the app's ControlMaster (closing the pane would close it); not offered on SFTP-only accounts (`PaneOps.openTerminal` null). The terminal runs a one-shot script in the runtime dir that deletes itself. Known terminals (`KNOWN` in `terminal.rs`) are detected where installed (`terminal_list`: macOS app bundles in /Applications, ~/Applications, /System/Applications/Utilities; Linux commands in the PATH) and started their own way: `open -a` for Terminal and iTerm2 (they run a `.command` file, no automation permission), `open -na Ghostty.app --args -e`, the bundled binary for WezTerm/kitty/Alacritty, the command with its flag on Linux. Settings store the terminal id (older names like "iTerm" still match); default Terminal on macOS, `$TERMINAL` or the first found on Linux; any other terminal through a custom command (`{script}` placeholder, run with `sh -c`). Buttons: `>_` in each pane header, ⌥⌘T, context menu.

## About

"Informazioni su Glassferry" (`AboutDialog`): from the macOS app menu (custom menu bar in `lib.rs` `app_menu`: Tauri's default menus in Italian plus "Informazioni su Glassferry", "Impostazioni… ⌘,", Aiuto → "Glassferry su GitHub"; items handled by the UI arrive as the `menu` event) and from the Impostazioni dialog (Linux has no app menu). Shows version, commit and its date (`build.rs` → `GF_COMMIT`, `GF_COMMIT_DATE`), system, `ssh -V`, the MIT license (`LICENSE`, embedded), the repository (`open_repository`), "Copia informazioni" for bug reports, and the third-party components: `scripts/third-party.mjs` (run by `pnpm build`, or `pnpm third-party`) writes `src/generated/third-party.json` from `cargo tree -e normal` for the shipped targets + `cargo metadata` (license files of each crate) and `pnpm licenses list --prod`; texts deduplicated, loaded on demand as a separate chunk. Commit the regenerated file when dependencies change.

## Layout

Backend (`src-tauri/src/`):
- `lib.rs` — Tauri state, commands, exit cleanup
- `connections.rs` (store, categories, generated ssh_config, `ssh -G`, keychain), `ssh_config.rs` (host aliases for the import), `session.rs` (master, channels, probe, `sh_command`), `askpass.rs`, `runtime_dir.rs`, `error.rs`
- `settings.rs` — settings the backend needs (`settings.json` in the config dir: ssh binary, read at each connect via `AppState::ssh_bin()`; `ssh_check` runs `ssh -V`, OpenSSH ≥ 8.4); commands `settings_load`/`settings_save`
- `terminal.rs` — open the system terminal at a folder (see Terminal)
- `ids.rs` — owner/group names (see File operations)
- `log.rs` — activity log (ring buffer, events, rotating file)
- `entry.rs` (`FileEntry`, shared by both panes), `local.rs` (local listing)
- `transfer.rs`, `fileops.rs`, `attrs.rs`, `dirsize.rs`, `search.rs`, `edit.rs`, `plan.rs` — see the sections above
- `src-tauri/examples/` — headless checks against the fixture: `spike.rs` (connect/probe/list/throughput), `transfer.rs`, `remote_copy.rs`, `resume.rs`, `fileops.rs`, `edit.rs`, `connections.rs` (`-F`, ProxyJump between app connections, `ssh -G` errors, keychain round trip: `cargo run --example connections -- "$PWD/../.dev-sshd" segreta <work-dir>`)

Frontend (`src/`):
- `api.ts` — typed wrappers for every Tauri command/event (keep in sync with `lib.rs`)
- `App.tsx` — connection state, wiring between toolbar, panes, transfers, edits and dialogs
- `components/` — React function components only:
  - `Toolbar` (Nuovo collegamento ⌘K, Gestisci connessioni, ⌘J, ⚙ Impostazioni ⌘,), `SettingsDialog`, `AboutDialog`, `QuickConnect` (search palette), `ConnectionManager` (modal: list, form, import from ~/.ssh/config, categories, import/export file)
  - `FilePane` (generic virtualized pane: keyboard, context menu, inline rename, drag & drop with `acceptsFrom`, `transferTargets` for ⇧F5; `color` = category border), `RemotePane` (name + category badge, open in other pane, close), `SplitView` (draggable dividers), `ContextMenu`
  - `BottomPanel` (tabs Log / Trasferimenti / File aperti, ⌘J), `TransfersView`, `EditsView`
  - dialogs: `AskpassDialog`, `ConflictDialog` (transfers), `EditConflictDialog`, `EditConfirmDialog`, `PlanDialog` (previewed operations), `Protection` (`ProtectionBanner`, `NameGate`, `CommandPreview`), `Dialogs` (`PromptDialog`, `ConfirmDialog`, `ChooseDialog`, `nameError`)
- hooks: `usePane.ts` (path, entries, sort, hidden files, selection), `useRemoteSlots.ts` (the two remote slots, shared sessions), `useConnections.ts`, `useActivityLog.ts`, `useTransfers.ts`, `useEdits.ts`
- `pointerDrag.ts` — pane-to-pane drag & drop with pointer events (see Transfers)
- pure modules (tested with vitest): `connections.ts` (search, import, merge, validation mirroring the backend), `fileList.ts` (sorting/selection), `logStore.ts` (log merge/filter), `dnd.ts` (drag payload, grouping by parent)
- `paths.ts` (remote POSIX paths), `format.ts`, `settings.ts` (`usePersistentState`, localStorage with safe fallbacks)

Other:
- `src-tauri/icons/icon.svg` — icon source; regenerate every size with `pnpm tauri icon <1024 px png>` (drop the android/ios outputs)
- `.github/workflows/` — `ci.yml` (build, vitest, cargo test, clippy `-D warnings` on macOS and Ubuntu 22.04), `release.yml` (tag `vX.Y.Z` = version in `tauri.conf.json`, `package.json` and `Cargo.toml`, release text = that version's section of `CHANGELOG.md` → draft release with `.dmg` arm64/x64, AppImage, `.deb`, `.rpm` for Linux x64 and arm64; macOS signed ad-hoc, not notarized; run by hand = build only, bundles as workflow artifacts)
- `scripts/third-party.mjs` — third-party components and licenses for the About dialog (`src/generated/third-party.json`)
- `scripts/test-sshd.sh` — local unprivileged sshd fixture (hosts `gf-local`, `gf-sftponly`, `gf-rc`, `gf-slow`); `scripts/delay-relay.py` adds latency for `gf-slow`
- `docs/architecture.md` — design decisions and their reasons; `docs/roadmap.md` — future work; `docs/linux-test.md` — checklist for testing the Linux packages; `docs/connections.md` — design of connection management and multiple remote panes (all four phases implemented)

## Commands

```sh
pnpm install
pnpm tauri dev                     # run the app
pnpm build                         # tsc + vite build (frontend type-check)
pnpm test                          # vitest (pure TS modules)
cd src-tauri && cargo test && cargo clippy --all-targets
scripts/test-sshd.sh start         # test servers on 127.0.0.1:2222/2223
cd src-tauri && cargo run --release --example spike -- "$PWD/../.dev-sshd/ssh" gf-local segreta [remote-file]
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

`docs/roadmap.md` (Italian) tracks future work, grouped in development waves (Wave 1 Affidabilità → Wave 4 Apertura al pubblico), each in priority order. When a new feature or improvement is proposed (by the user or as a suggestion at the end of a phase), add it there with the date and a short motivation, in the wave where it belongs or under "Da collocare" until the user decides; when one is done, move it to "Fatto" with the commit. Out-of-scope items for the MVP are listed there too.

When a design decision is taken or changed, update `docs/architecture.md`.
