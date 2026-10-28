//! Headless transfer check against the local sshd fixture:
//! upload a tree, download it back, re-upload with conflicts, cancel.
//!
//! scripts/test-sshd.sh start
//! cargo run --release --example transfer -- "$PWD/../.dev-sshd/ssh" gf-local segreta <work-dir>

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use glassferry_lib::askpass::{AskpassServer, PromptKind};
use glassferry_lib::log::{Logger, Status};
use glassferry_lib::runtime_dir;
use glassferry_lib::session::{Session, SshLauncher};
use glassferry_lib::transfer::{ConflictAction, Direction, JobInfo, JobState, TransferEvent, TransferManager};

fn main() {
    if let Some(code) = glassferry_lib::askpass::run_helper_if_requested() {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().collect();
    let [_, ssh_bin, alias, secret, work] = args.as_slice() else {
        eprintln!("usage: transfer <ssh-bin> <alias> <secret> <work-dir>");
        std::process::exit(2);
    };
    let work = PathBuf::from(work);
    let (alias, secret) = (alias.clone(), secret.clone());

    tauri::async_runtime::block_on(async move {
        let dir = runtime_dir::create().unwrap();
        let logger = Logger::new(
            Some(Arc::new(|batch| {
                for e in batch.iter().filter(|e| e.status == Status::Error) {
                    println!("[log error] {} {} {} {:?}", e.op, e.detail, e.error.clone().unwrap_or_default(), e.output);
                }
            })),
            None,
        );
        let server: Arc<OnceLock<Arc<AskpassServer>>> = Arc::new(OnceLock::new());
        let responder = server.clone();
        let askpass = AskpassServer::start(
            &dir,
            Arc::new(move |req| {
                let answer = match req.kind {
                    PromptKind::Secret => Some(secret.clone()),
                    PromptKind::HostKey => Some("yes".into()),
                    _ => Some(String::new()),
                };
                if req.kind != PromptKind::Notice {
                    responder.get().unwrap().respond(req.id, answer, false);
                }
            }),
            logger.clone(),
        )
        .unwrap();
        let _ = server.set(askpass.clone());

        let launcher = SshLauncher {
            ssh_bin: PathBuf::from(ssh_bin),
            alias: alias.clone(),
            config_file: None,
            label: alias.clone(),
            control_path: dir.join("transfer.sock"),
            log_path: dir.join("transfer.log"),
            env: askpass.env_for(&alias, 0),
            verbose: false,
            logger: logger.clone(),
        };
        let session = Arc::new(Session::connect(launcher).await.expect("connect"));

        // Conflict answers: overwrite all on the first prompt.
        let latest: Arc<Mutex<Vec<JobInfo>>> = Arc::new(Mutex::new(Vec::new()));
        let manager_cell: Arc<OnceLock<Arc<TransferManager>>> = Arc::new(OnceLock::new());
        let conflicts = Arc::new(Mutex::new(0));
        let (latest_sink, cell, conflict_count) = (latest.clone(), manager_cell.clone(), conflicts.clone());
        let manager = TransferManager::new(
            logger.clone(),
            Arc::new(move |event| match event {
                TransferEvent::Updates(jobs) => latest_sink.lock().unwrap().extend(jobs),
                TransferEvent::Conflict(req) => {
                    *conflict_count.lock().unwrap() += 1;
                    cell.get().unwrap().respond_conflict(req.id, ConflictAction::Overwrite, true);
                }
            }),
        );
        let _ = manager_cell.set(manager.clone());

        // Local source tree with edge-case sizes.
        let src = work.join("src");
        let remote = work.join("remote");
        let back = work.join("back");
        for d in [&src, &remote, &back] {
            let _ = std::fs::remove_dir_all(d);
        }
        std::fs::create_dir_all(src.join("tree/nested/deeper")).unwrap();
        std::fs::create_dir_all(src.join("tree/empty-dir")).unwrap();
        std::fs::create_dir_all(&remote).unwrap();
        std::fs::create_dir_all(&back).unwrap();
        let sizes = [0usize, 1, 4095, 256 * 1024, 256 * 1024 + 1, 3 * 1024 * 1024 + 17];
        for (i, size) in sizes.iter().enumerate() {
            let sub = if i % 2 == 0 { "tree" } else { "tree/nested/deeper" };
            std::fs::write(src.join(format!("{sub}/f{i}.bin")), pseudo_random(*size, i as u64)).unwrap();
        }
        std::fs::write(src.join("tree/nome con spazi 'e' \"virgolette\".txt"), b"quote test").unwrap();
        let big_mb: usize = std::env::var("BIG_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
        std::fs::write(src.join("big.bin"), pseudo_random(big_mb * 1024 * 1024, 99)).unwrap();

        let run = |direction, from: &Path, names: Vec<&str>, to: &Path| {
            manager.start(
                1,
                session.clone(),
                direction,
                from.to_string_lossy().into_owned(),
                names.into_iter().map(String::from).collect(),
                to.to_string_lossy().into_owned(),
                None,
            )
        };

        let t = Instant::now();
        let up = run(Direction::Upload, &src, vec!["tree", "big.bin"], &remote);
        let info = wait(&manager, up).await;
        report("upload", &info, t.elapsed());

        let t = Instant::now();
        let down = run(Direction::Download, &remote, vec!["tree", "big.bin"], &back);
        let info = wait(&manager, down).await;
        report("download", &info, t.elapsed());

        let diff = std::process::Command::new("diff").arg("-r").arg(&src).arg(&back).status().unwrap();
        println!("diff -r src back: {}", if diff.success() { "IDENTICAL" } else { "DIFFERENT" });

        let t = Instant::now();
        let again = run(Direction::Upload, &src, vec!["tree"], &remote);
        let info = wait(&manager, again).await;
        report("re-upload (conflicts)", &info, t.elapsed());
        println!("conflict prompts: {} (expected 1, then overwrite-all)", conflicts.lock().unwrap());

        let cancel = run(Direction::Download, &remote, vec!["big.bin"], &work.join("back/tree"));
        tokio::time::sleep(Duration::from_millis(150)).await;
        manager.cancel(cancel);
        let info = wait(&manager, cancel).await;
        let leftovers = std::fs::read_dir(work.join("back/tree"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".gf-part") || e.file_name() == "big.bin")
            .count();
        println!("cancel: state={:?} leftovers={leftovers}", info.state);

        let mtime_src = std::fs::metadata(src.join("big.bin")).unwrap().modified().unwrap();
        let mtime_back = std::fs::metadata(back.join("big.bin")).unwrap().modified().unwrap();
        let delta = mtime_src.duration_since(mtime_back).unwrap_or_else(|e| e.duration());
        println!("mtime preserved: {}", delta < Duration::from_secs(1));

        manager.clear_finished();
        match Arc::try_unwrap(session) {
            Ok(session) => session.close().await,
            Err(shared) => {
                eprintln!("session still shared: stopping the master only");
                shared.launcher.stop_master().await;
            }
        }
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

fn report(label: &str, info: &JobInfo, elapsed: Duration) {
    println!(
        "{label}: {:?} files={}/{} skipped={} failed={} bytes={} in {:.2}s ({:.1} MiB/s) errors={:?}",
        info.state,
        info.done_files,
        info.total_files,
        info.skipped_files,
        info.failed_files,
        info.done_bytes,
        elapsed.as_secs_f64(),
        info.done_bytes as f64 / elapsed.as_secs_f64() / 1048576.0,
        info.errors
    );
}

/// Deterministic, incompressible-enough test data.
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
