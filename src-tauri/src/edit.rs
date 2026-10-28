//! Editing remote files with the system's default application.
//!
//! The file is downloaded into a private cache directory and opened. The
//! directory (not the file: many editors save by writing a new file and
//! renaming it) is watched; after a short debounce, a content change is
//! uploaded. Before each upload the remote file is compared with the version
//! that was opened (size, mtime and, when the server has `sha256sum`, the
//! content hash): if someone else changed it, the user decides.
//!
//! Uploads write the file in place (truncate + write) so owner and mode are
//! preserved; the trade-off is that a dropped connection can leave it partial.
//!
//! On protected connections (`confirm_saves`), the first upload of each open
//! file waits for an explicit confirmation (`edit-confirm` event).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};

use crate::error::{AppError, Result};
use crate::log::Channel;
use crate::session::{sh_command, Session};

const DEBOUNCE: Duration = Duration::from_millis(500);
/// Larger files are read into memory to edit: refuse beyond this.
const MAX_EDIT_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EditState {
    /// Local copy equals the remote file.
    Synced,
    Uploading,
    /// The remote file changed since it was opened: waiting for the user.
    Conflict,
    /// Last upload failed; the next save retries.
    Error,
    /// The upload was not confirmed (protected connection): local changes only.
    Unconfirmed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditInfo {
    pub id: u64,
    pub session_id: u64,
    pub host: String,
    pub remote_path: String,
    pub local_path: String,
    pub state: EditState,
    pub uploads: u64,
    /// Milliseconds since the Unix epoch.
    pub last_sync_at: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditConflict {
    pub id: u64,
    pub host: String,
    pub remote_path: String,
    pub opened_size: u64,
    pub opened_modified: Option<u64>,
    pub current_size: u64,
    pub current_modified: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConflictChoice {
    /// Upload the local version anyway.
    Overwrite,
    /// Replace the local copy with the server version (local kept as backup).
    Reload,
    /// Do nothing for now; the next save asks again.
    Keep,
}

/// Confirmation asked before the first upload on a protected connection.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditConfirm {
    pub id: u64,
    pub host: String,
    pub remote_path: String,
    pub size: u64,
    /// How the upload is carried out (preview).
    pub commands: Vec<String>,
}

pub enum EditEvent {
    Update(EditInfo),
    Conflict(EditConflict),
    ConfirmSave(EditConfirm),
}

type Sink = Arc<dyn Fn(EditEvent) + Send + Sync>;

/// What the remote file looked like when we last synced with it.
#[derive(Clone)]
struct RemoteState {
    size: u64,
    modified: Option<u64>,
    /// Content hash; `None` when the server cannot compute it.
    sha256: Option<String>,
}

struct Edit {
    session: Arc<Session>,
    info: Mutex<EditInfo>,
    remote: Mutex<RemoteState>,
    /// Hash of the local content last uploaded or downloaded.
    synced_hash: Mutex<String>,
    conflict_reply: Mutex<Option<oneshot::Sender<ConflictChoice>>>,
    /// The next upload needs a confirmation (protected connection, not confirmed yet).
    confirm_saves: AtomicBool,
    confirm_reply: Mutex<Option<oneshot::Sender<bool>>>,
    /// Dropping the watcher stops it.
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

pub struct EditManager {
    root: PathBuf,
    sink: Sink,
    next_id: AtomicU64,
    edits: Mutex<HashMap<u64, Arc<Edit>>>,
    /// Off in headless tests: no editor window.
    launch_editor: AtomicBool,
}

impl EditManager {
    /// `root` holds one private subdirectory per open file.
    pub fn new(root: PathBuf, sink: Sink) -> Arc<Self> {
        Arc::new(Self {
            root,
            sink,
            next_id: AtomicU64::new(1),
            edits: Mutex::new(HashMap::new()),
            launch_editor: AtomicBool::new(true),
        })
    }

    pub fn set_launch_editor(&self, launch: bool) {
        self.launch_editor.store(launch, Ordering::Relaxed);
    }

    fn launch(&self, path: &str) -> Result<()> {
        if self.launch_editor.load(Ordering::Relaxed) {
            open_with_default_app(path)
        } else {
            Ok(())
        }
    }

    pub fn default_root() -> Option<PathBuf> {
        dirs::cache_dir().map(|d| d.join("sshfilemanager/edit"))
    }

    pub fn list(&self) -> Vec<EditInfo> {
        let mut list: Vec<EditInfo> = self.edits.lock().unwrap().values().map(|e| e.info.lock().unwrap().clone()).collect();
        list.sort_by_key(|e| e.id);
        list
    }

    fn get(&self, id: u64) -> Result<Arc<Edit>> {
        self.edits
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| AppError::Other("file non più aperto".into()))
    }

    fn emit(&self, edit: &Edit) {
        (self.sink)(EditEvent::Update(edit.info.lock().unwrap().clone()));
    }

    fn set_state(&self, edit: &Edit, state: EditState, error: Option<String>) {
        {
            let mut info = edit.info.lock().unwrap();
            info.state = state;
            info.error = error;
        }
        self.emit(edit);
    }

    /// Downloads `remote_path`, opens it with the default application and
    /// starts watching it. Opening the same file twice reuses the copy.
    /// `confirm_saves`: ask before the first upload (protected connection).
    pub async fn open(
        self: &Arc<Self>,
        session_id: u64,
        session: Arc<Session>,
        remote_path: String,
        confirm_saves: bool,
    ) -> Result<EditInfo> {
        let existing = self
            .edits
            .lock()
            .unwrap()
            .values()
            .find(|e| {
                let info = e.info.lock().unwrap();
                info.session_id == session_id && info.remote_path == remote_path
            })
            .cloned();
        if let Some(edit) = existing {
            let info = edit.info.lock().unwrap().clone();
            self.launch(&info.local_path)?;
            return Ok(info);
        }

        let host = session.launcher.label.clone();
        let file_name = Path::new(&remote_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| AppError::Other("percorso non valido".into()))?;
        let dir = self.root.join(short_hash(&format!("{host}\0{remote_path}")));
        std::fs::create_dir_all(&dir)?;
        restrict_permissions(&self.root);
        let local_path = dir.join(&file_name);

        let (content, remote) = download(&session, &remote_path).await?;
        tokio::fs::write(&local_path, &content).await?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let edit = Arc::new(Edit {
            session,
            info: Mutex::new(EditInfo {
                id,
                session_id,
                host,
                remote_path,
                local_path: local_path.to_string_lossy().into_owned(),
                state: EditState::Synced,
                uploads: 0,
                last_sync_at: now_ms(),
                error: None,
            }),
            remote: Mutex::new(remote),
            synced_hash: Mutex::new(sha256_hex(&content)),
            conflict_reply: Mutex::new(None),
            confirm_saves: AtomicBool::new(confirm_saves),
            confirm_reply: Mutex::new(None),
            watcher: Mutex::new(None),
        });

        let (tx, rx) = mpsc::unbounded_channel();
        let watched_name = std::ffi::OsString::from(&file_name);
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event
                && event.paths.iter().any(|p| p.file_name() == Some(watched_name.as_os_str()))
            {
                let _ = tx.send(());
            }
        })
        .map_err(|e| AppError::Other(format!("watcher: {e}")))?;
        notify::Watcher::watch(&mut watcher, &dir, notify::RecursiveMode::NonRecursive)
            .map_err(|e| AppError::Other(format!("watcher: {e}")))?;
        *edit.watcher.lock().unwrap() = Some(watcher);

        self.edits.lock().unwrap().insert(id, edit.clone());
        let manager = self.clone();
        let watched = edit.clone();
        tauri::async_runtime::spawn(async move { manager.watch_loop(watched, rx).await });

        self.launch(&local_path.to_string_lossy())?;
        self.emit(&edit);
        let info = edit.info.lock().unwrap().clone();
        Ok(info)
    }

    async fn watch_loop(self: Arc<Self>, edit: Arc<Edit>, mut rx: mpsc::UnboundedReceiver<()>) {
        while rx.recv().await.is_some() {
            // Debounce: editors emit several events per save.
            loop {
                match tokio::time::timeout(DEBOUNCE, rx.recv()).await {
                    Ok(Some(())) => continue,
                    Ok(None) => return,
                    Err(_) => break,
                }
            }
            self.sync(&edit).await;
        }
    }

    /// Uploads the local copy if its content changed since the last sync.
    async fn sync(&self, edit: &Arc<Edit>) {
        self.sync_with(edit, false).await
    }

    /// `force`: upload even if unchanged and without the conflict check.
    async fn sync_with(&self, edit: &Arc<Edit>, force: bool) {
        let local_path = edit.info.lock().unwrap().local_path.clone();
        let Ok(content) = tokio::fs::read(&local_path).await else {
            // Mid-rename or deleted by the editor: wait for the next event.
            return;
        };
        let hash = sha256_hex(&content);
        if !force && *edit.synced_hash.lock().unwrap() == hash {
            return;
        }

        let remote_path = edit.info.lock().unwrap().remote_path.clone();
        let opened = edit.remote.lock().unwrap().clone();
        let current = match remote_state(&edit.session, &remote_path, opened.sha256.is_some()).await {
            Ok(state) => state,
            Err(e) => return self.set_state(edit, EditState::Error, Some(e.to_string())),
        };
        let changed = current.size != opened.size
            || current.modified != opened.modified
            || (opened.sha256.is_some() && current.sha256 != opened.sha256);
        if changed && !force {
            match self.ask_conflict(edit, &opened, &current).await {
                ConflictChoice::Overwrite => {}
                ConflictChoice::Keep => return,
                ConflictChoice::Reload => return self.reload(edit, &content).await,
            }
        }

        if edit.confirm_saves.load(Ordering::Relaxed) {
            if !self.ask_confirm(edit, content.len() as u64).await {
                return self.set_state(
                    edit,
                    EditState::Unconfirmed,
                    Some("caricamento non confermato: le modifiche sono solo nella copia locale".into()),
                );
            }
            edit.confirm_saves.store(false, Ordering::Relaxed);
        }

        self.set_state(edit, EditState::Uploading, None);
        match upload(&edit.session, &remote_path, &content).await {
            Ok(mut state) => {
                if opened.sha256.is_some() {
                    state.sha256 = Some(hash.clone());
                }
                *edit.remote.lock().unwrap() = state;
                *edit.synced_hash.lock().unwrap() = hash;
                {
                    let mut info = edit.info.lock().unwrap();
                    info.uploads += 1;
                    info.last_sync_at = now_ms();
                }
                self.set_state(edit, EditState::Synced, None);
            }
            Err(e) => self.set_state(edit, EditState::Error, Some(e.to_string())),
        }
    }

    async fn ask_confirm(&self, edit: &Arc<Edit>, size: u64) -> bool {
        let (tx, rx) = oneshot::channel();
        *edit.confirm_reply.lock().unwrap() = Some(tx);
        let info = edit.info.lock().unwrap().clone();
        let commands = vec![format!(
            "sftp> put {} {}   # scrittura diretta: mantiene proprietario e permessi",
            crate::log::sftp_quote(&info.local_path),
            crate::log::sftp_quote(&info.remote_path)
        )];
        (self.sink)(EditEvent::ConfirmSave(EditConfirm {
            id: info.id,
            host: info.host,
            remote_path: info.remote_path,
            size,
            commands,
        }));
        rx.await.unwrap_or(false)
    }

    /// Answers a pending upload confirmation; without one, `allow` retries
    /// the upload of changes left unconfirmed (which asks again).
    pub fn confirm_save(self: &Arc<Self>, id: u64, allow: bool) -> Result<()> {
        let edit = self.get(id)?;
        if let Some(tx) = edit.confirm_reply.lock().unwrap().take() {
            let _ = tx.send(allow);
            return Ok(());
        }
        if allow {
            let manager = self.clone();
            tauri::async_runtime::spawn(async move { manager.sync(&edit).await });
        }
        Ok(())
    }

    async fn ask_conflict(&self, edit: &Arc<Edit>, opened: &RemoteState, current: &RemoteState) -> ConflictChoice {
        let (tx, rx) = oneshot::channel();
        *edit.conflict_reply.lock().unwrap() = Some(tx);
        self.set_state(edit, EditState::Conflict, None);
        let info = edit.info.lock().unwrap().clone();
        (self.sink)(EditEvent::Conflict(EditConflict {
            id: info.id,
            host: info.host,
            remote_path: info.remote_path,
            opened_size: opened.size,
            opened_modified: opened.modified,
            current_size: current.size,
            current_modified: current.modified,
        }));
        let choice = rx.await.unwrap_or(ConflictChoice::Keep);
        if choice == ConflictChoice::Keep {
            self.set_state(edit, EditState::Conflict, Some("modifiche locali non caricate".into()));
        }
        choice
    }

    /// Answers a pending conflict prompt, or acts on a conflict left pending
    /// earlier ("keep"): overwrite uploads now, reload downloads now.
    pub fn resolve(self: &Arc<Self>, id: u64, choice: ConflictChoice) -> Result<()> {
        let edit = self.get(id)?;
        if let Some(tx) = edit.conflict_reply.lock().unwrap().take() {
            let _ = tx.send(choice);
            return Ok(());
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            match choice {
                ConflictChoice::Overwrite => manager.sync_with(&edit, true).await,
                ConflictChoice::Reload => {
                    let local_path = edit.info.lock().unwrap().local_path.clone();
                    let content = tokio::fs::read(&local_path).await.unwrap_or_default();
                    manager.reload(&edit, &content).await
                }
                ConflictChoice::Keep => {}
            }
        });
        Ok(())
    }

    /// Replaces the local copy with the server version; the local content is
    /// saved next to it as "<name> (mia versione)".
    async fn reload(&self, edit: &Arc<Edit>, local_content: &[u8]) {
        let (remote_path, local_path) = {
            let info = edit.info.lock().unwrap();
            (info.remote_path.clone(), PathBuf::from(&info.local_path))
        };
        let backup = backup_path(&local_path);
        if let Err(e) = tokio::fs::write(&backup, local_content).await {
            return self.set_state(edit, EditState::Error, Some(format!("backup: {e}")));
        }
        match download(&edit.session, &remote_path).await {
            Ok((content, state)) => {
                // Update the hash first: the write below triggers the watcher.
                *edit.synced_hash.lock().unwrap() = sha256_hex(&content);
                *edit.remote.lock().unwrap() = state;
                if let Err(e) = tokio::fs::write(&local_path, &content).await {
                    return self.set_state(edit, EditState::Error, Some(e.to_string()));
                }
                edit.info.lock().unwrap().last_sync_at = now_ms();
                self.set_state(
                    edit,
                    EditState::Synced,
                    Some(format!("versione del server caricata; la tua è in {}", backup.display())),
                );
            }
            Err(e) => self.set_state(edit, EditState::Error, Some(e.to_string())),
        }
    }

    pub fn reopen(&self, id: u64) -> Result<()> {
        self.launch(&self.get(id)?.info.lock().unwrap().local_path)
    }

    pub fn local_path(&self, id: u64) -> Result<String> {
        Ok(self.get(id)?.info.lock().unwrap().local_path.clone())
    }

    /// Stops watching and removes the local copy.
    pub fn close(&self, id: u64) {
        let Some(edit) = self.edits.lock().unwrap().remove(&id) else { return };
        edit.watcher.lock().unwrap().take();
        if let Some(tx) = edit.conflict_reply.lock().unwrap().take() {
            let _ = tx.send(ConflictChoice::Keep);
        }
        if let Some(tx) = edit.confirm_reply.lock().unwrap().take() {
            let _ = tx.send(false);
        }
        if let Some(dir) = Path::new(&edit.info.lock().unwrap().local_path).parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
        edit.session
            .launcher
            .logger
            .start(Some(&edit.session.launcher.label), Channel::Local, "edit-close", edit.info.lock().unwrap().remote_path.clone())
            .ok(None);
    }

    pub fn close_session(&self, session_id: u64) {
        let ids: Vec<u64> = self
            .edits
            .lock()
            .unwrap()
            .values()
            .map(|e| e.info.lock().unwrap().clone())
            .filter(|i| i.session_id == session_id)
            .map(|i| i.id)
            .collect();
        for id in ids {
            self.close(id);
        }
    }

    /// On exit: keep copies with changes that were never uploaded.
    pub fn close_all_synced(&self) {
        let ids: Vec<u64> = self
            .list()
            .into_iter()
            .filter(|i| i.state == EditState::Synced)
            .map(|i| i.id)
            .collect();
        for id in ids {
            self.close(id);
        }
    }
}

