//! One SSH connection to a host: a ControlMaster process owned by the app,
//! an SFTP channel multiplexed over it, and optional exec channels.
//!
//! The only options we add to the user's config are the ones needed to own
//! the master (`-M`, `-S`, `ControlPersist`) and, for channels, the ones that
//! would otherwise break a binary protocol (`-T`, `RemoteCommand=none`).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use futures_util::TryStreamExt;
use openssh_sftp_client::{Sftp, SftpOptions};
use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

use crate::error::{AppError, Result};

const CONTROL_PERSIST: &str = "ControlPersist=15m";
const PROBE_MARKER: &str = "__SFM__";
const PROBED_COMMANDS: &[&str] = &["rm", "cp", "mv", "du", "find", "sha256sum", "tar"];

/// How ssh processes for one host are spawned.
#[derive(Clone)]
pub struct SshLauncher {
    pub ssh_bin: PathBuf,
    pub alias: String,
    pub control_path: PathBuf,
    pub log_path: PathBuf,
    /// Extra environment, e.g. askpass variables. Empty means ssh uses the tty.
    pub env: Vec<(String, String)>,
}

impl SshLauncher {
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.ssh_bin);
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        cmd
    }

    /// Command for a channel over the master. Falls back to a direct
    /// connection if the master is gone (ssh's default behaviour).
    fn channel_command(&self) -> Command {
        let mut cmd = self.command();
        cmd.arg("-S")
            .arg(&self.control_path)
            .args(["-o", "ControlMaster=no", "-o", "RemoteCommand=none", "-T"]);
        cmd
    }

    /// Starts the master and waits until authentication has completed.
    pub async fn start_master(&self) -> Result<()> {
        let _ = std::fs::remove_file(&self.log_path);
        let status = self
            .command()
            .args(["-M", "-N", "-f", "-o", CONTROL_PERSIST])
            .arg("-S")
            .arg(&self.control_path)
            .arg("-E")
            .arg(&self.log_path)
            .arg("--")
            .arg(&self.alias)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
        if !status.success() {
            let message = self.read_log();
            let _ = std::fs::remove_file(&self.log_path);
            return Err(AppError::Ssh(message));
        }
        Ok(())
    }

    pub async fn stop_master(&self) {
        let _ = self
            .command()
            .arg("-S")
            .arg(&self.control_path)
            .args(["-O", "exit", "--"])
            .arg(&self.alias)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        let _ = std::fs::remove_file(&self.log_path);
    }

    /// Blocking variant used while the app is shutting down.
    pub fn stop_master_blocking(&self) {
        let _ = std::process::Command::new(&self.ssh_bin)
            .arg("-S")
            .arg(&self.control_path)
            .args(["-O", "exit", "--"])
            .arg(&self.alias)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_file(&self.log_path);
    }

    fn read_log(&self) -> String {
        let log = std::fs::read_to_string(&self.log_path).unwrap_or_default();
        let log = log.trim();
        if log.is_empty() {
            "ssh exited with an error".into()
        } else {
            log.to_string()
        }
    }

    /// Spawns `ssh -s <alias> sftp` over the master.
    pub fn spawn_sftp(&self) -> Result<Child> {
        let child = self
            .channel_command()
            .args(["-s", "--"])
            .arg(&self.alias)
            .arg("sftp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        Ok(child)
    }

    /// Runs a remote command line (already quoted for a POSIX shell).
    pub async fn exec(&self, command_line: &str) -> Result<std::process::Output> {
        let output = self
            .channel_command()
            .arg("--")
            .arg(&self.alias)
            .arg(command_line)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await?;
        Ok(output)
    }
}

/// Quotes a string for a POSIX shell: `'` becomes `'\''`.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Builds `sh -c '<script>' sh arg1 arg2...` so that the script always runs
/// under a POSIX shell (whatever the login shell is) and paths travel as
/// positional parameters, never as code.
pub fn sh_command(script: &str, args: &[&str]) -> String {
    let mut line = format!("env LC_ALL=C sh -c {} sh", sh_quote(script));
    for arg in args {
        line.push(' ');
        line.push_str(&sh_quote(arg));
    }
    line
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// The account can run shell commands (not an SFTP-only account).
    pub shell: bool,
    pub os: Option<String>,
    pub commands: Vec<String>,
}

