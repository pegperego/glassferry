//! One SSH connection to a host: a ControlMaster process owned by the app,
//! an SFTP channel multiplexed over it, and optional exec channels.
//!
//! The only options we add to the user's config are the ones needed to own
//! the master (`-M`, `-S`, `ControlPersist`) and, for channels, the ones that
//! would otherwise break a binary protocol (`-T`, `RemoteCommand=none`).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_util::TryStreamExt;
use openssh_sftp_client::{Sftp, SftpOptions};
use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

use crate::entry::{EntryKind, FileEntry};
use crate::error::{AppError, Result};
use crate::log::{sftp_equivalent, shell_line, Channel, LogOp, Logger};

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
    /// Never logged: it contains the askpass token.
    pub env: Vec<(String, String)>,
    /// Diagnostic mode: `-v` on the master, debug output attached to the log.
    pub verbose: bool,
    pub logger: Arc<Logger>,
}

/// Command line of a prepared command, for the activity log.
fn command_line(cmd: &Command) -> String {
    let std = cmd.as_std();
    shell_line(std::iter::once(std.get_program()).chain(std.get_args()))
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

    fn log(&self, channel: Channel, op: &str, detail: impl Into<String>) -> LogOp {
        self.logger.start(Some(&self.alias), channel, op, detail)
    }

    /// Starts the master and waits until authentication has completed.
    pub async fn start_master(&self) -> Result<()> {
        let _ = std::fs::remove_file(&self.log_path);
        if self.control_path.exists() {
            // A leftover master on our socket would capture every channel
            // (and ssh would silently disable multiplexing for the new one).
            let _ = Command::from(self.stop_command()).status().await;
            let _ = std::fs::remove_file(&self.control_path);
        }
        let mut cmd = self.command();
        cmd.args(["-M", "-N", "-f", "-o", CONTROL_PERSIST]);
        if self.verbose {
            cmd.arg("-v");
        }
        cmd.arg("-S")
            .arg(&self.control_path)
            .arg("-E")
            .arg(&self.log_path)
            .arg("--")
            .arg(&self.alias)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut op = self.log(Channel::Ssh, "connect", command_line(&cmd));
        op.equivalent(shell_line(["ssh", "-v", "-N", "--", &self.alias]));
        let status = match cmd.status().await {
            Ok(status) => status,
            Err(e) => {
                op.fail(&e);
                return Err(e.into());
            }
        };
        let log = self.read_log();
        if !status.success() {
            let _ = std::fs::remove_file(&self.log_path);
            let message = last_meaningful_line(&log).unwrap_or("ssh exited with an error").to_string();
            op.output(log);
            op.fail(&message);
            return Err(AppError::Ssh(message));
        }
        if self.verbose {
            op.output(log);
        }
        op.ok(Some(if self.verbose { "diagnostica (-v)".into() } else { "master attivo".into() }));
        Ok(())
    }

    fn stop_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.ssh_bin);
        cmd.arg("-S")
            .arg(&self.control_path)
            .args(["-O", "exit", "--"])
            .arg(&self.alias)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    pub async fn stop_master(&self) {
        let cmd = Command::from(self.stop_command());
        let op = self.log(Channel::Ssh, "disconnect", command_line(&cmd));
        self.finish_stop(op, Command::from(self.stop_command()).status().await);
    }

    /// Blocking variant used while the app is shutting down.
    pub fn stop_master_blocking(&self) {
        let cmd = Command::from(self.stop_command());
        let op = self.log(Channel::Ssh, "disconnect", command_line(&cmd));
        self.finish_stop(op, self.stop_command().status());
    }

    fn finish_stop(&self, op: LogOp, status: std::io::Result<std::process::ExitStatus>) {
        let _ = std::fs::remove_file(&self.log_path);
        match status {
            Ok(s) if s.success() => op.ok(None),
            // The master may already be gone (ControlPersist expired).
            Ok(s) => op.ok(Some(format!("exit {}", s.code().unwrap_or(-1)))),
            Err(e) => op.fail(e),
        }
    }

    fn read_log(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    /// Spawns `ssh -s <alias> sftp` over the master.
    pub fn spawn_sftp(&self) -> Result<(Child, LogOp)> {
        let mut cmd = self.channel_command();
        cmd.args(["-s", "--"])
            .arg(&self.alias)
            .arg("sftp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut op = self.log(Channel::Ssh, "sftp-channel", command_line(&cmd));
        op.equivalent(shell_line(["sftp", &self.alias]));
        match cmd.spawn() {
            Ok(child) => Ok((child, op)),
            Err(e) => {
                op.fail(&e);
                Err(e.into())
            }
        }
    }

    /// Runs a remote command line (already quoted for a POSIX shell).
    pub async fn exec(&self, op_name: &str, command_line_remote: &str) -> Result<std::process::Output> {
        let mut cmd = self.channel_command();
        cmd.arg("--")
            .arg(&self.alias)
            .arg(command_line_remote)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let mut op = self.log(Channel::Exec, op_name, command_line_remote);
        op.equivalent(command_line(&cmd).replacen(&*self.ssh_bin.to_string_lossy(), "ssh", 1));
        match cmd.output().await {
            Ok(output) => {
                op.output(String::from_utf8_lossy(&output.stderr));
                let code = output.status.code().unwrap_or(-1);
                if output.status.success() {
                    op.ok(Some(format!("exit {code}")));
                } else {
                    op.fail(format!("exit {code}"));
                }
                Ok(output)
            }
            Err(e) => {
                op.fail(&e);
                Err(e.into())
            }
        }
    }
}

/// Last non-debug line of an ssh log: usually the actual error.
fn last_meaningful_line(log: &str) -> Option<&str> {
    log.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty() && !l.starts_with("debug"))
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
    let Ok(output) = launcher.exec("probe", &sh_command(&script, &[])).await else {
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
        let (mut child, op) = launcher.spawn_sftp()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");

        let sftp = match Sftp::new(stdin, stdout, SftpOptions::default()).await {
            Ok(sftp) => sftp,
            Err(e) => {
                let mut message = String::new();
                let read = stderr.read_to_string(&mut message);
                let _ = tokio::time::timeout(Duration::from_secs(2), read).await;
                let message = message.trim();
                let error = if message.is_empty() {
                    e.into()
                } else {
                    AppError::Ssh(message.to_string())
                };
                op.fail(&error);
                return Err(error);
            }
        };
        op.ok(None);
        // Drain stderr so the channel never blocks on a full pipe.
        tauri::async_runtime::spawn(async move {
            let mut sink = Vec::new();
            let _ = stderr.read_to_end(&mut sink).await;
        });

        let mut op = launcher.log(Channel::Sftp, "realpath", ".");
        op.equivalent(sftp_equivalent(&launcher.alias, "pwd", &[]));
        let home = op.result(sftp.fs().canonicalize(".").await, |home| {
            Some(home.to_string_lossy().into_owned())
        })?;
        let capabilities = probe(launcher).await;
        Ok(Self {
            launcher: launcher.clone(),
            sftp,
            home,
            capabilities,
            _sftp_process: child,
        })
    }

    pub async fn list_dir(&self, path: &Path) -> Result<Vec<FileEntry>> {
        let path_str = path.to_string_lossy();
        let mut op = self.launcher.log(Channel::Sftp, "readdir", path_str.clone());
        op.equivalent(sftp_equivalent(&self.launcher.alias, "ls -la", &[&path_str]));
        op.result(self.read_dir(path).await, |entries| Some(format!("{} voci", entries.len())))
    }

    pub(crate) async fn read_dir(&self, path: &Path) -> Result<Vec<FileEntry>> {
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
            entries.push(FileEntry {
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
