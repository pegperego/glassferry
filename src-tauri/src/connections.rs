//! The app's own list of connections (`connections.json`), their categories,
//! and the `ssh_config` generated for connections configured in the app.
//!
//! A connection either points to an alias of `~/.ssh/config` (we store only
//! the alias and our metadata) or carries its own settings. The latter are
//! written as `Host gf-<id>` blocks to a generated file that ends with
//! `Include ~/.ssh/config`, and ssh is started with `-F <file>`: ssh still
//! interprets everything, we only write the config. Passwords never go to
//! these files: they live in the system keychain (`keyring`).

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{AppError, Result};

const STORE_FILE: &str = "connections.json";
const SSH_CONFIG_FILE: &str = "ssh_config";
const STORE_VERSION: u32 = 1;
const KEYCHAIN_SERVICE: &str = "glassferry";
/// Keywords that would break the structure of the generated file.
const FORBIDDEN_KEYWORDS: &[&str] = &["host", "match", "include"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: String,
    pub name: String,
    pub color: String,
    /// Destructive operations ask for a stronger confirmation.
    pub protected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Source {
    /// A `Host` alias of the user's ssh config.
    #[serde(rename_all = "camelCase")]
    SshConfig { alias: String },
    /// Settings stored in the app and written to the generated ssh_config.
    #[serde(rename_all = "camelCase")]
    App {
        host: String,
        #[serde(default)]
        user: Option<String>,
        #[serde(default)]
        port: Option<u16>,
        #[serde(default)]
        identity_file: Option<String>,
        #[serde(default)]
        proxy_jump: Option<String>,
        /// Extra `Keyword value` lines.
        #[serde(default)]
        extra_options: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub category_id: String,
    pub source: Source,
    #[serde(default)]
    pub remote_dir: Option<String>,
    #[serde(default)]
    pub local_dir: Option<String>,
    #[serde(default)]
    pub save_password: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    pub version: u32,
    pub categories: Vec<Category>,
    pub connections: Vec<Connection>,
}

impl Default for Store {
    fn default() -> Self {
        let category = |id: &str, name: &str, color: &str, protected| Category {
            id: id.into(),
            name: name.into(),
            color: color.into(),
            protected,
        };
        Self {
            version: STORE_VERSION,
            categories: vec![
                category("dev", "Sviluppo", "#2e9d5b", false),
                category("test", "Test", "#d9a400", false),
                category("prod", "Produzione", "#d23c3c", true),
            ],
            connections: Vec::new(),
        }
    }
}

impl Store {
    pub fn connection(&self, id: &str) -> Result<&Connection> {
        self.connections
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| AppError::Other(format!("connessione {id} non trovata")))
    }

    pub fn validate(&self) -> Result<()> {
        let mut ids = HashSet::new();
        for category in &self.categories {
            check_id(&category.id)?;
            if !ids.insert(&category.id) {
                return invalid(format!("categoria duplicata: {}", category.id));
            }
            if category.name.trim().is_empty() {
                return invalid("una categoria non ha nome");
            }
        }
        let mut connection_ids = HashSet::new();
        for connection in &self.connections {
            check_id(&connection.id)?;
            if !connection_ids.insert(&connection.id) {
                return invalid(format!("connessione duplicata: {}", connection.id));
            }
            if connection.name.trim().is_empty() {
                return invalid("una connessione non ha nome");
            }
            if !ids.contains(&connection.category_id) {
                return invalid(format!("«{}»: categoria inesistente", connection.name));
            }
            validate_source(&connection.source).map_err(|e| AppError::Other(format!("«{}»: {e}", connection.name)))?;
        }
        Ok(())
    }
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(AppError::Other(message.into()))
}

/// Ids end up in host aliases and keychain accounts.
fn check_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return invalid(format!("id non valido: {id:?}"));
    }
    Ok(())
}

/// A single-word value: no whitespace, quotes or control characters.
fn check_word(field: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"') {
        return invalid(format!("{field} non valido: {value:?}"));
    }
    Ok(())
}

