pub mod askpass;
pub mod attrs;
pub mod dirsize;
pub mod connections;
pub mod edit;
pub mod entry;
pub mod error;
pub mod fileops;
pub mod ids;
pub mod local;
pub mod log;
pub mod plan;
pub mod runtime_dir;
pub mod session;
pub mod settings;
pub mod terminal;
pub mod ssh_config;
pub mod transfer;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use askpass::{AskpassServer, AttemptOptions};
use connections::{keychain, ConnectionStore, Source, Store};
use error::{AppError, Result};
use entry::FileEntry;
use log::{LogEntry, Logger};
use session::{Capabilities, Session, SshLauncher};
use edit::{ConflictChoice, EditEvent, EditInfo, EditManager};
use fileops::{DeleteMethod, Target};
use plan::{PlanOutcome, PlanPreview, PlanRequest, Planner};
use ssh_config::HostEntry;
use transfer::{ConflictAction, Direction, JobInfo, TransferEvent, TransferManager};

struct AppState {
    settings_store: settings::SettingsStore,
    settings: Mutex<settings::Settings>,
    runtime_dir: PathBuf,
    askpass: Arc<AskpassServer>,
    logger: Arc<Logger>,
    transfers: Arc<TransferManager>,
    edits: Arc<EditManager>,
    planner: Arc<Planner>,
    connection_store: ConnectionStore,
    connections: Mutex<Store>,
    /// Problem met loading `connections.json`, shown once by the UI.
    connections_warning: Mutex<Option<String>>,
    next_session_id: AtomicU64,
    sessions: Mutex<HashMap<u64, OpenSession>>,
}

struct OpenSession {
    connection_id: String,
    session: Arc<Session>,
}

impl AppState {
    /// Read at each use: a change applies to the next connection.
    fn ssh_bin(&self) -> PathBuf {
        self.settings.lock().unwrap().ssh_bin()
    }

    fn session(&self, id: u64) -> Result<Arc<Session>> {
        self.open_session(id).map(|(_, session)| session)
    }

    fn endpoint(&self, id: u64) -> Result<transfer::Endpoint> {
        let (connection_id, session) = self.open_session(id)?;
        Ok(transfer::Endpoint {
            session_id: id,
            connection_id,
            session,
        })
    }

