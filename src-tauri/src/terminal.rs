//! Opening the system terminal at a pane's folder.
//!
//! Locally the login shell starts in that folder. For a remote pane a new,
//! independent ssh connection (same binary, `-F` and alias, authenticated in
//! the terminal itself) changes to the folder and starts a login shell. Not
//! through the app's ControlMaster: closing the pane would close the
//! terminal too.
//!
//! The terminal runs a one-shot script written to the private runtime dir
//! (it removes itself): `open -a <app> <script>.command` on macOS (Terminal
//! and iTerm run it, no automation permission needed), `$TERMINAL -e` or
//! `x-terminal-emulator -e` on Linux, or a custom command with `{script}`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{AppError, Result};
use crate::log::shell_line;
use crate::session::sh_quote;

pub const DEFAULT_MAC_APP: &str = "Terminal";
pub const PLACEHOLDER: &str = "{script}";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct TerminalSettings {
    /// macOS application that runs the script (`open -a`); `None`: Terminal.
    pub app: Option<String>,
    /// Command line run with `sh -c`, `{script}` replaced by the script's
    /// path; overrides `app` and the Linux default.
    pub command: Option<String>,
}

impl TerminalSettings {
    pub fn normalized(self) -> Self {
        let clean = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Self {
            app: clean(self.app),
            command: clean(self.command),
        }
    }
}

/// Script for a local folder.
pub fn local_script(dir: &str) -> String {
    format!(
        "#!/bin/sh\nrm -f -- \"$0\"\ncd -- {} || exit 1\nexec \"${{SHELL:-/bin/sh}}\" -l\n",
        sh_quote(dir)
    )
}

/// The ssh command line of a remote terminal: `-t` because a command is
/// given (ssh would not allocate a tty), `-o RemoteCommand=none` because a
/// `RemoteCommand` in the config refuses a command on the command line (as
/// for the app's channels).
pub fn ssh_line(ssh_bin: &Path, config_file: Option<&Path>, alias: &str, path: &str) -> String {
    let mut args: Vec<String> = vec![ssh_bin.to_string_lossy().into_owned()];
    if let Some(file) = config_file {
        args.push("-F".into());
        args.push(file.to_string_lossy().into_owned());
    }
    args.extend(["-t", "-o", "RemoteCommand=none", "--"].map(String::from));
    args.push(alias.into());
    args.push(remote_command(path));
    shell_line(args)
}

/// Run by the remote login shell, whatever it is: a POSIX `sh` changes to
/// the folder (passed as a positional parameter, never as code) and replaces
/// itself with the user's login shell; a missing folder leaves the home.
fn remote_command(path: &str) -> String {
    let inner = r#"cd -- "$1" || echo "glassferry: cartella non trovata: $1"; exec "${SHELL:-/bin/sh}" -l"#;
    format!("exec sh -c {} sh {}", sh_quote(inner), sh_quote(path))
}

/// Script for a remote folder.
pub fn remote_script(ssh_line: &str) -> String {
    format!("#!/bin/sh\nrm -f -- \"$0\"\nexec {ssh_line}\n")
}

/// Writes the script and hands it to the terminal; returns the command used.
pub async fn open(settings: &TerminalSettings, runtime_dir: &Path, script: &str) -> Result<String> {
    let path = write_script(runtime_dir, script)?;
    let result = launch(settings, &path).await;
    if result.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    result
}

fn write_script(runtime_dir: &Path, script: &str) -> Result<PathBuf> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let name = format!("term-{}-{}.command", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let path = runtime_dir.join(name);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, script.as_bytes()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

async fn launch(settings: &TerminalSettings, script: &Path) -> Result<String> {
    let script_arg = script.to_string_lossy().into_owned();
    if let Some(command) = &settings.command {
        let line = if command.contains(PLACEHOLDER) {
            command.replace(PLACEHOLDER, &sh_quote(&script_arg))
        } else {
            format!("{command} {}", sh_quote(&script_arg))
        };
        spawn(Command::new("/bin/sh").arg("-c").arg(&line))?;
        return Ok(line);
    }
    if cfg!(target_os = "macos") {
        let app = settings.app.as_deref().unwrap_or(DEFAULT_MAC_APP);
        let output = Command::new("open").args(["-a", app]).arg(script).output().await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::Other(format!("impossibile aprire {app}: {}", stderr.trim())));
        }
        return Ok(shell_line(["open", "-a", app, &script_arg]));
    }
    let terminal = std::env::var("TERMINAL").ok().filter(|t| !t.is_empty()).unwrap_or("x-terminal-emulator".into());
    spawn(Command::new(&terminal).arg("-e").arg(script))
        .map_err(|e| AppError::Other(format!("{terminal}: {e} (imposta un comando nelle Impostazioni)")))?;
    Ok(shell_line([terminal.as_str(), "-e", &script_arg]))
}

/// Starts the terminal without waiting for it (it runs until closed).
fn spawn(command: &mut Command) -> Result<()> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake login shell that prints where it starts.
    fn fake_shell(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let shell = dir.join("fake-shell");
        std::fs::write(&shell, "#!/bin/sh\npwd\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        shell
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gf-terminal-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn local_script_starts_the_shell_in_the_folder_and_removes_itself() {
        let dir = scratch("local");
        let target = dir.join("it's a \"dir\" $HOME");
        std::fs::create_dir_all(&target).unwrap();
        let script = write_script(&dir, &local_script(&target.to_string_lossy())).unwrap();
        let output = std::process::Command::new(&script).env("SHELL", fake_shell(&dir)).output().unwrap();
        let printed = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(std::fs::canonicalize(printed).unwrap(), std::fs::canonicalize(&target).unwrap());
        assert!(!script.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_command_survives_the_login_shell() {
        // What the remote login shell runs, here with a local sh.
        let dir = scratch("remote");
        let target = dir.join("spazi e 'apici'; $(rm -rf x)");
        std::fs::create_dir_all(&target).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(remote_command(&target.to_string_lossy()))
            .env("SHELL", fake_shell(&dir))
            .output()
            .unwrap();
        let printed = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(std::fs::canonicalize(printed).unwrap(), std::fs::canonicalize(&target).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ssh_line_keeps_config_and_alias() {
        let line = ssh_line(Path::new("/usr/bin/ssh"), Some(Path::new("/c/ssh_config")), "gf-1", "/srv/www");
        assert!(line.starts_with("/usr/bin/ssh -F /c/ssh_config -t -o RemoteCommand=none -- gf-1 "), "{line}");
    }
}
