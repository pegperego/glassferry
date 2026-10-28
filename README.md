# Glassferry

A free, open source SFTP client for macOS and Linux that uses your system OpenSSH. It honours your `~/.ssh/config` exactly as the terminal does: `Include`, `Match`, `ProxyJump`, `IdentityAgent` (1Password and other agents), `RemoteCommand`. Nothing is installed on the server: only the standard SFTP subsystem is used, plus ordinary shell commands when the account allows them.

Status: MVP complete, not yet released. The interface is in Italian for now.

## Features

- **Connection manager.** Import aliases from your ssh config (settings stay there) or configure connections in the app (written to a generated ssh_config that includes yours, so ssh still does the work). Each connection has a name and a colored category, e.g. a red "Produzione". Quick connect with ⌘K.
- **Passwords in the system keychain**, per connection and only if you choose so; never in files.
- **Prompts inside the app.** Passwords, passphrases, OTP codes, FIDO confirmations and unknown host keys are asked in the app window through `SSH_ASKPASS`.
- **Dual pane, local | remote.**
  - Virtualized lists, sorting, hidden files, Finder-style selection, full keyboard navigation.
- **Transfers.**
  - Recursive upload and download, with a queue, progress, speed and cancel.
  - Conflict prompts (overwrite / skip, apply to all).
  - Drag & drop between panes and from the Finder.
  - Up to two remote panes, side by side or stacked; copies between them run on the server when both show the same connection, and stream through the app between different servers.
  - Files are written to a temporary name and renamed at the end, so a destination file is never left truncated.
- **File operations.**
  - New folder, rename, move and delete. Never overwrites.
  - Local deletes go to the Trash; remote deletes are permanent and confirmed first.
- **Edit remote files with your default app.** Every save is uploaded. If someone else changed the file on the server meanwhile, the app asks before overwriting.
- **Activity log.**
  - Every ssh command, SFTP request and remote command, with its exact command line and a terminal equivalent.
  - Also kept in a rotating log file. A diagnostic mode per host adds `ssh -v` output.
- **Works with SFTP-only accounts** (`ForceCommand internal-sftp`, chroot). When a shell is available it speeds up recursive delete, cross-filesystem moves and conflict detection.

## Keyboard

| Key | Action |
|---|---|
| ↑ ↓, Page Up/Down, Home/End (⇧ extends) | Move the cursor |
| Enter, ⌘↓ | Open folder / edit remote file / open local file |
| ⌫, ⌘↑ | Parent folder |
| ⌘A, Esc | Select all, clear selection |
| ⇧⌘. | Show hidden files |
| F5 | Copy the selection to the other pane |
| F2 | Rename |
| F7, ⇧⌘N | New folder |
| ⌘⌫, Delete | Delete |
| ⌘K | New connection (search) |
| ⌘J | Show / hide the bottom panel |

## Privacy and local files

| What | Where |
|---|---|
| Passwords, passphrases and askpass answers | Never logged; passwords saved only in the system keychain when enabled for a connection |
| Connections | `~/Library/Application Support/glassferry/connections.json` and the generated `ssh_config` (mode 0600), no secrets |
| Activity log | `~/Library/Logs/Glassferry/` (Linux: `$XDG_STATE_HOME/glassferry/logs`) |
| Local copies of files being edited | `~/Library/Caches/glassferry/edit/`. Removed when closed, and on exit once synced |
| Control sockets | `/tmp/gf-<uid>` (mode 0700) |

## Requirements

- macOS or Linux with OpenSSH 8.4 or newer (`SSH_ASKPASS_REQUIRE`).
- To build: Rust (rustup), Node 22+, pnpm.

## Development

```sh
pnpm install
pnpm tauri dev
```

Further reading:
- [`CLAUDE.md`](CLAUDE.md): architecture, modules and test commands, including a local test sshd.
- [`docs/architecture.md`](docs/architecture.md): design decisions and why they were taken.
- [`docs/roadmap.md`](docs/roadmap.md): planned and proposed work.

## License

[MIT](LICENSE)