// ---- remote I/O ----------------------------------------------------------------

async fn remote_state(session: &Session, path: &str, with_hash: bool) -> Result<RemoteState> {
    let meta = session.sftp.fs().metadata(path).await?;
    let sha256 = if with_hash { remote_sha256(session, path).await } else { None };
    Ok(RemoteState {
        size: meta.len().unwrap_or(0),
        modified: meta.modified().map(|t| t.as_duration().as_secs()),
        sha256,
    })
}

async fn remote_sha256(session: &Session, path: &str) -> Option<String> {
    if !session.capabilities.commands.iter().any(|c| c == "sha256sum") {
        return None;
    }
    let output = session
        .launcher
        .exec("sha256sum", &sh_command(r#"sha256sum -- "$1""#, &[path]))
        .await
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let hash = stdout.split_whitespace().next()?.trim_start_matches('\\');
    (output.status.success() && hash.len() == 64).then(|| hash.to_ascii_lowercase())
}

async fn download(session: &Session, path: &str) -> Result<(Vec<u8>, RemoteState)> {
    let mut op = session.launcher.log(Channel::Sftp, "edit-open", path);
    op.equivalent(session.launcher.sftp_equivalent("get", &[path]));
    let result = async {
        let meta = session.sftp.fs().metadata(path).await?;
        if meta.file_type().is_some_and(|t| t.is_dir()) {
            return Err(AppError::Other("è una cartella".into()));
        }
        if meta.len().unwrap_or(0) > MAX_EDIT_BYTES {
            return Err(AppError::Other(format!(
                "file troppo grande da modificare ({}): scaricalo",
                crate::transfer::format_bytes(meta.len().unwrap_or(0))
            )));
        }
        let content = session.sftp.fs().read(path).await?.to_vec();
        let sha256 = remote_sha256(session, path).await;
        // Hash from the server and of what we read must agree; otherwise the
        // file changed while we were reading: just keep the size/mtime check.
        let sha256 = sha256.filter(|h| *h == sha256_hex(&content));
        Ok((
            content,
            RemoteState {
                size: meta.len().unwrap_or(0),
                modified: meta.modified().map(|t| t.as_duration().as_secs()),
                sha256,
            },
        ))
    }
    .await;
    op.result(result, |(content, _)| Some(crate::transfer::format_bytes(content.len() as u64)))
}

async fn upload(session: &Session, path: &str, content: &[u8]) -> Result<RemoteState> {
    let mut op = session.launcher.log(Channel::Sftp, "edit-save", path);
    op.equivalent(session.launcher.sftp_equivalent("put", &["<copia locale>", path]));
    let result = async {
        let mut file = session.sftp.options().write(true).create(true).truncate(true).open(path).await?;
        file.write_all(content).await?;
        file.close().await?;
        remote_state(session, path, false).await
    }
    .await;
    op.result(result, |_| Some(crate::transfer::format_bytes(content.len() as u64)))
}

// ---- helpers -------------------------------------------------------------------

fn open_with_default_app(path: &str) -> Result<()> {
    let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(program)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| AppError::Other(format!("impossibile aprire il file: {e}")))
}

fn restrict_permissions(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

fn backup_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match path.extension() {
        Some(ext) => format!("{stem} (mia versione).{}", ext.to_string_lossy()),
        None => format!("{stem} (mia versione)"),
    };
    path.with_file_name(name)
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn short_hash(text: &str) -> String {
    sha256_hex(text.as_bytes())[..16].to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_backup_names() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(backup_path(Path::new("/x/nginx.conf")), Path::new("/x/nginx (mia versione).conf"));
        assert_eq!(backup_path(Path::new("/x/Makefile")), Path::new("/x/Makefile (mia versione)"));
    }
}