/// A value that may contain spaces (it is quoted in the file).
fn check_text(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().any(|c| c.is_control() || c == '"') {
        return invalid(format!("{field} non valido: {value:?}"));
    }
    Ok(())
}

pub fn validate_source(source: &Source) -> Result<()> {
    match source {
        Source::SshConfig { alias } => check_word("alias", alias),
        Source::App {
            host,
            user,
            port,
            identity_file,
            proxy_jump,
            extra_options,
        } => {
            check_word("host", host)?;
            if let Some(user) = user {
                check_word("utente", user)?;
            }
            if *port == Some(0) {
                return invalid("porta non valida");
            }
            if let Some(file) = identity_file {
                check_text("chiave", file)?;
            }
            if let Some(jump) = proxy_jump {
                check_word("ProxyJump", jump)?;
            }
            for line in extra_options {
                parse_option(line)?;
            }
            Ok(())
        }
    }
}

/// Splits `Keyword value` (or `Keyword=value`) and rejects keywords that
/// would change the structure of the generated file.
fn parse_option(line: &str) -> Result<(&str, &str)> {
    let line = line.trim();
    let end = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    let (keyword, value) = line.split_at(end);
    let value = value.trim_start().strip_prefix('=').unwrap_or(value).trim();
    if keyword.is_empty() || !keyword.chars().all(|c| c.is_ascii_alphanumeric()) || value.is_empty() {
        return invalid(format!("opzione non valida: {line:?} (formato: Chiave valore)"));
    }
    if FORBIDDEN_KEYWORDS.contains(&keyword.to_ascii_lowercase().as_str()) {
        return invalid(format!("opzione non ammessa: {keyword}"));
    }
    if value.chars().any(char::is_control) {
        return invalid(format!("opzione non valida: {line:?}"));
    }
    Ok((keyword, value))
}

/// Alias of an app connection inside the generated config.
pub fn app_alias(id: &str) -> String {
    format!("gf-{id}")
}

fn quote_value(value: &str) -> String {
    if value.contains(char::is_whitespace) {
        format!("\"{value}\"")
    } else {
        value.to_string()
    }
}

fn host_block(alias: &str, source: &Source) -> String {
    let Source::App {
        host,
        user,
        port,
        identity_file,
        proxy_jump,
        extra_options,
    } = source
    else {
        return String::new();
    };
    let mut block = format!("Host {alias}\n  HostName {host}\n");
    if let Some(user) = user {
        block.push_str(&format!("  User {user}\n"));
    }
    if let Some(port) = port {
        block.push_str(&format!("  Port {port}\n"));
    }
    if let Some(file) = identity_file {
        block.push_str(&format!("  IdentityFile {}\n", quote_value(file)));
    }
    if let Some(jump) = proxy_jump {
        block.push_str(&format!("  ProxyJump {jump}\n"));
    }
    for line in extra_options {
        if let Ok((keyword, value)) = parse_option(line) {
            block.push_str(&format!("  {keyword} {value}\n"));
        }
    }
    block.push('\n');
    block
}

/// The generated config: our blocks first (in ssh the first value wins),
/// then the user's config for defaults (`Host *`, `IdentityAgent`) and for
/// aliases used as ProxyJump. `-F` skips the system file, so it is included
/// too, as ssh would read it last without `-F`.
pub fn render_ssh_config<'a>(blocks: impl IntoIterator<Item = (String, &'a Source)>) -> String {
    let mut out = String::from("# Generato da Glassferry: non modificare a mano.\n\n");
    for (alias, source) in blocks {
        out.push_str(&host_block(&alias, source));
    }
    out.push_str("Match all\nInclude ~/.ssh/config\nInclude /etc/ssh/ssh_config\n");
    out
}