pub async fn probe(launcher: &SshLauncher) -> Capabilities {
    let script = format!(
        "echo {PROBE_MARKER}; uname -s; for c in {}; do command -v \"$c\" >/dev/null 2>&1 && echo \"$c\"; done",
        PROBED_COMMANDS.join(" ")
    );
    let Ok(output) = launcher.exec(&sh_command(&script, &[])).await else {
        return Capabilities::default();
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines().map(str::trim);
    // Banners or MOTD-like noise may precede the marker.
    if !lines.by_ref().any(|l| l == PROBE_MARKER) {
        return Capabilities::default();
    }
    Capabilities {
        shell: true,
        os: lines.next().map(str::to_string),
        commands: lines
            .filter(|l| PROBED_COMMANDS.contains(l))
            .map(str::to_string)
            .collect(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEntry {
    pub name: String,
    pub kind: EntryKind,
    /// For symlinks: whether the target is a directory.
    pub target_is_dir: bool,
    pub size: Option<u64>,
    /// Seconds since the Unix epoch.
    pub modified: Option<u64>,
    pub permissions: Option<u32>,
}

pub struct Session {
    pub launcher: SshLauncher,
    pub sftp: Sftp,
    pub home: PathBuf,
    pub capabilities: Capabilities,
    _sftp_process: Child,
}

impl Session {
    pub async fn connect(launcher: SshLauncher) -> Result<Self> {
        launcher.start_master().await?;
        match Self::open_channels(&launcher).await {
            Ok(session) => Ok(session),
            Err(e) => {
                launcher.stop_master().await;
                Err(e)
            }
        }
    }

    async fn open_channels(launcher: &SshLauncher) -> Result<Self> {
        let mut child = launcher.spawn_sftp()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");

        let sftp = match Sftp::new(stdin, stdout, SftpOptions::default()).await {
            Ok(sftp) => sftp,
            Err(e) => {
                let mut message = String::new();
                let _ = stderr.read_to_string(&mut message).await;
                let message = message.trim();
                return Err(if message.is_empty() {
                    e.into()
                } else {
                    AppError::Ssh(message.to_string())
                });
            }
        };
        // Drain stderr so the channel never blocks on a full pipe.
        tauri::async_runtime::spawn(async move {
            let mut sink = Vec::new();
            let _ = stderr.read_to_end(&mut sink).await;
        });

        let home = sftp.fs().canonicalize(".").await?;
        let capabilities = probe(launcher).await;
        Ok(Self {
            launcher: launcher.clone(),
            sftp,
            home,
            capabilities,
            _sftp_process: child,
        })
    }

    pub async fn list_dir(&self, path: &Path) -> Result<Vec<RemoteEntry>> {
        let mut fs = self.sftp.fs();
        let dir = fs.open_dir(path).await?;
        let raw: Vec<_> = dir.read_dir().try_collect().await?;

        let mut entries = Vec::with_capacity(raw.len());
        for entry in raw {
            let name = entry.filename().to_string_lossy().into_owned();
            if name == "." || name == ".." {
                continue;
            }
            let meta = entry.metadata();
            let file_type = entry.file_type();
            let kind = match file_type {
                Some(t) if t.is_dir() => EntryKind::Dir,
                Some(t) if t.is_file() => EntryKind::File,
                Some(t) if t.is_symlink() => EntryKind::Symlink,
                _ => EntryKind::Other,
            };
            let target_is_dir = if matches!(kind, EntryKind::Symlink) {
                fs.metadata(path.join(&name))
                    .await
                    .ok()
                    .and_then(|m| m.file_type())
                    .is_some_and(|t| t.is_dir())
            } else {
                false
            };
            entries.push(RemoteEntry {
                name,
                kind,
                target_is_dir,
                size: meta.len(),
                modified: meta.modified().map(|t| t.as_duration().as_secs()),
                permissions: meta.permissions().map(|p| p.as_raw().bits()),
            });
        }
        Ok(entries)
    }

    pub async fn close(self) {
        let _ = self.sftp.close().await;
        self.launcher.stop_master().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_single_quotes() {
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn builds_sh_command_with_positional_args() {
        assert_eq!(
            sh_command(r#"rm -rf -- "$1""#, &["/tmp/x'y"]),
            r#"env LC_ALL=C sh -c 'rm -rf -- "$1"' sh '/tmp/x'\''y'"#
        );
    }
}
