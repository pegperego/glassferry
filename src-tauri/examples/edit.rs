//! Headless check of remote editing against the sshd fixture (no editor
//! window is opened): local saves are uploaded, unchanged saves are not,
//! remote changes raise a conflict.
//!
//! cargo run --example edit -- "$PWD/../.dev-sshd/ssh" gf-local segreta <work-dir>
//! (the fixture server is localhost, so <work-dir> is also the "remote" path)

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use glassferry_lib::askpass::{AskpassServer, PromptKind};
use glassferry_lib::edit::{ConflictChoice, EditEvent, EditManager, EditState};
use glassferry_lib::log::Logger;
use glassferry_lib::runtime_dir;
use glassferry_lib::session::{Session, SshLauncher};

fn main() {
    if let Some(code) = glassferry_lib::askpass::run_helper_if_requested() {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().collect();
    let [_, ssh_bin, alias, secret, work] = args.as_slice() else {
        eprintln!("usage: edit <ssh-bin> <alias> <secret> <work-dir>");
        std::process::exit(2);
    };
    let (alias, secret, work) = (alias.clone(), secret.clone(), PathBuf::from(work));

    tauri::async_runtime::block_on(async move {
        let dir = runtime_dir::create().unwrap();
        let logger = Logger::new(None, None);
        let server: Arc<OnceLock<Arc<AskpassServer>>> = Arc::new(OnceLock::new());
        let responder = server.clone();
        let askpass = AskpassServer::start(
            &dir,
            Arc::new(move |req| {
                let answer = if req.kind == PromptKind::Secret { secret.clone() } else { "yes".into() };
                if req.kind != PromptKind::Notice {
                    responder.get().unwrap().respond(req.id, Some(answer), false);
                }
            }),
            logger.clone(),
        )
        .unwrap();
        let _ = server.set(askpass.clone());
        let session = Arc::new(
            Session::connect(SshLauncher {
                ssh_bin: PathBuf::from(ssh_bin),
                alias: alias.clone(),
                config_file: None,
                label: alias.clone(),
                control_path: dir.join("edit.sock"),
                log_path: dir.join("edit.log"),
                env: askpass.env_for(&alias, 0),
                verbose: false,
                logger: logger.clone(),
            })
            .await
            .expect("connect"),
        );

        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(work.join("remote")).unwrap();
        let remote = work.join("remote/nginx.conf");
        std::fs::write(&remote, "server { listen 80; }\n").unwrap();

        // Conflict policy is scripted per step through this cell.
        let next_choice: Arc<Mutex<ConflictChoice>> = Arc::new(Mutex::new(ConflictChoice::Keep));
        let conflicts = Arc::new(Mutex::new(0));
        // Upload confirmations (protected connection): answer and count.
        let next_confirm = Arc::new(Mutex::new(false));
        let confirms = Arc::new(Mutex::new(0));
        let manager_cell: Arc<OnceLock<Arc<EditManager>>> = Arc::new(OnceLock::new());
        let (cell, choice, count) = (manager_cell.clone(), next_choice.clone(), conflicts.clone());
        let (allow, confirm_count) = (next_confirm.clone(), confirms.clone());
        let manager = EditManager::new(
            work.join("cache"),
            Arc::new(move |event| match event {
                EditEvent::Conflict(c) => {
                    *count.lock().unwrap() += 1;
                    let (manager, choice) = (cell.get().unwrap().clone(), *choice.lock().unwrap());
                    // Answer from another task, like the UI would.
                    tauri::async_runtime::spawn(async move {
                        let _ = manager.resolve(c.id, choice);
                    });
                }
                EditEvent::ConfirmSave(c) => {
                    *confirm_count.lock().unwrap() += 1;
                    let (manager, allow) = (cell.get().unwrap().clone(), *allow.lock().unwrap());
                    tauri::async_runtime::spawn(async move {
                        let _ = manager.confirm_save(c.id, allow);
                    });
                }
                EditEvent::Update(_) => {}
            }),
        );
        manager.set_launch_editor(false);
        let _ = manager_cell.set(manager.clone());

        let check = |label: &str, ok: bool| println!("{} {label}", if ok { "PASS" } else { "FAIL" });
        let settle = || tokio::time::sleep(Duration::from_millis(1500));
        let info = manager.open(1, session.clone(), remote.to_string_lossy().into_owned(), false).await.unwrap();
        let local = PathBuf::from(&info.local_path);
        check("local copy matches", std::fs::read(&local).unwrap() == std::fs::read(&remote).unwrap());
        let uploads = || manager.list()[0].uploads;
        let state = || manager.list()[0].state;

        std::fs::write(&local, "server { listen 8080; }\n").unwrap();
        settle().await;
        check("save uploads", std::fs::read_to_string(&remote).unwrap().contains("8080") && uploads() == 1);

        // Atomic save, like many editors: write elsewhere, rename over.
        let tmp = local.with_file_name(".nginx.conf.swp");
        std::fs::write(&tmp, "server { listen 8443; }\n").unwrap();
        std::fs::rename(&tmp, &local).unwrap();
        settle().await;
        check("atomic save uploads", std::fs::read_to_string(&remote).unwrap().contains("8443") && uploads() == 2);

        std::fs::write(&local, std::fs::read(&local).unwrap()).unwrap();
        settle().await;
        check("unchanged save does not upload", uploads() == 2);

        // Someone else changes the server copy (same size, same second).
        std::fs::write(&remote, "server { listen 9443; }\n").unwrap();
        *next_choice.lock().unwrap() = ConflictChoice::Reload;
        std::fs::write(&local, "server { listen 7443; }\n").unwrap();
        settle().await;
        let backup = local.with_file_name("nginx (mia versione).conf");
        check(
            "remote change detected, reload keeps a backup",
            *conflicts.lock().unwrap() == 1
                && std::fs::read_to_string(&local).unwrap().contains("9443")
                && std::fs::read_to_string(&backup).unwrap().contains("7443")
                && state() == EditState::Synced,
        );

        std::fs::write(&remote, "server { listen 1; } # changed again\n").unwrap();
        *next_choice.lock().unwrap() = ConflictChoice::Overwrite;
        std::fs::write(&local, "server { listen 2; }\n").unwrap();
        settle().await;
        check(
            "overwrite after conflict",
            *conflicts.lock().unwrap() == 2 && std::fs::read_to_string(&remote).unwrap().contains("listen 2;"),
        );

        std::fs::write(&remote, "server { listen 3; } # third change\n").unwrap();
        *next_choice.lock().unwrap() = ConflictChoice::Keep;
        std::fs::write(&local, "server { listen 4; }\n").unwrap();
        settle().await;
        check(
            "keep leaves both untouched",
            std::fs::read_to_string(&remote).unwrap().contains("listen 3;") && state() == EditState::Conflict,
        );
        manager.resolve(info.id, ConflictChoice::Overwrite).unwrap();
        settle().await;
        check("resolve later uploads", std::fs::read_to_string(&remote).unwrap().contains("listen 4;") && state() == EditState::Synced);

        manager.close(info.id);
        check("close removes local copy", !local.exists() && manager.list().is_empty());

        // Protected connection: the first upload waits for a confirmation.
        let info = manager.open(1, session.clone(), remote.to_string_lossy().into_owned(), true).await.unwrap();
        let local = PathBuf::from(&info.local_path);
        std::fs::write(&local, "server { listen 5; }\n").unwrap();
        settle().await;
        check(
            "refused confirmation keeps the server untouched",
            *confirms.lock().unwrap() == 1
                && std::fs::read_to_string(&remote).unwrap().contains("listen 4;")
                && state() == EditState::Unconfirmed,
        );
        // "Carica sul server" later asks again, then uploads.
        *next_confirm.lock().unwrap() = true;
        manager.confirm_save(info.id, true).unwrap();
        settle().await;
        check(
            "retrying asks again and uploads",
            *confirms.lock().unwrap() == 2 && std::fs::read_to_string(&remote).unwrap().contains("listen 5;"),
        );
        std::fs::write(&local, "server { listen 6; }\n").unwrap();
        settle().await;
        check(
            "once confirmed, later saves do not ask",
            *confirms.lock().unwrap() == 2 && std::fs::read_to_string(&remote).unwrap().contains("listen 6;"),
        );
        manager.close(info.id);

        match Arc::try_unwrap(session) {
            Ok(s) => s.close().await,
            Err(s) => s.launcher.stop_master().await,
        }
        let _ = std::fs::remove_dir_all(&work);
        tokio::time::sleep(Duration::from_millis(200)).await;
    });
}
