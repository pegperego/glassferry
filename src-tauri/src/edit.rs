//! Editing remote files with the system's default application.
//!
//! The file is downloaded into a private cache directory and opened. The
//! directory (not the file: many editors save by writing a new file and
//! renaming it) is watched; after a short debounce, a content change is
//! uploaded. Before each upload the remote file is compared with the version
//! that was opened: size and mtime, then, when those match, the content hash
//! (`sha256sum` on the server, or the file read over SFTP on accounts without
//! a shell: SFTP v3 mtimes have 1 s resolution). If someone else changed it,
//! the user decides.
//!
//! Uploads write `<name>.gf-part` and rename it over the file (posix-rename,
//! one step), so a dropped connection never leaves it partial; the mode is
//! copied. When replacing would lose something visible over SFTP (the path
//! is a symlink, owner or group would change, no posix-rename, the folder is
//! not writable) the file is written in place (truncate + write) instead.
//!
//! On protected connections (`confirm_saves`), the first upload of each open
//! file waits for an explicit confirmation (`edit-confirm` event).
//!
//! Local changes are never dropped: on disconnect a file with changes not on
//! the server is *detached* (copy kept, no session) and reattached when the
//! same connection connects again, which uploads it with the usual conflict
//! check. Open files are recorded in `<root>/edits.json`, so detached copies
//! survive a restart or a crash.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use openssh_sftp_client::metadata::MetaDataBuilder;
use tokio::sync::{mpsc, oneshot};

use crate::error::{AppError, Result};
use crate::log::{Channel, Logger};
use crate::session::{sh_command, Session};

const DEBOUNCE: Duration = Duration::from_millis(500);
/// Larger files are read into memory to edit: refuse beyond this.
const MAX_EDIT_BYTES: u64 = 100 * 1024 * 1024;
const MANIFEST: &str = "edits.json";
const DETACHED_MESSAGE: &str = "verranno caricate alla prossima connessione";

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
    /// Local changes kept while the connection is closed; uploaded on reconnect.
    Detached,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditInfo {
    pub id: u64,
    /// `None` while detached.
    pub session_id: Option<u64>,
    pub connection_id: String,
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
#[derive(Clone, Serialize, Deserialize)]
struct RemoteState {
    size: u64,
    modified: Option<u64>,
    /// Content hash; `None` when the server cannot compute it.
    sha256: Option<String>,
}

/// One entry of the manifest: enough to reattach a copy after a restart.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedEdit {
    connection_id: String,
    host: String,
    remote_path: String,
    local_path: String,
    remote: RemoteState,
    synced_hash: String,
    confirm_saves: bool,
    uploads: u64,
    last_sync_at: u64,
}

struct Edit {
    /// `None` while detached.
    session: Mutex<Option<Arc<Session>>>,
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
    logger: Arc<Logger>,
    sink: Sink,
    next_id: AtomicU64,
    edits: Mutex<HashMap<u64, Arc<Edit>>>,
    /// Off in headless tests: no editor window.
    launch_editor: AtomicBool,
    /// Apps by extension (settings); other files open with the default app.
    editors: Mutex<Vec<crate::settings::EditorRule>>,
}

impl EditManager {
    /// `root` holds one private subdirectory per open file and the manifest;
    /// copies with changes left by a previous run come back detached.
    pub fn new(root: PathBuf, logger: Arc<Logger>, sink: Sink) -> Arc<Self> {
        let manager = Arc::new(Self {
            root,
            logger,
            sink,
            next_id: AtomicU64::new(1),
            edits: Mutex::new(HashMap::new()),
            launch_editor: AtomicBool::new(true),
            editors: Mutex::new(Vec::new()),
        });
        manager.restore();
        manager
    }

