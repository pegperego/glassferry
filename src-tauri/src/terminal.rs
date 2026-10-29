//! Opening the system terminal at a pane's folder.
//!
//! Locally the login shell starts in that folder. For a remote pane a new,
//! independent ssh connection (same binary, `-F` and alias, authenticated in
//! the terminal itself) changes to the folder and starts a login shell. Not
//! through the app's ControlMaster: closing the pane would close the
//! terminal too.
//!
//! The terminal runs a one-shot script written to the private runtime dir
//! (it removes itself). The terminals the app knows (`KNOWN`) are detected
//! where they are installed and started the way each one runs a program:
//! `open -a` for Terminal and iTerm2 (they run a `.command` file, no
//! automation permission needed), `open -na ... --args -e` for Ghostty,
//! the bundled binary for WezTerm, kitty, Alacritty, the command in the PATH
//! on Linux. Any other terminal: a custom command with `{script}`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{AppError, Result};
use crate::log::shell_line;
use crate::session::sh_quote;

pub const PLACEHOLDER: &str = "{script}";

/// How a terminal runs the script.
#[derive(Clone, Copy)]
enum Start {
    /// `open -a <app> <script>`: the app opens the `.command` file.
    OpenFile,
    /// `open -na <app> --args <args> <script>`.
    OpenArgs(&'static [&'static str]),
    /// `<program> <args> <script>`: the binary inside the bundle (macOS)
    /// or the command in the PATH (Linux).
    Exec(&'static [&'static str]),
}

struct Known {
    id: &'static str,
    name: &'static str,
    /// macOS: app bundle name; Linux: command.
    install: &'static str,
    /// macOS, `Start::Exec`: the binary inside the bundle.
    binary: &'static str,
    start: Start,
}

#[cfg(target_os = "macos")]
const KNOWN: &[Known] = &[
    Known { id: "terminal", name: "Terminal", install: "Terminal.app", binary: "", start: Start::OpenFile },
    Known { id: "iterm", name: "iTerm2", install: "iTerm.app", binary: "", start: Start::OpenFile },
    Known { id: "ghostty", name: "Ghostty", install: "Ghostty.app", binary: "", start: Start::OpenArgs(&["-e"]) },
    Known { id: "wezterm", name: "WezTerm", install: "WezTerm.app", binary: "Contents/MacOS/wezterm", start: Start::Exec(&["start", "--"]) },
    Known { id: "kitty", name: "kitty", install: "kitty.app", binary: "Contents/MacOS/kitty", start: Start::Exec(&[]) },
    Known { id: "alacritty", name: "Alacritty", install: "Alacritty.app", binary: "Contents/MacOS/alacritty", start: Start::Exec(&["-e"]) },
];

#[cfg(not(target_os = "macos"))]
const KNOWN: &[Known] = &[
    Known { id: "x-terminal-emulator", name: "Predefinito di sistema (x-terminal-emulator)", install: "x-terminal-emulator", binary: "", start: Start::Exec(&["-e"]) },
    Known { id: "gnome-terminal", name: "GNOME Terminal", install: "gnome-terminal", binary: "", start: Start::Exec(&["--"]) },
    Known { id: "konsole", name: "Konsole", install: "konsole", binary: "", start: Start::Exec(&["-e"]) },
    Known { id: "xfce4-terminal", name: "Xfce Terminal", install: "xfce4-terminal", binary: "", start: Start::Exec(&["-x"]) },
    Known { id: "kitty", name: "kitty", install: "kitty", binary: "", start: Start::Exec(&[]) },
    Known { id: "alacritty", name: "Alacritty", install: "alacritty", binary: "", start: Start::Exec(&["-e"]) },
    Known { id: "wezterm", name: "WezTerm", install: "wezterm", binary: "", start: Start::Exec(&["start", "--"]) },
    Known { id: "xterm", name: "xterm", install: "xterm", binary: "", start: Start::Exec(&["-e"]) },
];

/// A terminal found on this machine.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalApp {
    pub id: String,
    pub name: String,
}

/// Where a known terminal is installed: the app bundle, or the command.
fn locate(known: &Known) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = dirs::home_dir().unwrap_or_default();
        [
            PathBuf::from("/Applications"),
            home.join("Applications"),
            PathBuf::from("/System/Applications/Utilities"),
            PathBuf::from("/Applications/Utilities"),
        ]
        .into_iter()
        .map(|dir| dir.join(known.install))
        .find(|path| path.is_dir())
    } else {
        std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .map(|dir| dir.join(known.install))
            .find(|path| path.is_file())
    }
}

