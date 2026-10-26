pub mod askpass;
pub mod edit;
pub mod entry;
pub mod error;
pub mod fileops;
pub mod local;
pub mod log;
pub mod runtime_dir;
pub mod session;
pub mod ssh_config;
pub mod transfer;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use askpass::AskpassServer;
use error::{AppError, Result};
use entry::FileEntry;
use log::{LogEntry, Logger};
use session::{Capabilities, Session, SshLauncher};
use edit::{ConflictChoice, EditEvent, EditInfo, EditManager};
use fileops::{DeleteMethod, Target};
use ssh_config::HostEntry;
use transfer::{ConflictAction, Direction, JobInfo, TransferEvent, TransferManager};

const DEFAULT_SSH_BIN: &str = "/usr/bin/ssh";

struct AppState {
    ssh_bin: PathBuf,
    runtime_dir: PathBuf,
    askpass: Arc<AskpassServer>,
    logger: Arc<Logger>,
    transfers: Arc<TransferManager>,
    edits: Arc<EditManager>,
    next_session_id: AtomicU64,
    sessions: Mutex<HashMap<u64, Arc<Session>>>,
}

impl AppState {
    fn session(&self, id: u64) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or(AppError::SessionNotFound(id))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectResult {
    session_id: u64,
    home: String,
    capabilities: Capabilities,
}

#[tauri::command]
fn list_hosts() -> Result<Vec<HostEntry>> {
    ssh_config::list_hosts()
}

#[tauri::command]
async fn resolve_host(state: State<'_, AppState>, alias: String) -> Result<BTreeMap<String, String>> {
    ssh_config::resolve_host(&state.ssh_bin, &alias).await
}

#[tauri::command]
async fn connect(state: State<'_, AppState>, alias: String, verbose: bool) -> Result<ConnectResult> {
    let id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let launcher = SshLauncher {
        ssh_bin: state.ssh_bin.clone(),
        control_path: state.runtime_dir.join(format!("{pid}-{id}.sock")),
        log_path: state.runtime_dir.join(format!("{pid}-{id}.log")),
        env: state.askpass.env_for(&alias),
        verbose,
        logger: state.logger.clone(),
        alias,
    };
    let session = Session::connect(launcher).await?;
    let result = ConnectResult {
        session_id: id,
        home: session.home.to_string_lossy().into_owned(),
        capabilities: session.capabilities.clone(),
    };
    state.sessions.lock().unwrap().insert(id, Arc::new(session));
    Ok(result)
}

#[tauri::command]
async fn list_dir(state: State<'_, AppState>, session_id: u64, path: String) -> Result<Vec<FileEntry>> {
    state.session(session_id)?.list_dir(path.as_ref()).await
}

#[tauri::command]
fn local_home() -> Result<String> {
    Ok(local::home()?.to_string_lossy().into_owned())
}

#[tauri::command]
async fn local_list_dir(state: State<'_, AppState>, path: String) -> Result<Vec<FileEntry>> {
    local::list_dir(&state.logger, PathBuf::from(path)).await
}

#[tauri::command]
async fn disconnect(state: State<'_, AppState>, session_id: u64) -> Result<()> {
    state.transfers.cancel_session(session_id);
    state.edits.close_session(session_id);
    let session = state.sessions.lock().unwrap().remove(&session_id);
    if let Some(session) = session {
        match Arc::try_unwrap(session) {
            Ok(session) => session.close().await,
            // Still in use by a running command: just stop the master.
            Err(shared) => shared.launcher.stop_master().await,
        }
    }
    Ok(())
}

#[tauri::command]
fn askpass_respond(state: State<'_, AppState>, id: u64, value: Option<String>) {
    state.askpass.respond(id, value);
}

#[tauri::command]
fn log_snapshot(state: State<'_, AppState>) -> Vec<LogEntry> {
    state.logger.snapshot()
}

#[tauri::command]
fn log_clear(state: State<'_, AppState>) {
    state.logger.clear();
}

#[tauri::command]
fn log_set_capacity(state: State<'_, AppState>, capacity: usize) {
    state.logger.set_capacity(capacity);
}

#[tauri::command]
fn log_open_dir(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = state
        .logger
        .file_dir()
        .ok_or_else(|| AppError::Other("log su file non disponibile".into()))?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| AppError::Other(e.to_string()))
}

#[tauri::command]
fn transfer_start(
    state: State<'_, AppState>,
    session_id: u64,
    direction: Direction,
    src_dir: String,
    names: Vec<String>,
    dst_dir: String,
) -> Result<u64> {
    let session = state.session(session_id)?;
    Ok(state
        .transfers
        .start(session_id, session, direction, src_dir, names, dst_dir))
}

#[tauri::command]
fn transfer_cancel(state: State<'_, AppState>, job_id: u64) {
    state.transfers.cancel(job_id);
}

#[tauri::command]
fn transfer_list(state: State<'_, AppState>) -> Vec<JobInfo> {
    state.transfers.list()
}

#[tauri::command]
fn transfer_clear_finished(state: State<'_, AppState>) {
    state.transfers.clear_finished();
}

#[tauri::command]
fn transfer_conflict_respond(state: State<'_, AppState>, id: u64, action: ConflictAction, apply_to_all: bool) {
    state.transfers.respond_conflict(id, action, apply_to_all);
}