    fn restore(&self) {
        let path = self.root.join(MANIFEST);
        let Ok(bytes) = std::fs::read(&path) else { return };
        let saved: Vec<SavedEdit> = match serde_json::from_slice(&bytes) {
            Ok(saved) => saved,
            Err(e) => {
                return self
                    .logger
                    .start(None, Channel::Local, "edit-restore", path.to_string_lossy())
                    .fail(e);
            }
        };
        for entry in saved {
            let op = self.logger.start(Some(&entry.host), Channel::Local, "edit-restore", entry.remote_path.clone());
            let content = match std::fs::read(&entry.local_path) {
                Ok(content) => content,
                Err(e) => {
                    op.fail(format!("copia locale {}: {e}", entry.local_path));
                    continue;
                }
            };
            if sha256_hex(&content) == entry.synced_hash {
                remove_copy(&entry.local_path);
                op.ok(Some("nessuna modifica da caricare: copia rimossa".into()));
                continue;
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let edit = Arc::new(Edit {
                session: Mutex::new(None),
                info: Mutex::new(EditInfo {
                    id,
                    session_id: None,
                    connection_id: entry.connection_id,
                    host: entry.host,
                    remote_path: entry.remote_path,
                    local_path: entry.local_path,
                    state: EditState::Detached,
                    uploads: entry.uploads,
                    last_sync_at: entry.last_sync_at,
                    error: Some(DETACHED_MESSAGE.into()),
                }),
                remote: Mutex::new(entry.remote),
                synced_hash: Mutex::new(entry.synced_hash),
                conflict_reply: Mutex::new(None),
                confirm_saves: AtomicBool::new(entry.confirm_saves),
                confirm_reply: Mutex::new(None),
                watcher: Mutex::new(None),
            });
            self.edits.lock().unwrap().insert(id, edit);
            op.ok(Some("modifiche non caricate: in attesa della connessione".into()));
        }
        self.save_manifest();
    }

    /// Records the open files. Written on every change, so it also survives a crash.
    fn save_manifest(&self) {
        let saved: Vec<SavedEdit> = self
            .edits
            .lock()
            .unwrap()
            .values()
            .map(|edit| {
                let info = edit.info.lock().unwrap();
                SavedEdit {
                    connection_id: info.connection_id.clone(),
                    host: info.host.clone(),
                    remote_path: info.remote_path.clone(),
                    local_path: info.local_path.clone(),
                    remote: edit.remote.lock().unwrap().clone(),
                    synced_hash: edit.synced_hash.lock().unwrap().clone(),
                    confirm_saves: edit.confirm_saves.load(Ordering::Relaxed),
                    uploads: info.uploads,
                    last_sync_at: info.last_sync_at,
                }
            })
            .collect();
        let path = self.root.join(MANIFEST);
        let result = (|| -> std::io::Result<()> {
            if saved.is_empty() {
                return match std::fs::remove_file(&path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                };
            }
            std::fs::create_dir_all(&self.root)?;
            restrict_permissions(&self.root);
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&saved).map_err(std::io::Error::other)?)?;
            std::fs::rename(&tmp, &path)
        })();
        if let Err(e) = result {
            self.logger.start(None, Channel::Local, "edit-manifest", path.to_string_lossy()).fail(e);
        }
    }

    pub fn set_launch_editor(&self, launch: bool) {
        self.launch_editor.store(launch, Ordering::Relaxed);
    }

    pub fn set_editors(&self, rules: Vec<crate::settings::EditorRule>) {
        *self.editors.lock().unwrap() = rules;
    }

    /// Opens the local copy with the app of its extension, or the default one.
    fn launch(&self, path: &str) -> Result<()> {
        if !self.launch_editor.load(Ordering::Relaxed) {
            return Ok(());
        }
        let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let app = crate::settings::EditorRule::find(&self.editors.lock().unwrap(), &name).map(|r| r.app.clone());
        let mut command = open_command(path, app.as_deref());
        let mut op = self.logger.start(None, Channel::Local, "open-editor", path);
        op.equivalent(crate::log::shell_line(
            std::iter::once(command.get_program()).chain(command.get_args()),
        ));
        let result = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| AppError::Other(format!("impossibile aprire il file: {e}")));
        op.result(result, |_| Some(app.unwrap_or_else(|| "app predefinita".into())))
    }

    pub fn default_root() -> Option<PathBuf> {
        dirs::cache_dir().map(|d| d.join("glassferry/edit"))
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
        self.save_manifest();
    }

    fn set_state(&self, edit: &Edit, state: EditState, error: Option<String>) {
        // A detached copy stays detached whatever a sync still running reports.
        let (state, error) = if edit.session.lock().unwrap().is_none() {
            (EditState::Detached, Some(DETACHED_MESSAGE.to_string()))
        } else {
            (state, error)
        };
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
        connection_id: &str,
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
                info.connection_id == connection_id && info.remote_path == remote_path
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
        let dir = self.root.join(short_hash(&format!("{connection_id}\0{remote_path}")));
        std::fs::create_dir_all(&dir)?;
        restrict_permissions(&self.root);
        let local_path = dir.join(&file_name);

        let (content, remote) = download(&session, &remote_path).await?;
        tokio::fs::write(&local_path, &content).await?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let edit = Arc::new(Edit {
            session: Mutex::new(Some(session)),
            info: Mutex::new(EditInfo {
                id,
                session_id: Some(session_id),
                connection_id: connection_id.to_string(),
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

        self.watch(&edit)?;
        self.edits.lock().unwrap().insert(id, edit.clone());

        self.launch(&local_path.to_string_lossy())?;
        self.emit(&edit);
        let info = edit.info.lock().unwrap().clone();
        Ok(info)
    }

    /// Watches the copy's directory and syncs after each save; dropping the
    /// watcher ends the loop.
    fn watch(self: &Arc<Self>, edit: &Arc<Edit>) -> Result<()> {
        let local_path = PathBuf::from(&edit.info.lock().unwrap().local_path);
        let (Some(dir), Some(file_name)) = (local_path.parent(), local_path.file_name()) else {
            return Err(AppError::Other("percorso non valido".into()));
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let watched_name = file_name.to_os_string();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event
                && event.paths.iter().any(|p| p.file_name() == Some(watched_name.as_os_str()))
            {
                let _ = tx.send(());
            }
        })
        .map_err(|e| AppError::Other(format!("watcher: {e}")))?;
        notify::Watcher::watch(&mut watcher, dir, notify::RecursiveMode::NonRecursive)
            .map_err(|e| AppError::Other(format!("watcher: {e}")))?;
        *edit.watcher.lock().unwrap() = Some(watcher);
        let manager = self.clone();
        let watched = edit.clone();
        tauri::async_runtime::spawn(async move { manager.watch_loop(watched, rx).await });
        Ok(())
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
        let Some(session) = edit.session.lock().unwrap().clone() else { return };
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
        let mut current = match remote_state(&session, &remote_path).await {
            Ok(state) => state,
            Err(e) => return self.set_state(edit, EditState::Error, Some(e.to_string())),
        };
        let mut changed = current.size != opened.size || current.modified != opened.modified;
        // Same size and second: only the content tells.
        if let (false, false, Some(opened_hash)) = (changed, force, &opened.sha256) {
            match remote_hash(&session, &remote_path).await {
                Ok(hash) => {
                    changed = hash != *opened_hash;
                    current.sha256 = Some(hash);
                }
                Err(e) => return self.set_state(edit, EditState::Error, Some(e.to_string())),
            }
        }
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
        match upload(&session, &remote_path, &content).await {
            Ok(mut state) => {
                state.sha256 = Some(hash.clone());
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
        let (local, remote) = (crate::log::sftp_quote(&info.local_path), crate::log::sftp_quote(&info.remote_path));
        let part = crate::log::sftp_quote(&crate::transfer::part_name(&info.remote_path));
        let commands = vec![
            format!("sftp> put {local} {part}   # stessi permessi del file"),
            format!("sftp> rename {part} {remote}   # posix-rename: sostituisce in un passo"),
            format!(
                "# se è un link, se proprietario o gruppo cambierebbero o manca posix-rename: sftp> put {local} {remote}   # scrittura diretta"
            ),
        ];
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
        let Some(session) = edit.session.lock().unwrap().clone() else { return };
        let (remote_path, local_path) = {
            let info = edit.info.lock().unwrap();
            (info.remote_path.clone(), PathBuf::from(&info.local_path))
        };
        let backup = backup_path(&local_path);
        if let Err(e) = tokio::fs::write(&backup, local_content).await {
            return self.set_state(edit, EditState::Error, Some(format!("backup: {e}")));
        }
        match download(&session, &remote_path).await {
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

    /// The server's current version against the local copy, for the conflict dialog.
    pub async fn conflict_diff(&self, id: u64) -> Result<crate::diff::ConflictDiff> {
        let edit = self.get(id)?;
        let Some(session) = edit.session.lock().unwrap().clone() else {
            return Err(AppError::Other("connessione chiusa: riconnetti per vedere le differenze".into()));
        };
        let (remote_path, local_path) = {
            let info = edit.info.lock().unwrap();
            (info.remote_path.clone(), info.local_path.clone())
        };
        let mine = tokio::fs::read(&local_path).await?;
        let mut op = session.launcher.log(Channel::Sftp, "edit-diff", &remote_path);
        op.equivalent(session.launcher.sftp_equivalent("get", &[&remote_path]));
        let size = session.sftp.fs().metadata(&remote_path).await.ok().and_then(|m| m.len()).unwrap_or(0);
        if size as usize > crate::diff::MAX_DIFF_BYTES {
            op.ok(Some("troppo grande per il confronto".into()));
            return Ok(crate::diff::ConflictDiff { too_large: true, ..Default::default() });
        }
        let server = op.result(session.sftp.fs().read(&remote_path).await.map_err(AppError::from), |b| {
            Some(crate::transfer::format_bytes(b.len() as u64))
        })?;
        tokio::task::spawn_blocking(move || crate::diff::diff(&server, &mine))
            .await
            .map_err(|e| AppError::Other(e.to_string()))
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
        let info = edit.info.lock().unwrap().clone();
        remove_copy(&info.local_path);
        self.logger.start(Some(&info.host), Channel::Local, "edit-close", info.remote_path).ok(None);
        self.save_manifest();
    }

    /// The copy differs from what the server had at the last sync, including
    /// a save the debounce has not handled yet.
    fn has_local_changes(edit: &Edit) -> bool {
        let local_path = edit.info.lock().unwrap().local_path.clone();
        match std::fs::read(&local_path) {
            Ok(content) => sha256_hex(&content) != *edit.synced_hash.lock().unwrap(),
            Err(_) => false,
        }
    }

    /// The session is closing: copies without local changes are closed, the
    /// others are detached until the connection opens again.
    pub fn close_session(&self, session_id: u64) {
        let edits: Vec<Arc<Edit>> = self
            .edits
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.info.lock().unwrap().session_id == Some(session_id))
            .cloned()
            .collect();
        for edit in edits {
            if Self::has_local_changes(&edit) {
                self.detach(&edit);
            } else {
                let id = edit.info.lock().unwrap().id;
                self.close(id);
            }
        }
    }

    fn detach(&self, edit: &Edit) {
        edit.session.lock().unwrap().take();
        edit.watcher.lock().unwrap().take();
        // Pending prompts get "not now": reattaching asks again.
        if let Some(tx) = edit.conflict_reply.lock().unwrap().take() {
            let _ = tx.send(ConflictChoice::Keep);
        }
        if let Some(tx) = edit.confirm_reply.lock().unwrap().take() {
            let _ = tx.send(false);
        }
        let info = {
            let mut info = edit.info.lock().unwrap();
            info.session_id = None;
            info.clone()
        };
        self.logger
            .start(Some(&info.host), Channel::Local, "edit-detach", info.remote_path)
            .ok(Some("modifiche non caricate: copia locale conservata".into()));
        self.set_state(edit, EditState::Detached, None);
    }

    /// A connection opened: its detached copies use the new session and their
    /// changes are uploaded (conflict check and confirmation as for a save).
    pub fn attach(self: &Arc<Self>, connection_id: &str, session_id: u64, session: Arc<Session>) {
        let detached: Vec<Arc<Edit>> = self
            .edits
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.session.lock().unwrap().is_none() && e.info.lock().unwrap().connection_id == connection_id)
            .cloned()
            .collect();
        for edit in detached {
            *edit.session.lock().unwrap() = Some(session.clone());
            let info = {
                let mut info = edit.info.lock().unwrap();
                info.session_id = Some(session_id);
                info.host = session.launcher.label.clone();
                info.clone()
            };
            let op = self.logger.start(Some(&info.host), Channel::Local, "edit-attach", info.remote_path);
            if let Err(e) = self.watch(&edit) {
                op.fail(&e);
                self.set_state(&edit, EditState::Error, Some(e.to_string()));
                continue;
            }
            op.ok(None);
            self.set_state(&edit, EditState::Synced, None);
            let manager = self.clone();
            tauri::async_runtime::spawn(async move { manager.sync(&edit).await });
        }
    }

    /// On exit: closes the copies without local changes. The others stay in
    /// the manifest and come back detached at the next start.
    pub fn close_unchanged(&self) {
        let edits: Vec<Arc<Edit>> = self.edits.lock().unwrap().values().cloned().collect();
        for edit in edits {
            if !Self::has_local_changes(&edit) {
                let id = edit.info.lock().unwrap().id;
                self.close(id);
            }
        }
    }
}

// ---- remote I/O ----------------------------------------------------------------

/// Size and mtime; the hash is left to `remote_hash`, only needed when they match.
async fn remote_state(session: &Session, path: &str) -> Result<RemoteState> {
    let meta = session.sftp.fs().metadata(path).await?;
    Ok(RemoteState {
        size: meta.len().unwrap_or(0),
        modified: meta.modified().map(|t| t.as_duration().as_secs()),
        sha256: None,
    })
}

/// Content hash of the remote file: `sha256sum` on the server when the probe
/// found it, otherwise the file is read over SFTP (at most `MAX_EDIT_BYTES`).
async fn remote_hash(session: &Session, path: &str) -> Result<String> {
    if let Some(hash) = remote_sha256(session, path).await {
        return Ok(hash);
    }
    let mut op = session.launcher.log(Channel::Sftp, "edit-check", path);
    op.equivalent(session.launcher.sftp_equivalent("get", &[path]));
    let result = async { Ok::<_, AppError>(sha256_hex(&session.sftp.fs().read(path).await?)) }.await;
    op.result(result, |_| Some("confronto del contenuto letto via SFTP (sha256sum non disponibile)".into()))
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
        // When the server can hash, it must agree with what we read; otherwise
        // the file changed while we were reading: keep the size/mtime check.
        let read = sha256_hex(&content);
        let sha256 = match remote_sha256(session, path).await {
            Some(hash) => (hash == read).then_some(read),
            None => Some(read),
        };
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
    let part = crate::transfer::part_name(path);
    let result = async {
        let in_place = write_replacing(session, path, &part, content).await?;
        if in_place.is_some() {
            let mut file = session.sftp.options().write(true).create(true).truncate(true).open(path).await?;
            file.write_all(content).await?;
            file.close().await?;
        }
        Ok((remote_state(session, path).await?, in_place))
    }
    .await;
    let size = crate::transfer::format_bytes(content.len() as u64);
    match &result {
        Ok((_, Some(_))) | Err(_) => op.equivalent(session.launcher.sftp_equivalent("put", &["<copia locale>", path])),
        Ok((_, None)) => op.equivalent(session.launcher.sftp_equivalent("put", &["<copia locale>", &part])),
    };
    op.result(result, |(_, in_place)| {
        Some(match in_place {
            None => format!("{size}, sostituito con posix-rename"),
            Some(reason) => format!("{size}, scrittura diretta: {reason}"),
        })
    })
    .map(|(state, _)| state)
}

/// Writes `part` with the file's mode and renames it over `path` in one step.
/// `Ok(Some(reason))`: replacing would lose something, the server is
/// unchanged and the caller writes in place. `Err`: the original is intact,
/// the part file removed.
async fn write_replacing(session: &Session, path: &str, part: &str, content: &[u8]) -> Result<Option<String>> {
    if !session.sftp.support_posix_rename() {
        return Ok(Some("il server non ha posix-rename".into()));
    }
    let mut fs = session.sftp.fs();
    let original = fs.symlink_metadata(path).await?;
    if original.file_type().is_some_and(|t| t.is_symlink()) {
        return Ok(Some("è un link simbolico".into()));
    }
    let Ok(mut file) = session.sftp.options().write(true).create(true).truncate(true).open(part).await else {
        return Ok(Some("impossibile creare il file temporaneo nella cartella".into()));
    };
    let result: Result<Option<String>> = async {
        // Mode first: the content is never readable with wider permissions.
        if let Some(permissions) = original.permissions() {
            fs.set_metadata(part, MetaDataBuilder::new().permissions(permissions).create()).await?;
        }
        let created = fs.metadata(part).await?;
        if created.uid() != original.uid() || created.gid() != original.gid() {
            file.close().await?;
            return Ok(Some("proprietario o gruppo cambierebbero".into()));
        }
        file.write_all(content).await?;
        file.close().await?;
        fs.rename(part, path).await?;
        Ok(None)
    }
    .await;
    if !matches!(result, Ok(None)) {
        let _ = fs.remove_file(part).await;
    }
    result
}

// ---- helpers -------------------------------------------------------------------

/// macOS: `open [-a app] path`. Linux: `xdg-open path`, or the configured
/// command with the path as its last argument (`sh -c`, so it can carry
/// options, e.g. `code --new-window`).
fn open_command(path: &str, app: Option<&str>) -> std::process::Command {
    match app {
        Some(app) if cfg!(target_os = "macos") => {
            let mut command = std::process::Command::new("open");
            command.args(["-a", app, path]);
            command
        }
        Some(app) => {
            let mut command = std::process::Command::new("/bin/sh");
            command.args(["-c", &format!(r#"{app} "$1""#), "sh", path]);
            command
        }
        None => {
            let mut command = std::process::Command::new(if cfg!(target_os = "macos") { "open" } else { "xdg-open" });
            command.arg(path);
            command
        }
    }
}

/// Removes a copy with its private directory.
fn remove_copy(local_path: &str) {
    if let Some(dir) = Path::new(local_path).parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
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