/// Writes a file atomically with mode 0600.
fn write_private(path: &Path, content: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(content)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Where ssh must find a connection: alias plus optional `-F` file.
#[derive(Debug, Clone)]
pub struct SshTarget {
    pub alias: String,
    pub config_file: Option<PathBuf>,
}

pub struct ConnectionStore {
    dir: PathBuf,
}

impl ConnectionStore {
    /// `~/Library/Application Support/glassferry` (Linux: `$XDG_CONFIG_HOME/glassferry`).
    pub fn default_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("glassferry"))
    }

    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn ssh_config_path(&self) -> PathBuf {
        self.dir.join(SSH_CONFIG_FILE)
    }

    /// Missing file = default categories and no connections.
    pub fn load(&self) -> Result<Store> {
        match std::fs::read(self.dir.join(STORE_FILE)) {
            Ok(bytes) => parse_store(&bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Renames an unreadable `connections.json` so it is never overwritten.
    pub fn set_aside(&self) -> Result<PathBuf> {
        let from = self.dir.join(STORE_FILE);
        let to = self.dir.join(format!("{STORE_FILE}.broken-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        std::fs::rename(&from, &to)?;
        Ok(to)
    }

    pub fn save(&self, store: &Store) -> Result<()> {
        store.validate()?;
        std::fs::create_dir_all(&self.dir)?;
        write_private(&self.dir.join(STORE_FILE), &serde_json::to_vec_pretty(store).map_err(other)?)?;
        self.write_ssh_config(store)
    }

    /// Regenerates the ssh_config from the store (also at startup, so a
    /// hand-edited or missing file is repaired).
    pub fn write_ssh_config(&self, store: &Store) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let blocks = store.connections.iter().map(|c| (app_alias(&c.id), &c.source));
        write_private(&self.ssh_config_path(), render_ssh_config(blocks).as_bytes())
    }

    pub fn target(&self, connection: &Connection) -> SshTarget {
        match &connection.source {
            Source::SshConfig { alias } => SshTarget {
                alias: alias.clone(),
                config_file: None,
            },
            Source::App { .. } => SshTarget {
                alias: app_alias(&connection.id),
                config_file: Some(self.ssh_config_path()),
            },
        }
    }
}

fn other(e: impl std::fmt::Display) -> AppError {
    AppError::Other(e.to_string())
}

pub fn parse_store(bytes: &[u8]) -> Result<Store> {
    let store: Store = serde_json::from_slice(bytes).map_err(|e| AppError::Other(format!("file connessioni non valido: {e}")))?;
    if store.version > STORE_VERSION {
        return invalid("file connessioni creato da una versione più recente dell'app");
    }
    Ok(store)
}

/// Effective settings as resolved by `ssh -G` for a source that may not be
/// saved yet: app sources are written to a temporary config first, so a bad
/// option is reported by ssh itself.
pub async fn check_source(ssh_bin: &Path, scratch_dir: &Path, source: &Source) -> Result<BTreeMap<String, String>> {
    validate_source(source)?;
    match source {
        Source::SshConfig { alias } => resolve(ssh_bin, None, alias).await,
        Source::App { .. } => {
            let file = scratch_dir.join(format!("check-{}.conf", crate::runtime_dir::random_token()?));
            let alias = "gf-check";
            write_private(&file, render_ssh_config([(alias.to_string(), source)]).as_bytes())?;
            let result = resolve(ssh_bin, Some(&file), alias).await;
            let _ = std::fs::remove_file(&file);
            // "<tmp file>: line 3: Bad configuration option: x" → keep ssh's
            // words, drop our temporary path and the summary line.
            result.map_err(|e| match e {
                AppError::Ssh(message) => AppError::Ssh(
                    message
                        .lines()
                        .filter(|l| !l.contains("terminating"))
                        .map(|l| l.replace(&*file.to_string_lossy(), "configurazione"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                other => other,
            })
        }
    }
}

/// `ssh [-F file] -G alias`, multi-valued options joined with newlines.
pub async fn resolve(ssh_bin: &Path, config_file: Option<&Path>, alias: &str) -> Result<BTreeMap<String, String>> {
    let mut cmd = Command::new(ssh_bin);
    if let Some(file) = config_file {
        cmd.arg("-F").arg(file);
    }
    let output = cmd
        .arg("-G")
        .arg("--")
        .arg(alias)
        .stdin(std::process::Stdio::null())
        .output()
        .await?;
    if !output.status.success() {
        return Err(AppError::Ssh(String::from_utf8_lossy(&output.stderr).trim().to_string()));
    }
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((key, value)) = line.split_once(' ') {
            map.entry(key.to_string())
                .and_modify(|v| {
                    v.push('\n');
                    v.push_str(value);
                })
                .or_insert_with(|| value.to_string());
        }
    }
    Ok(map)
}

/// Saved passwords, one keychain item per connection id.
pub mod keychain {
    use super::*;

    fn entry(connection_id: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, connection_id).map_err(other)
    }

    pub fn get(connection_id: &str) -> Result<Option<String>> {
        match entry(connection_id)?.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(other(e)),
        }
    }

    pub fn set(connection_id: &str, password: &str) -> Result<()> {
        entry(connection_id)?.set_password(password).map_err(other)
    }

    pub fn delete(connection_id: &str) -> Result<()> {
        match entry(connection_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(other(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_source() -> Source {
        Source::App {
            host: "10.0.0.5".into(),
            user: Some("deploy".into()),
            port: Some(2222),
            identity_file: Some("~/.ssh/my key".into()),
            proxy_jump: Some("bastion".into()),
            extra_options: vec!["ServerAliveInterval=30".into()],
        }
    }

    #[test]
    fn renders_app_blocks_before_user_config() {
        let ssh_config = Source::SshConfig { alias: "web".into() };
        let source = app_source();
        let out = render_ssh_config([("gf-a".to_string(), &source), ("gf-b".to_string(), &ssh_config)]);
        assert!(
            out.contains(
                "Host gf-a\n  HostName 10.0.0.5\n  User deploy\n  Port 2222\n  IdentityFile \"~/.ssh/my key\"\n  ProxyJump bastion\n  ServerAliveInterval 30\n\n"
            ),
            "{out}"
        );
        assert!(!out.contains("gf-b"));
        assert!(out.ends_with("Match all\nInclude ~/.ssh/config\nInclude /etc/ssh/ssh_config\n"));
    }

    #[test]
    fn rejects_options_that_break_the_file() {
        assert!(parse_option("Include /etc/passwd").is_err());
        assert!(parse_option("host x").is_err());
        assert!(parse_option("Compression").is_err());
        assert_eq!(parse_option("  Compression yes ").unwrap(), ("Compression", "yes"));
        let mut source = app_source();
        if let Source::App { host, .. } = &mut source {
            *host = "a\nHost *".into();
        }
        assert!(validate_source(&source).is_err());
    }

    #[test]
    fn validates_store_references() {
        let mut store = Store::default();
        store.connections.push(Connection {
            id: "abc-1".into(),
            name: "Web".into(),
            category_id: "prod".into(),
            source: app_source(),
            remote_dir: None,
            local_dir: None,
            save_password: false,
        });
        assert!(store.validate().is_ok());
        store.connections[0].category_id = "missing".into();
        assert!(store.validate().is_err());
        store.connections[0].category_id = "prod".into();
        store.connections[0].id = "../x".into();
        assert!(store.validate().is_err());
    }

    #[test]
    fn saves_and_loads_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("gf-conn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store_dir = ConnectionStore::new(dir.clone());
        assert_eq!(store_dir.load().unwrap(), Store::default());
        let mut store = Store::default();
        store.connections.push(Connection {
            id: "c1".into(),
            name: "Staging".into(),
            category_id: "test".into(),
            source: app_source(),
            remote_dir: Some("/srv".into()),
            local_dir: None,
            save_password: true,
        });
        store_dir.save(&store).unwrap();
        assert_eq!(store_dir.load().unwrap(), store);
        let mode = std::fs::metadata(store_dir.ssh_config_path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(std::fs::read_to_string(store_dir.ssh_config_path()).unwrap().contains("Host gf-c1\n"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
