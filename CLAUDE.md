# SSH File Manager

Open source (MIT) SFTP client with a GUI, macOS first, Linux supported. Tauri 2 + Rust backend, React + TypeScript frontend.

## Core principle

Never parse or reinterpret the user's `~/.ssh/config`. All connections go through the system `ssh` binary (default `/usr/bin/ssh`), so Include/Match/ProxyJump/IdentityAgent work exactly as in the terminal. Nothing is ever installed on servers.

- Host list: `Host` aliases collected by `ssh_config.rs` (follows `Include`, skips wildcard/negated patterns). Effective settings only via `ssh -G`.
- Connection (`session.rs`): the app owns a ControlMaster (`ssh -M -N -f -o ControlPersist=15m -S <sock> -E <log> -- alias`); channels reuse it with `-S <sock> -o ControlMaster=no -o RemoteCommand=none -T`.
- SFTP: `ssh -s -- alias sftp`, stdin/stdout handed to `openssh_sftp_client::Sftp::new`. We do NOT use the `openssh` crate (it injects its own options).
- Shell commands are an optional accelerator: `probe()` detects shell access and available commands; SFTP must always work alone (SFTP-only accounts). Remote commands are built only with `sh_command(script, args)` (paths as positional params, `env LC_ALL=C sh -c`), never by string concatenation.
- Only add ssh options that are strictly required (listed above). Any new override needs a justification in a comment.

## Askpass

The app binary is also the `SSH_ASKPASS` helper (`askpass.rs`): when started with `SFM_ASKPASS_SOCKET` it forwards the prompt to the running app over a Unix socket (token-authenticated) and prints the answer. The app emits `askpass-request` to the UI; the UI answers with the `askpass_respond` command. Prompt kinds: `secret`, `confirm`, `notice`, `hostKey`.

Runtime files (sockets, ssh logs) live in a short private dir (`runtime_dir.rs`): `/tmp/sfm-<uid>` or `$XDG_RUNTIME_DIR/sfm`, mode 0700. Socket paths must stay short (~104 bytes on macOS).

## Activity log

Every ssh process, SFTP request, exec command, askpass prompt and local operation goes through `log.rs` (`Logger::start(...)` returns a `LogOp`; finish it with `ok`/`fail`/`result`, dropping it unfinished records an error). Entries carry the exact command line (`shell_line`) and, when useful, a terminal equivalent (`sftp_equivalent`). Ring buffer (default 1000, set from the UI), batched `log-batch` events every 100 ms, rotating text file in `~/Library/Logs/SSH File Manager/` (Linux: `$XDG_STATE_HOME/sshfilemanager/logs`). Never log askpass answers, tokens or the ssh environment. New backend operations must be logged.

Diagnostic mode per host (UI toggle, stored in localStorage) passes `verbose` to `connect`: the master runs with `-v` and its debug output is attached to the connect entry.

## Layout

- `src-tauri/src/lib.rs` — Tauri state, commands, exit cleanup
- `src-tauri/src/{ssh_config,session,askpass,runtime_dir,error}.rs`
- `src-tauri/src/entry.rs` (`FileEntry`, shared by both panes), `local.rs` (local listing)
- `src-tauri/examples/spike.rs` — headless connect/probe/list/throughput check
- `src/api.ts` — typed wrappers for every Tauri command/event (keep in sync with `lib.rs`)
- `src/components/` — React function components only: `FilePane` (generic virtualized pane), `RemotePane`, `HostList`, `AskpassDialog`
- `src/usePane.ts` — pane state hook (path, entries, sort, hidden files, selection); `src/fileList.ts` — pure sorting/selection logic (tested in `fileList.test.ts`)
- `src/paths.ts` (remote POSIX paths), `src/format.ts`
- `src/components/LogPanel.tsx`, `src/useActivityLog.ts`, `src/logStore.ts` (pure merge/filter, tested) — bottom log panel (⌘J)
- `src/settings.ts` — `usePersistentState` (localStorage with safe fallbacks)
- `scripts/test-sshd.sh` — local unprivileged sshd fixture (hosts `sfm-local`, `sfm-sftponly`, `sfm-rc`)

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
- UI checks without the native window: run `pnpm dev` and open http://localhost:1420 with Playwright, mocking `window.__TAURI_INTERNALS__.invoke` via `page.addInitScript`.

## Out of scope (MVP)

Windows, drag out to Finder, File Provider integration, chmod/chown UI, resume of interrupted transfers.
