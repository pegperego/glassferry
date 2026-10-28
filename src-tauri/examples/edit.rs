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
            logger.clone(),
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
        let info = manager.open("conn", 1, session.clone(), remote.to_string_lossy().into_owned(), false).await.unwrap();
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
        let info = manager.open("conn", 1, session.clone(), remote.to_string_lossy().into_owned(), true).await.unwrap();
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

        // Disconnect: a copy with changes is detached, an unchanged one closed.
        let other = work.join("remote/other.txt");
        std::fs::write(&other, "untouched\n").unwrap();
        let unchanged = manager.open("conn", 1, session.clone(), other.to_string_lossy().into_owned(), false).await.unwrap();
        let info = manager.open("conn", 1, session.clone(), remote.to_string_lossy().into_owned(), false).await.unwrap();
        let local = PathBuf::from(&info.local_path);
        // Saved just before disconnecting: the debounce has not uploaded it yet.
        std::fs::write(&local, "server { listen 10; }\n").unwrap();
        manager.close_session(1);
        check(
            "disconnect keeps the changed copy, closes the unchanged one",
            local.exists()
                && !PathBuf::from(&unchanged.local_path).exists()
                && manager.list().len() == 1
                && state() == EditState::Detached
                && std::fs::read_to_string(&remote).unwrap().contains("listen 6;"),
        );
        std::fs::write(&local, "server { listen 11; }\n").unwrap();
        settle().await;
        check(
            "saves while detached stay local",
            std::fs::read_to_string(&remote).unwrap().contains("listen 6;") && state() == EditState::Detached,
        );

        // Restart (or crash): a new manager restores the copy from the manifest.
        let restarted = EditManager::new(work.join("cache"), logger.clone(), Arc::new(|_| {}));
        restarted.set_launch_editor(false);
        let restored = restarted.list();
        check(
            "restart restores the detached copy",
            restored.len() == 1 && restored[0].state == EditState::Detached && restored[0].local_path == info.local_path,
        );
        restarted.attach("other-connection", 2, session.clone());
        check("other connections do not attach it", restarted.list()[0].state == EditState::Detached);
        restarted.attach("conn", 2, session.clone());
        settle().await;
        let attached = &restarted.list()[0];
        check(
            "reconnecting uploads the kept changes",
            std::fs::read_to_string(&remote).unwrap().contains("listen 11;")
                && attached.state == EditState::Synced
                && attached.session_id == Some(2),
        );
        std::fs::write(&local, "server { listen 12; }\n").unwrap();
        settle().await;
        check("saves upload again after reattaching", std::fs::read_to_string(&remote).unwrap().contains("listen 12;"));
        restarted.close_session(2);
        check(
            "synced copies close on disconnect, manifest removed",
            restarted.list().is_empty() && !local.exists() && !work.join("cache/edits.json").exists(),
        );

        // Saves replace the file in one step (new inode, no partial file if
        // the connection drops), keeping its mode.
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let remote_path = remote.to_string_lossy().into_owned();
        std::fs::set_permissions(&remote, std::fs::Permissions::from_mode(0o640)).unwrap();
        let inode = std::fs::metadata(&remote).unwrap().ino();
        let info = restarted.open("conn2", 3, session.clone(), remote_path.clone(), false).await.unwrap();
        std::fs::write(&info.local_path, "server { listen 13; }\n").unwrap();
        settle().await;
        let meta = std::fs::metadata(&remote).unwrap();
        check(
            "save replaces the file in one step, keeping its mode",
            std::fs::read_to_string(&remote).unwrap().contains("listen 13;")
                && meta.ino() != inode
                && meta.mode() & 0o777 == 0o640
                && !work.join("remote/nginx.conf.gf-part").exists(),
        );
        restarted.close(info.id);

        // A symlink is written in place: the link stays, the target changes.
        let link = work.join("remote/link.conf");
        std::os::unix::fs::symlink(&remote, &link).unwrap();
        let info = restarted.open("conn2", 3, session.clone(), link.to_string_lossy().into_owned(), false).await.unwrap();
        std::fs::write(&info.local_path, "server { listen 14; }\n").unwrap();
        settle().await;
        check(
            "symlink stays a link, its target is updated",
            std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink()
                && std::fs::read_to_string(&remote).unwrap().contains("listen 14;"),
        );
        restarted.close(info.id);

        // A group the new file would not get: written in place, group kept.
        let new_file_gid = {
            let probe = work.join("remote/.gid-probe");
            std::fs::write(&probe, "").unwrap();
            let gid = std::fs::metadata(&probe).unwrap().gid();
            std::fs::remove_file(&probe).unwrap();
            gid
        };
        match secondary_groups().into_iter().find(|g| *g != new_file_gid) {
            Some(gid) => {
                std::os::unix::fs::chown(&remote, None, Some(gid)).unwrap();
                let inode = std::fs::metadata(&remote).unwrap().ino();
                let info = restarted.open("conn2", 3, session.clone(), remote_path.clone(), false).await.unwrap();
                std::fs::write(&info.local_path, "server { listen 15; }\n").unwrap();
                settle().await;
                let meta = std::fs::metadata(&remote).unwrap();
                check(
                    "a group the new file would not get: written in place",
                    std::fs::read_to_string(&remote).unwrap().contains("listen 15;")
                        && meta.gid() == gid
                        && meta.ino() == inode,
                );
                restarted.close(info.id);
            }
            None => println!("SKIP group check: the user has no other group"),
        }

        match Arc::try_unwrap(session) {
            Ok(s) => s.close().await,
            Err(s) => s.launcher.stop_master().await,
        }
        let _ = std::fs::remove_dir_all(&work);
        tokio::time::sleep(Duration::from_millis(200)).await;
    });
}

/// Groups of the current user (`getgroups`).
fn secondary_groups() -> Vec<u32> {
    let mut groups = vec![0 as libc::gid_t; 64];
    let n = unsafe { libc::getgroups(groups.len() as libc::c_int, groups.as_mut_ptr()) };
    groups.truncate(n.max(0) as usize);
    groups
}