    /// The session and the id of the connection it was opened for.
    fn open_session(&self, id: u64) -> Result<(String, Arc<Session>)> {
        self.sessions
            .lock()
            .unwrap()
            .get(&id)
            .map(|open| (open.connection_id.clone(), open.session.clone()))
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionsLoad {
    store: Store,
    warning: Option<String>,
}

#[tauri::command]
fn settings_load(state: State<'_, AppState>) -> settings::Settings {
    state.settings.lock().unwrap().clone()
}

/// Validates, saves and applies the settings (to new connections).
#[tauri::command]
fn settings_save(state: State<'_, AppState>, settings: settings::Settings) -> Result<settings::Settings> {
    let settings = settings.normalized()?;
    let op = state.logger.start(None, log::Channel::Local, "settings-save", state.settings_store.path().to_string_lossy());
    op.result(state.settings_store.save(&settings), |_| Some(format!("ssh: {}", settings.ssh_bin().display())))?;
    *state.settings.lock().unwrap() = settings.clone();
    Ok(settings)
}

/// Opens the system terminal in `path`: a local shell, or a new ssh
/// connection to the pane's host that changes to `path`.
#[tauri::command]
async fn open_terminal(state: State<'_, AppState>, target: Target, path: String) -> Result<()> {
    let settings = state.settings.lock().unwrap().terminal.clone();
    let (host, script, equivalent) = match target {
        Target::Local => (None, terminal::local_script(&path), format!("cd {}", log::shell_line([&path]))),
        Target::Remote { session_id } => {
            let session = state.session(session_id)?;
            if !session.capabilities.shell {
                return Err(AppError::Other("account solo SFTP: il server non concede una shell".into()));
            }
            let launcher = &session.launcher;
            let line = terminal::ssh_line(&launcher.ssh_bin, launcher.config_file.as_deref(), &launcher.alias, &path);
            (Some(launcher.label.clone()), terminal::remote_script(&line), line)
        }
    };
    let mut op = state.logger.start(host.as_deref(), log::Channel::Local, "terminal", &path);
    op.equivalent(equivalent);
    op.result(terminal::open(&settings, &state.runtime_dir, &script).await, |how| Some(how.clone()))?;
    Ok(())
}

/// Terminals the app knows that are installed on this machine.
#[tauri::command]
fn terminal_list() -> Vec<terminal::TerminalApp> {
    terminal::installed()
}

/// `ssh -V` of `path` (the configured binary when `None`).
#[tauri::command]
async fn ssh_check(state: State<'_, AppState>, path: Option<String>) -> Result<settings::SshCheck> {
    let path = match path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => state.ssh_bin(),
    };
    let mut op = state.logger.start(None, log::Channel::Local, "ssh-version", path.to_string_lossy());
    op.equivalent(format!("{} -V", log::shell_line([&path])));
    op.result(settings::check_ssh(&path).await, |check| Some(check.version.clone()))
}

#[tauri::command]
fn connections_load(state: State<'_, AppState>) -> ConnectionsLoad {
    ConnectionsLoad {
        store: state.connections.lock().unwrap().clone(),
        warning: state.connections_warning.lock().unwrap().take(),
    }
}

/// Saves the whole store and regenerates the ssh_config. Saved passwords of
/// removed connections (or with saving turned off) are deleted.
#[tauri::command]
fn connections_save(state: State<'_, AppState>, store: Store) -> Result<Store> {
    let op = state.logger.start(None, log::Channel::Local, "connections-save", state.connection_store.ssh_config_path().to_string_lossy());
    op.result(state.connection_store.save(&store), |_| Some(format!("{} connessioni", store.connections.len())))?;
    let previous = std::mem::replace(&mut *state.connections.lock().unwrap(), store.clone());
    for old in previous.connections.iter().filter(|c| c.save_password) {
        if !store.connections.iter().any(|c| c.id == old.id && c.save_password) {
            forget_password(&state, &old.id, &old.name);
        }
    }
    Ok(store)
}

fn forget_password(state: &AppState, id: &str, label: &str) {
    let op = state.logger.start(Some(label), log::Channel::Local, "keychain", "rimozione password salvata");
    let _ = op.result(keychain::delete(id), |_| None);
}

#[tauri::command]
fn connection_forget_password(state: State<'_, AppState>, id: String) -> Result<()> {
    let name = state.connections.lock().unwrap().connection(&id)?.name.clone();
    forget_password(&state, &id, &name);
    Ok(())
}

/// Effective settings (`ssh -G`) of a source, saved or not.
#[tauri::command]
async fn connection_check(state: State<'_, AppState>, source: Source) -> Result<BTreeMap<String, String>> {
    connections::check_source(&state.ssh_bin(), &state.runtime_dir, &source).await
}

#[tauri::command]
fn connections_export(state: State<'_, AppState>, path: String) -> Result<()> {
    let store = state.connections.lock().unwrap().clone();
    let op = state.logger.start(None, log::Channel::Local, "connections-export", &path);
    let json = serde_json::to_vec_pretty(&store).map_err(|e| AppError::Other(e.to_string()))?;
    op.result(std::fs::write(&path, json).map_err(AppError::from), |_| Some(format!("{} connessioni", store.connections.len())))
}

/// Reads an exported file; the UI merges it and saves.
#[tauri::command]
fn connections_import(state: State<'_, AppState>, path: String) -> Result<Store> {
    let op = state.logger.start(None, log::Channel::Local, "connections-import", &path);
    let result = std::fs::read(&path).map_err(AppError::from).and_then(|bytes| {
        let store = connections::parse_store(&bytes)?;
        store.validate()?;
        Ok(store)
    });
    op.result(result, |store| Some(format!("{} connessioni", store.connections.len())))
}

#[tauri::command]
async fn connect(state: State<'_, AppState>, connection_id: String, verbose: bool) -> Result<ConnectResult> {
    let connection = state.connections.lock().unwrap().connection(&connection_id)?.clone();
    let target = state.connection_store.target(&connection);
    let saved_password = if connection.save_password {
        let op = state.logger.start(Some(&connection.name), log::Channel::Local, "keychain", "lettura password salvata");
        let id = connection.id.clone();
        let result = tauri::async_runtime::spawn_blocking(move || keychain::get(&id))
            .await
            .map_err(|e| AppError::Other(e.to_string()))
            .and_then(|r| r);
        op.result(result, |p| Some(if p.is_some() { "trovata".into() } else { "nessuna".into() })).ok().flatten()
    } else {
        None
    };
    let attempt = state.askpass.register(AttemptOptions {
        saved_password,
        save_enabled: connection.save_password,
    });

    let id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let launcher = SshLauncher {
        ssh_bin: state.ssh_bin(),
        control_path: state.runtime_dir.join(format!("{pid}-{id}.sock")),
        log_path: state.runtime_dir.join(format!("{pid}-{id}.log")),
        env: state.askpass.env_for(&connection.name, attempt),
        verbose,
        logger: state.logger.clone(),
        alias: target.alias,
        config_file: target.config_file,
        label: connection.name.clone(),
    };
    let session = Session::connect(launcher).await;
    let to_save = state.askpass.finish(attempt);
    let session = session?;
    if let Some(password) = to_save {
        let op = state.logger.start(Some(&connection.name), log::Channel::Local, "keychain", "salvataggio password");
        let id = connection.id.clone();
        let result = tauri::async_runtime::spawn_blocking(move || keychain::set(&id, &password))
            .await
            .map_err(|e| AppError::Other(e.to_string()))
            .and_then(|r| r);
        let _ = op.result(result, |_| None);
    }
    let result = ConnectResult {
        session_id: id,
        home: session.home.to_string_lossy().into_owned(),
        capabilities: session.capabilities.clone(),
    };
    let session = Arc::new(session);
    state.sessions.lock().unwrap().insert(
        id,
        OpenSession {
            connection_id: connection_id.clone(),
            session: session.clone(),
        },
    );
    // Copies with changes left from an earlier session of this connection.
    state.edits.attach(&connection_id, id, session);
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
    let session = state.sessions.lock().unwrap().remove(&session_id).map(|open| open.session);
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
fn askpass_respond(state: State<'_, AppState>, id: u64, value: Option<String>, save: bool) {
    state.askpass.respond(id, value, save);
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

/// `dst_session_id`: destination of a remote → remote copy (`direction: "remote"`).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn transfer_start(
    state: State<'_, AppState>,
    session_id: u64,
    direction: Direction,
    src_dir: String,
    names: Vec<String>,
    dst_dir: String,
    dst_session_id: Option<u64>,
) -> Result<u64> {
    let src = state.endpoint(session_id)?;
    let dst = match (direction, dst_session_id) {
        (Direction::Remote, Some(id)) => {
            if id == session_id {
                transfer::check_same_session_copy(&src_dir, &names, &dst_dir)?;
            }
            Some(state.endpoint(id)?)
        }
        (Direction::Remote, None) => return Err(AppError::Other("manca la sessione di destinazione".into())),
        _ => None,
    };
    Ok(state.transfers.start(src, direction, src_dir, names, dst_dir, dst))
}

/// Resumes an interrupted job on open sessions of the same connections.
#[tauri::command]
fn transfer_resume(state: State<'_, AppState>, job_id: u64, session_id: u64, dst_session_id: Option<u64>) -> Result<u64> {
    let src = state.endpoint(session_id)?;
    let dst = dst_session_id.map(|id| state.endpoint(id)).transpose()?;
    state.transfers.resume(job_id, src, dst)
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

/// chmod / chown / chgrp of a selection. `dry_run`: only the commands that
/// would run (preview); otherwise they run and are returned.
#[tauri::command]
async fn fs_attrs(
    state: State<'_, AppState>,
    target: Target,
    dir: String,
    names: Vec<String>,
    change: attrs::AttrChange,
    dry_run: bool,
) -> Result<Vec<String>> {
    match (target, dry_run) {
        (Target::Local, true) => attrs::local::commands(&dir, &names, &change),
        (Target::Local, false) => attrs::local::apply(&state.logger, &dir, &names, &change).await,
        (Target::Remote { session_id }, true) => attrs::remote::commands(&*state.session(session_id)?, &dir, &names, &change),
        (Target::Remote { session_id }, false) => attrs::remote::apply(&*state.session(session_id)?, &dir, &names, &change).await,
    }
}

/// Sizes of the folders `names` in `dir` (du on the server, or a walk).
#[tauri::command]
async fn fs_dir_size(state: State<'_, AppState>, target: Target, dir: String, names: Vec<String>) -> Result<Vec<dirsize::DirSize>> {
    match target {
        Target::Local => dirsize::local::sizes(&state.logger, &dir, &names).await,
        Target::Remote { session_id } => dirsize::remote::sizes(&*state.session(session_id)?, &dir, &names).await,
    }
}

/// Prepares a delete, move or rename and returns its preview.
#[tauri::command]
async fn fs_plan(state: State<'_, AppState>, target: Target, request: PlanRequest) -> Result<PlanPreview> {
    let session = match target {
        Target::Local => None,
        Target::Remote { session_id } => Some(state.session(session_id)?),
    };
    state.planner.plan(&state.logger, target, session.as_deref(), request).await
}

/// Runs exactly the previewed plan (single use).
#[tauri::command]
async fn fs_plan_execute(state: State<'_, AppState>, id: u64) -> Result<PlanOutcome> {
    let session = match state.planner.target(id)? {
        Target::Local => None,
        Target::Remote { session_id } => Some(state.session(session_id)?),
    };
    state.planner.execute(&state.logger, id, session.as_deref()).await
}

#[tauri::command]
fn fs_plan_discard(state: State<'_, AppState>, id: u64) {
    state.planner.discard(id);
}

#[tauri::command]
async fn edit_open(state: State<'_, AppState>, session_id: u64, path: String, confirm_saves: bool) -> Result<EditInfo> {
    let (connection_id, session) = state.open_session(session_id)?;
    state.edits.open(&connection_id, session_id, session, path, confirm_saves).await
}

#[tauri::command]
fn edit_confirm_save(state: State<'_, AppState>, id: u64, allow: bool) -> Result<()> {
    state.edits.confirm_save(id, allow)
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

/// Loads `connections.json` and regenerates the ssh_config. An unreadable
/// file is set aside (never overwritten) and the app starts with defaults.
fn load_connections(store: &ConnectionStore, logger: &Arc<Logger>) -> (Store, Option<String>) {
    let op = logger.start(None, log::Channel::Local, "connections-load", store.ssh_config_path().with_file_name("connections.json").to_string_lossy());
    match store.load() {
        Ok(loaded) => {
            op.ok(Some(format!("{} connessioni", loaded.connections.len())));
            if let Err(e) = store.write_ssh_config(&loaded) {
                logger.start(None, log::Channel::Local, "ssh-config", store.ssh_config_path().to_string_lossy()).fail(&e);
            }
            (loaded, None)
        }
        Err(e) => {
            op.fail(&e);
            let warning = match store.set_aside() {
                Ok(path) => format!("{e}. Il file è stato rinominato in {} e l'elenco riparte vuoto.", path.display()),
                Err(_) => e.to_string(),
            };
            (Store::default(), Some(warning))
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Some(code) = askpass::run_helper_if_requested() {
        std::process::exit(code);
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
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
                ConnectionStore::default_dir().map(|d| d.join("transfers.json")),
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
                logger.clone(),
                Arc::new(move |event| {
                    let _ = match event {
                        EditEvent::Update(info) => handle.emit("edit-update", info),
                        EditEvent::Conflict(conflict) => handle.emit("edit-conflict", conflict),
                        EditEvent::ConfirmSave(confirm) => handle.emit("edit-confirm", confirm),
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
            let config_dir = ConnectionStore::default_dir().unwrap_or_else(|| runtime_dir.join("config"));
            let connection_store = ConnectionStore::new(config_dir.clone());
            let (store, connections_warning) = load_connections(&connection_store, &logger);
            let settings_store = settings::SettingsStore::new(&config_dir);
            let op = logger.start(None, log::Channel::Local, "settings-load", settings_store.path().to_string_lossy());
            let settings = match settings_store.load() {
                (settings, None) => {
                    op.ok(Some(format!("ssh: {}", settings.ssh_bin().display())));
                    settings
                }
                (settings, Some(problem)) => {
                    op.fail(format!("{problem}: uso le impostazioni predefinite"));
                    settings
                }
            };
            app.manage(AppState {
                settings_store,
                settings: Mutex::new(settings),
                runtime_dir,
                askpass,
                logger,
                transfers,
                edits,
                planner: Planner::new(),
                connection_store,
                connections: Mutex::new(store),
                connections_warning: Mutex::new(connections_warning),
                next_session_id: AtomicU64::new(1),
                sessions: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_hosts,
            settings_load,
            settings_save,
            ssh_check,
            open_terminal,
            terminal_list,
            connections_load,
            connections_save,
            connection_forget_password,
            connection_check,
            connections_export,
            connections_import,
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
            transfer_resume,
            transfer_list,
            transfer_clear_finished,
            transfer_conflict_respond,
            fs_mkdir,
            fs_rename,
            fs_move,
            fs_delete,
            fs_attrs,
            fs_dir_size,
            fs_plan,
            fs_plan_execute,
            fs_plan_discard,
            edit_open,
            edit_list,
            edit_close,
            edit_resolve,
            edit_confirm_save,
            edit_reopen,
            edit_reveal,
            local_open
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            let state = handle.state::<AppState>();
            state.edits.close_unchanged();
            for open in state.sessions.lock().unwrap().values() {
                open.session.launcher.stop_master_blocking();
            }
            state.askpass.remove_socket();
        }
    });
}
