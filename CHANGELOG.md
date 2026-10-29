# Changelog

Notable changes of each release. The section of a version is also the text of its GitHub release (`.github/workflows/release.yml`).

## [0.3.0] - 2026-10-29

First public release. The interface is in Italian for now.

### Highlights

- SFTP client that runs your system OpenSSH: `~/.ssh/config` works exactly as in the terminal (`Include`, `Match`, `ProxyJump`, agents such as 1Password). Nothing is installed on the server; SFTP-only accounts work too.
- Local pane and up to two remote panes, side by side or stacked; transfers, drag & drop, copies between servers.
- Connection manager with colored categories, protected connections that ask for the connection name before destructive operations, passwords in the system keychain.
- Edit remote files with your default app: every save is uploaded, conflicts are detected.
- Activity log with the exact command behind every operation.

### Reliability

- Edited files with changes not uploaded yet are never lost: on disconnect or exit they stay as "Non connesso" and are uploaded when you reconnect, also after a restart or a crash.
- Saves of edited files replace the remote file in one step (temporary file + posix-rename), so a dropped connection never leaves it truncated; files where that would change the owner, group or a symlink are written in place.
- Edit conflicts made by someone else within the same second are detected also on accounts without a shell.
- Interrupted transfers (cancelled, connection dropped, app closed) resume where they stopped.

### Everyday use

- Impostazioni (⌘,): the ssh binary to use, and the terminal (installed ones are detected).
- Open the system terminal in the folder of a pane (⌥⌘T or `>_`): locally, or with a new ssh connection to the pane's server.
- Bookmarks of folders, per connection and local (★, ⌘D), also in the ⌘K palette.
- Permissions, owner and group (chmod / chown / chgrp, also recursive), with the commands previewed; owner and group shown in the panes.
- Folder sizes on request (du on the server, or a walk).
- "Informazioni su Glassferry": version, license and the third-party components with their licenses.

### Install

macOS builds are signed ad-hoc but not notarized yet. On first launch macOS refuses to open the app: go to System Settings → Privacy & Security and click "Open Anyway", or run `xattr -dr com.apple.quarantine /Applications/Glassferry.app`.

Linux packages (AppImage, deb, rpm, for x64 and arm64) are built by CI but have not been tested on a real Linux desktop yet: reports are welcome.