#[tauri::command]
async fn fs_mkdir(state: State<'_, AppState>, target: Target, dir: String, name: String) -> Result<()> {
    match target {
        Target::Local => fileops::local::mkdir(&state.logger, &dir, &name).await,
        Target::Remote { session_id } => fileops::remote::mkdir(&*state.session(session_id)?, &dir, &name).await,
    }
}

#[tauri::command]
async fn fs_rename(state: State<'_, AppState>, target: Target, dir: String, from: String, to: String) -> Result<()> {
    match target {
        Target::Local => fileops::local::rename(&state.logger, &dir, &from, &to).await,
        Target::Remote { session_id } => {
            fileops::remote::rename(&*state.session(session_id)?, &dir, &from, &to).await
        }
    }
}

#[tauri::command]
async fn fs_move(
    state: State<'_, AppState>,
    target: Target,
    src_dir: String,
    names: Vec<String>,
    dst_dir: String,
) -> Result<()> {
    match target {
        Target::Local => fileops::local::move_to(&state.logger, &src_dir, &names, &dst_dir).await,
        Target::Remote { session_id } => {
            fileops::remote::move_to(&*state.session(session_id)?, &src_dir, &names, &dst_dir).await
        }
    }
}

#[tauri::command]
async fn fs_delete(state: State<'_, AppState>, target: Target, dir: String, names: Vec<String>) -> Result<DeleteMethod> {
    match target {
        Target::Local => fileops::local::delete(&state.logger, &dir, &names).await,
        Target::Remote { session_id } => fileops::remote::delete(&*state.session(session_id)?, &dir, &names).await,
    }
}

#[tauri::command]
async fn edit_open(state: State<'_, AppState>, session_id: u64, path: String) -> Result<EditInfo> {
    let session = state.session(session_id)?;
    state.edits.open(session_id, session, path).await
}

#[tauri::command]
fn edit_list(state: State<'_, AppState>) -> Vec<EditInfo> {
    state.edits.list()
}

#[tauri::command]
fn edit_close(state: State<'_, AppState>, id: u64) {
    state.edits.close(id);
}

#[tauri::command]
fn edit_resolve(state: State<'_, AppState>, id: u64, choice: ConflictChoice) -> Result<()> {
    state.edits.resolve(id, choice)
}

#[tauri::command]
fn edit_reopen(state: State<'_, AppState>, id: u64) -> Result<()> {
    state.edits.reopen(id)
}

#[tauri::command]
fn edit_reveal(app: tauri::AppHandle, state: State<'_, AppState>, id: u64) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(state.edits.local_path(id)?)
        .map_err(|e| AppError::Other(e.to_string()))
}

/// Opens a local file with its default application (double click).
#[tauri::command]
fn local_open(app: tauri::AppHandle, state: State<'_, AppState>, path: String) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let op = state.logger.start(None, log::Channel::Local, "open", &path);
    op.result(
        app.opener()
            .open_path(&path, None::<&str>)
            .map_err(|e| AppError::Other(e.to_string())),
        |_| None,
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Some(code) = askpass::run_helper_if_requested() {
        std::process::exit(code);
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let runtime_dir = runtime_dir::create()?;
            let handle = app.handle().clone();
            let logger = Logger::new(
                Some(Arc::new(move |batch| {
                    let _ = handle.emit("log-batch", batch);
                })),
                Logger::default_dir(),
            );
            let handle = app.handle().clone();
            let transfers = TransferManager::new(
                logger.clone(),
                Arc::new(move |event| {
                    let _ = match event {
                        TransferEvent::Updates(jobs) => handle.emit("transfer-update", jobs),
                        TransferEvent::Conflict(request) => handle.emit("transfer-conflict", request),
                    };
                }),
            );
            let handle = app.handle().clone();
            let edit_root = EditManager::default_root().unwrap_or_else(|| runtime_dir.join("edit"));
            let edits = EditManager::new(
                edit_root,
                Arc::new(move |event| {
                    let _ = match event {
                        EditEvent::Update(info) => handle.emit("edit-update", info),
                        EditEvent::Conflict(conflict) => handle.emit("edit-conflict", conflict),
                    };
                }),
            );
            let handle = app.handle().clone();
            let askpass = AskpassServer::start(
                &runtime_dir,
                Arc::new(move |request| {
                    let _ = handle.emit("askpass-request", request);
                }),
                logger.clone(),
            )?;
            app.manage(AppState {
                ssh_bin: PathBuf::from(DEFAULT_SSH_BIN),
                runtime_dir,
                askpass,
                logger,
                transfers,
                edits,
                next_session_id: AtomicU64::new(1),
                sessions: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_hosts,
            resolve_host,
            connect,
            list_dir,
            local_home,
            local_list_dir,
            disconnect,
            askpass_respond,
            log_snapshot,
            log_clear,
            log_set_capacity,
            log_open_dir,
            transfer_start,
            transfer_cancel,
            transfer_list,
            transfer_clear_finished,
            transfer_conflict_respond,
            fs_mkdir,
            fs_rename,
            fs_move,
            fs_delete,
            edit_open,
            edit_list,
            edit_close,
            edit_resolve,
            edit_reopen,
            edit_reveal,
            local_open
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            let state = handle.state::<AppState>();
            state.edits.close_all_synced();
            for session in state.sessions.lock().unwrap().values() {
                session.launcher.stop_master_blocking();
            }
            state.askpass.remove_socket();
        }
    });
}