/// The known terminals installed here, in the order of `KNOWN`.
pub fn installed() -> Vec<TerminalApp> {
    KNOWN
        .iter()
        .filter(|k| locate(k).is_some())
        .map(|k| TerminalApp { id: k.id.into(), name: k.name.into() })
        .collect()
}

/// `app` as stored: an id, or a name from older settings ("Terminal", "iTerm").
fn known(app: &str) -> Option<&'static Known> {
    KNOWN
        .iter()
        .find(|k| k.id.eq_ignore_ascii_case(app) || k.name.eq_ignore_ascii_case(app) || k.install.trim_end_matches(".app").eq_ignore_ascii_case(app))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct TerminalSettings {
    /// Id of a known terminal (`installed`); `None`: Terminal on macOS,
    /// `$TERMINAL` or the first one installed on Linux.
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
    let chosen = match settings.app.as_deref() {
        Some(app) => Some(known(app).ok_or_else(|| AppError::Other(format!("terminale sconosciuto: {app}")))?),
        None if cfg!(target_os = "macos") => known("terminal"),
        None => {
            // Linux without a choice: $TERMINAL, as many tools do.
            if let Some(terminal) = std::env::var("TERMINAL").ok().filter(|t| !t.is_empty()) {
                spawn(Command::new(&terminal).arg("-e").arg(script))
                    .map_err(|e| AppError::Other(format!("{terminal}: {e}")))?;
                return Ok(shell_line([terminal.as_str(), "-e", &script_arg]));
            }
            KNOWN.iter().find(|k| locate(k).is_some())
        }
    };
    let Some(known) = chosen else {
        return Err(AppError::Other("nessun terminale trovato: scegline uno o imposta un comando nelle Impostazioni".into()));
    };
    let Some(location) = locate(known) else {
        return Err(AppError::Other(format!("{} non è installato: scegli un altro terminale nelle Impostazioni", known.name)));
    };
    let location_arg = location.to_string_lossy().into_owned();
    match known.start {
        Start::OpenFile | Start::OpenArgs(_) => {
            let mut args: Vec<String> = match known.start {
                Start::OpenArgs(extra) => {
                    let mut args = vec!["-na".to_string(), location_arg.clone(), "--args".into()];
                    args.extend(extra.iter().map(|a| a.to_string()));
                    args
                }
                _ => vec!["-a".to_string(), location_arg.clone()],
            };
            args.push(script_arg.clone());
            let output = Command::new("open").args(&args).output().await?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(AppError::Other(format!("impossibile aprire {}: {}", known.name, stderr.trim())));
            }
            Ok(shell_line(std::iter::once("open".to_string()).chain(args)))
        }
        Start::Exec(extra) => {
            let program = if known.binary.is_empty() { location } else { location.join(known.binary) };
            spawn(Command::new(&program).args(extra).arg(script))
                .map_err(|e| AppError::Other(format!("{}: {e}", known.name)))?;
            let mut line = vec![program.to_string_lossy().into_owned()];
            line.extend(extra.iter().map(|a| a.to_string()));
            line.push(script_arg);
            Ok(shell_line(line))
        }
    }
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
    fn finds_terminals_by_id_or_old_names() {
        let first = &KNOWN[0];
        assert_eq!(known(first.id).map(|k| k.id), Some(first.id));
        assert_eq!(known(&first.name.to_uppercase()).map(|k| k.id), Some(first.id));
        assert!(known("no-such-terminal").is_none());
        #[cfg(target_os = "macos")]
        {
            assert_eq!(known("iTerm").map(|k| k.id), Some("iterm"));
            assert!(installed().iter().any(|t| t.id == "terminal"));
        }
    }

    #[test]
    fn ssh_line_keeps_config_and_alias() {
        let line = ssh_line(Path::new("/usr/bin/ssh"), Some(Path::new("/c/ssh_config")), "gf-1", "/srv/www");
        assert!(line.starts_with("/usr/bin/ssh -F /c/ssh_config -t -o RemoteCommand=none -- gf-1 "), "{line}");
    }
}
