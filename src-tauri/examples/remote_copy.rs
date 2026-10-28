//! Headless check of remote → remote copies against the sshd fixture:
//! within one session (copy-data, then the `cp` and streaming fallbacks)
//! and between two sessions (streaming), plus a conflict on the second run.
//!
//! scripts/test-sshd.sh start
//! cargo run --release --example remote_copy -- "$PWD/../.dev-sshd/ssh" segreta <work-dir>

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use sshfilemanager_lib::askpass::{AskpassServer, PromptKind};
use sshfilemanager_lib::log::Logger;
use sshfilemanager_lib::runtime_dir;
use sshfilemanager_lib::session::{Session, SshLauncher};
use sshfilemanager_lib::transfer::{
    copy_strategy, ConflictAction, CopyStrategy, Direction, JobInfo, JobState, TransferEvent, TransferManager,
};

fn main() {
    if let Some(code) = sshfilemanager_lib::askpass::run_helper_if_requested() {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().collect();
    let [_, ssh_bin, secret, work] = args.as_slice() else {
        eprintln!("usage: remote_copy <ssh-bin> <secret> <work-dir>");
        std::process::exit(2);
    };
    let (ssh_bin, secret, work) = (PathBuf::from(ssh_bin), secret.clone(), PathBuf::from(work));

    tauri::async_runtime::block_on(async move {
        let dir = runtime_dir::create().unwrap();
        let logger = Logger::new(None, None);
        let server: Arc<OnceLock<Arc<AskpassServer>>> = Arc::new(OnceLock::new());
        let responder = server.clone();
        let askpass = AskpassServer::start(
            &dir,
            Arc::new(move |req| {
                let answer = match req.kind {
                    PromptKind::Secret => Some(secret.clone()),
                    PromptKind::HostKey => Some("yes".into()),
                    PromptKind::Confirm => Some(String::new()),
                    PromptKind::Notice => return,
                };
                responder.get().unwrap().respond(req.id, answer, false);
            }),
            logger.clone(),
        )
        .unwrap();
        let _ = server.set(askpass.clone());
        let connect = |alias: &str| {
            let launcher = SshLauncher {
                ssh_bin: ssh_bin.clone(),
                alias: alias.into(),
                config_file: None,
                label: alias.into(),
                control_path: dir.join(format!("rc-{alias}.sock")),
                log_path: dir.join(format!("rc-{alias}.log")),
                env: askpass.env_for(alias, 0),
                verbose: false,
                logger: logger.clone(),
            };
            async move { Arc::new(Session::connect(launcher).await.expect("connect")) }
        };
        let a = connect("sfm-local").await;
        let b = connect("sfm-sftponly").await;

        let conflicts = Arc::new(Mutex::new(Vec::new()));
        let cell: Arc<OnceLock<Arc<TransferManager>>> = Arc::new(OnceLock::new());
        let (seen, manager_cell) = (conflicts.clone(), cell.clone());
        let manager = TransferManager::new(
            logger.clone(),
            Arc::new(move |event| {
                if let TransferEvent::Conflict(req) = event {
                    seen.lock().unwrap().push(req.commands.clone());
                    manager_cell.get().unwrap().respond_conflict(req.id, ConflictAction::Overwrite, true);
                }
            }),
        );
        let _ = cell.set(manager.clone());

        let _ = std::fs::remove_dir_all(&work);
        let src = work.join("src");
        std::fs::create_dir_all(src.join("tree/nested")).unwrap();
        std::fs::create_dir_all(src.join("tree/empty")).unwrap();
        for (i, size) in [0usize, 1, 300 * 1024, 5 * 1024 * 1024 + 3].iter().enumerate() {
            let sub = if i % 2 == 0 { "tree" } else { "tree/nested" };
            std::fs::write(src.join(format!("{sub}/f{i}.bin")), pseudo_random(*size, i as u64)).unwrap();
        }
        std::fs::write(src.join("tree/it's \"quoted\".txt"), b"q").unwrap();

        let check = |label: &str, ok: bool| println!("{} {label}", if ok { "PASS" } else { "FAIL" });
        let copy = |from: &Arc<Session>, to: &Arc<Session>, dst: &Path| {
            manager.start(
                1,
                from.clone(),
                Direction::Remote,
                src.to_string_lossy().into_owned(),
                vec!["tree".into()],
                dst.to_string_lossy().into_owned(),
                Some((2, to.clone())),
            )
        };
        let identical = |dst: &Path| {
            std::process::Command::new("diff")
                .arg("-r")
                .arg(src.join("tree"))
                .arg(dst.join("tree"))
                .status()
                .unwrap()
                .success()
        };

        check("same session uses copy-data", copy_strategy(&a, &a) == CopyStrategy::CopyData);
        check("two sessions stream", copy_strategy(&a, &b) == CopyStrategy::Stream);
        for (i, (label, from, to, strategy)) in [
            ("copy-data (same session)", &a, &a, ""),
            ("cp -p (same session)", &a, &a, "shell"),
            ("stream (same session)", &a, &a, "stream"),
            ("stream (two sessions)", &a, &b, ""),
        ]
        .into_iter()
        .enumerate()
        {
            // SAFETY: single-threaded at this point (no job running).
            unsafe { std::env::set_var("SFM_COPY_STRATEGY", strategy) };
            let dst = work.join(format!("dst-{i}"));
            std::fs::create_dir_all(&dst).unwrap();
            let t = Instant::now();
            let info = wait(&manager, copy(from, to, &dst)).await;
            println!(
                "     {label}: {:?} {} file, {} B in {:.2}s",
                info.state,
                info.done_files,
                info.done_bytes,
                t.elapsed().as_secs_f64()
            );
            check(label, info.state == JobState::Done && identical(&dst));
        }
        unsafe { std::env::remove_var("SFM_COPY_STRATEGY") };

        // dst-3 already holds the two-session copy: every file conflicts.
        let dst = work.join("dst-3");
        let info = wait(&manager, copy(&a, &b, &dst)).await;
        let prompts = conflicts.lock().unwrap().clone();
        check(
            "second copy asks once, overwrite all",
            info.state == JobState::Done && prompts.len() == 1 && identical(&dst),
        );
        println!("     conflict commands: {:?}", prompts.first());
        let parts = walk(&work).into_iter().filter(|p| p.to_string_lossy().ends_with(".sfm-part")).count();
        check("no .sfm-part left", parts == 0);

        manager.clear_finished();
        drop(manager);
        for s in [a, b] {
            match Arc::try_unwrap(s) {
                Ok(s) => s.close().await,
                Err(s) => s.launcher.stop_master().await,
            }
        }
        let _ = std::fs::remove_dir_all(&work);
        tokio::time::sleep(Duration::from_millis(300)).await;
    });
}

async fn wait(manager: &TransferManager, id: u64) -> JobInfo {
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let info = manager.list().into_iter().find(|j| j.id == id).unwrap();
        if matches!(info.state, JobState::Done | JobState::Failed | JobState::Cancelled) {
            return info;
        }
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            }
            out.push(path);
        }
    }
    out
}

fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}
