pub mod askpass;
pub mod entry;
pub mod error;
pub mod local;
pub mod log;
pub mod runtime_dir;
pub mod session;
pub mod ssh_config;

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
use ssh_config::HostEntry;

const DEFAULT_SSH_BIN: &str = "/usr/bin/ssh";

struct AppState {
    ssh_bin: PathBuf,
    runtime_dir: PathBuf,
    askpass: Arc<AskpassServer>,
    logger: Arc<Logger>,
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
            log_open_dir
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            let state = handle.state::<AppState>();
            for session in state.sessions.lock().unwrap().values() {
                session.launcher.stop_master_blocking();
            }
            state.askpass.remove_socket();
        }
    });
}
