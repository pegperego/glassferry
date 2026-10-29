//! App settings the backend needs, in `settings.json` next to the
//! connections. UI-only preferences (layout, panel sizes) stay in the
//! webview's localStorage.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{AppError, Result};

pub const DEFAULT_SSH_BIN: &str = "/usr/bin/ssh";
/// `SSH_ASKPASS_REQUIRE`, which the in-app prompts rely on.
const MIN_SSH_VERSION: (u32, u32) = (8, 4);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Absolute path of the ssh binary; `None`: `/usr/bin/ssh`.
    pub ssh_bin: Option<String>,
    /// How "Apri nel terminale" opens the terminal.
    pub terminal: crate::terminal::TerminalSettings,
    /// Apps for editing remote files, by extension; others use the system default.
    pub editors: Vec<EditorRule>,
}

/// Remote files with one of these extensions open with `app`: on macOS an
/// application (name or path, for `open -a`), on Linux a command.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct EditorRule {
    /// Lowercase, without the dot.
    pub extensions: Vec<String>,
    pub app: String,
}

impl EditorRule {
    /// The rule for `file_name`, if any (the first matching one).
    pub fn find<'a>(rules: &'a [EditorRule], file_name: &str) -> Option<&'a EditorRule> {
        let extension = Path::new(file_name).extension()?.to_str()?.to_lowercase();
        rules.iter().find(|rule| rule.extensions.contains(&extension))
    }
}

impl Settings {
    pub fn ssh_bin(&self) -> PathBuf {
        PathBuf::from(self.ssh_bin.as_deref().unwrap_or(DEFAULT_SSH_BIN))
    }

    /// Empty strings mean "default"; paths must be absolute.
    pub fn normalized(mut self) -> Result<Self> {
        self.ssh_bin = self.ssh_bin.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
        self.terminal = self.terminal.normalized();
        let mut editors = Vec::new();
        for rule in self.editors {
            let extensions: Vec<String> = rule
                .extensions
                .iter()
                .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                .filter(|e| !e.is_empty())
                .collect();
            let app = rule.app.trim().to_string();
            if extensions.is_empty() && app.is_empty() {
                continue;
            }
            if extensions.is_empty() || app.is_empty() {
                return Err(AppError::Other("ogni regola di modifica vuole estensioni e app".into()));
            }
            if let Some(bad) = extensions.iter().find(|e| e.contains(['.', '/', ' ', '*'])) {
                return Err(AppError::Other(format!("estensione non valida: {bad}")));
            }
            editors.push(EditorRule { extensions, app });
        }
        self.editors = editors;
        if let Some(path) = &self.ssh_bin
            && !Path::new(path).is_absolute()
        {
            return Err(AppError::Other(format!("il percorso di ssh deve essere assoluto: {path}")));
        }
        Ok(self)
    }
}

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("settings.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Missing file: defaults. Unreadable file: defaults and the problem.
    pub fn load(&self) -> (Settings, Option<String>) {
        match std::fs::read(&self.path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(settings) => (settings, None),
                Err(e) => (Settings::default(), Some(format!("{}: {e}", self.path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => (Settings::default(), Some(format!("{}: {e}", self.path.display()))),
        }
    }

    pub fn save(&self, settings: &Settings) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(settings).map_err(|e| AppError::Other(e.to_string()))?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshCheck {
    /// First line of `ssh -V`.
    pub version: String,
    /// OpenSSH recent enough for the in-app prompts.
    pub supported: bool,
}

/// Runs `<path> -V` (the version goes to stderr).
pub async fn check_ssh(path: &Path) -> Result<SshCheck> {
    let output = Command::new(path)
        .arg("-V")
        .output()
        .await
        .map_err(|e| AppError::Other(format!("impossibile avviare {}: {e}", path.display())))?;
    let text = String::from_utf8_lossy(if output.stderr.is_empty() { &output.stdout } else { &output.stderr });
    let version = text.lines().next().unwrap_or_default().trim().to_string();
    if !output.status.success() || version.is_empty() {
        return Err(AppError::Other(format!("{} -V non ha risposto come ssh", path.display())));
    }
    let supported = openssh_version(&version).is_some_and(|v| v >= MIN_SSH_VERSION);
    Ok(SshCheck { version, supported })
}

/// `OpenSSH_9.8p1, LibreSSL 3.3.6` → (9, 8).
fn openssh_version(text: &str) -> Option<(u32, u32)> {
    let rest = text.strip_prefix("OpenSSH_")?;
    let mut parts = rest.split(|c: char| !c.is_ascii_digit());
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openssh_versions() {
        assert_eq!(openssh_version("OpenSSH_9.8p1, LibreSSL 3.3.6"), Some((9, 8)));
        assert_eq!(openssh_version("OpenSSH_8.4p1 Debian-5+deb11u3, OpenSSL 1.1.1w"), Some((8, 4)));
        assert_eq!(openssh_version("OpenSSH_for_Windows_8.1p1"), None);
        assert!(Some((8, 4)) >= Some(MIN_SSH_VERSION));
        assert!((8, 3) < MIN_SSH_VERSION);
    }

    #[tokio::test]
    async fn checks_the_system_ssh() {
        if !Path::new(DEFAULT_SSH_BIN).exists() {
            return;
        }
        let check = check_ssh(Path::new(DEFAULT_SSH_BIN)).await.unwrap();
        assert!(check.version.starts_with("OpenSSH_"), "{}", check.version);
        assert!(check_ssh(Path::new("/bin/false")).await.is_err());
        assert!(check_ssh(Path::new("/nonexistent/ssh")).await.is_err());
    }

    #[test]
    fn normalizes_and_matches_editor_rules() {
        let rule = |ext: &[&str], app: &str| EditorRule { extensions: ext.iter().map(|e| e.to_string()).collect(), app: app.into() };
        let settings = Settings {
            editors: vec![rule(&[" .Conf", "YML "], " Visual Studio Code "), rule(&[], "")],
            ..Default::default()
        }
        .normalized()
        .unwrap();
        assert_eq!(settings.editors, vec![rule(&["conf", "yml"], "Visual Studio Code")]);
        assert_eq!(EditorRule::find(&settings.editors, "nginx.CONF").map(|r| r.app.as_str()), Some("Visual Studio Code"));
        assert!(EditorRule::find(&settings.editors, "notes.txt").is_none());
        assert!(EditorRule::find(&settings.editors, "conf").is_none());
        let bad = Settings { editors: vec![rule(&["tar.gz"], "x")], ..Default::default() };
        assert!(bad.normalized().is_err());
        let half = Settings { editors: vec![rule(&["conf"], " ")], ..Default::default() };
        assert!(half.normalized().is_err());
    }

    #[test]
    fn normalizes_ssh_path() {
        let with = |path: &str| Settings { ssh_bin: Some(path.into()), ..Default::default() };
        let s = with("  ").normalized().unwrap();
        assert_eq!(s.ssh_bin, None);
        assert_eq!(s.ssh_bin(), PathBuf::from(DEFAULT_SSH_BIN));
        assert!(with("ssh").normalized().is_err());
        let s = with("/opt/homebrew/bin/ssh").normalized().unwrap();
        assert_eq!(s.ssh_bin(), PathBuf::from("/opt/homebrew/bin/ssh"));
    }
}
