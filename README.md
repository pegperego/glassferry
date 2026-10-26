# SSH File Manager

A free, open source SFTP client for macOS and Linux that uses your system OpenSSH, so it honours your `~/.ssh/config` exactly as the terminal does (Include, Match, ProxyJump, IdentityAgent such as 1Password, ...). Nothing is installed on the server: only the standard SFTP subsystem is used.

Status: early development (connection spike).

## Development

Requirements: Rust (rustup), Node 22+, pnpm, OpenSSH 8.4+.

```sh
pnpm install
pnpm tauri dev
```

See `CLAUDE.md` for architecture notes and test commands.

## License

MIT
