//! Headless check of resuming interrupted transfers against the sshd fixture:
//! a download, an upload and a streamed copy are cancelled midway and
//! resumed from their .gf-part; completed files are skipped; a changed
//! source restarts; a manager restarted from the manifest resumes too;
//! clearing a job removes its part file.
//!
//! scripts/test-sshd.sh start
//! cargo run --release --example resume -- "$PWD/../.dev-sshd/ssh" gf-slow segreta <work-dir>
//! (gf-slow adds latency, so there is time to cancel midway; the fixture
//! server is localhost, so paths are the same on both sides)

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use glassferry_lib::askpass::{AskpassServer, PromptKind};
use glassferry_lib::log::Logger;
use glassferry_lib::runtime_dir;
use glassferry_lib::session::{Session, SshLauncher};
use glassferry_lib::transfer::{Direction, Endpoint, JobInfo, JobState, TransferEvent, TransferManager};

fn main() {
    if let Some(code) = glassferry_lib::askpass::run_helper_if_requested() {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().collect();
    let [_, ssh_bin, alias, secret, work] = args.as_slice() else {
        eprintln!("usage: resume <ssh-bin> <alias> <secret> <work-dir>");
        std::process::exit(2);
    };
    let (ssh_bin, alias, secret, work) = (PathBuf::from(ssh_bin), alias.clone(), secret.clone(), PathBuf::from(work));

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
        let connect = |name: &str| {
            let launcher = SshLauncher {
                ssh_bin: ssh_bin.clone(),
                alias: alias.clone(),
                config_file: None,
                label: alias.clone(),
                control_path: dir.join(format!("resume-{name}.sock")),
                log_path: dir.join(format!("resume-{name}.log")),
                env: askpass.env_for(&alias, 0),
                verbose: false,
                logger: logger.clone(),
            };
            async move { Arc::new(Session::connect(launcher).await.expect("connect")) }
        };
        let a = connect("a").await;
        let b = connect("b").await;
        let ep = |id: u64, session: &Arc<Session>| Endpoint {
            session_id: id,
            connection_id: "conn".into(),
            session: session.clone(),
        };

        let _ = std::fs::remove_dir_all(&work);
        let src = work.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("small.txt"), b"small file\n").unwrap();
        std::fs::write(src.join("big.bin"), pseudo_random(40 * 1024 * 1024, 7)).unwrap();
        std::fs::write(src.join("other.bin"), pseudo_random(40 * 1024 * 1024, 8)).unwrap();

        let manifest = work.join("transfers.json");
        let conflicts = Arc::new(Mutex::new(0));
        let seen = conflicts.clone();
        let manager = TransferManager::new(
            logger.clone(),
            Some(manifest.clone()),
            Arc::new(move |event| {
                if let TransferEvent::Conflict(_) = event {
                    *seen.lock().unwrap() += 1;
                }
            }),
        );

        let check = |label: &str, ok: bool| println!("{} {label}", if ok { "PASS" } else { "FAIL" });
        let count_log = |needle: &str| logger.snapshot().iter().filter(|e| e.detail.contains(needle) || e.op == needle).count();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let names = |list: &[&str]| list.iter().map(|n| n.to_string()).collect::<Vec<_>>();

        // Download: cancelled midway, resumed from the part file.
        let down = work.join("down");
        std::fs::create_dir_all(&down).unwrap();
        let id = manager.start(ep(1, &a), Direction::Download, s(&src), names(&["small.txt", "big.bin"]), s(&down), None);
        let info = cancel_midway(&manager, id).await;
        let part = down.join("big.bin.gf-part");
        check(
            "download cancelled: resumable, part file kept",
            info.state == JobState::Cancelled
                && info.resumable
                && part.exists()
                && std::fs::metadata(&part).unwrap().len() > 0
                && manifest.exists(),
        );
        let (resumed, skipped) = (count_log("ripreso da"), count_log("resume-skip"));
        let id = manager.resume(id, ep(1, &a), None).unwrap();
        let info = wait(&manager, id).await;
        check(
            "download resumed from the part, completed file skipped",
            info.state == JobState::Done
                && same(&src.join("big.bin"), &down.join("big.bin"))
                && same(&src.join("small.txt"), &down.join("small.txt"))
                && count_log("ripreso da") == resumed + 1
                && count_log("resume-skip") == skipped + 1
                && !part.exists()
                && *conflicts.lock().unwrap() == 0,
        );

        // Upload.
        let up = work.join("up");
        std::fs::create_dir_all(&up).unwrap();
        let id = manager.start(ep(1, &a), Direction::Upload, s(&src), names(&["big.bin"]), s(&up), None);
        let info = cancel_midway(&manager, id).await;
        check("upload cancelled: resumable", info.state == JobState::Cancelled && info.resumable && up.join("big.bin.gf-part").exists());
        let resumed = count_log("ripreso da");
        let id = manager.resume(id, ep(1, &a), None).unwrap();
        let info = wait(&manager, id).await;
        check(
            "upload resumed from the part",
            info.state == JobState::Done && same(&src.join("big.bin"), &up.join("big.bin")) && count_log("ripreso da") == resumed + 1,
        );

        // Copy streamed between two sessions.
        let copy = work.join("copy");
        std::fs::create_dir_all(&copy).unwrap();
        let start_copy = |manager: &Arc<TransferManager>| {
            manager.start(ep(1, &a), Direction::Remote, s(&src), names(&["big.bin"]), s(&copy), Some(ep(2, &b)))
        };
        let id = start_copy(&manager);
        let info = cancel_midway(&manager, id).await;
        check("streamed copy cancelled: resumable", info.state == JobState::Cancelled && info.resumable);
        let resumed = count_log("ripreso da");
        check(
            "resume refuses a different destination connection",
            manager
                .resume(id, ep(1, &a), Some(Endpoint { session_id: 2, connection_id: "other".into(), session: b.clone() }))
                .is_err(),
        );
        let id = manager.resume(id, ep(1, &a), Some(ep(2, &b))).unwrap();
        let info = wait(&manager, id).await;
        check(
            "streamed copy resumed from the part",
            info.state == JobState::Done && same(&src.join("big.bin"), &copy.join("big.bin")) && count_log("ripreso da") == resumed + 1,
        );

        // A source changed since the interruption starts over.
        let down2 = work.join("down2");
        std::fs::create_dir_all(&down2).unwrap();
        let id = manager.start(ep(1, &a), Direction::Download, s(&src), names(&["other.bin"]), s(&down2), None);
        cancel_midway(&manager, id).await;
        std::fs::write(src.join("other.bin"), pseudo_random(40 * 1024 * 1024 + 5, 9)).unwrap();
        let resumed = count_log("ripreso da");
        let id = manager.resume(id, ep(1, &a), None).unwrap();
        let info = wait(&manager, id).await;
        check(
            "changed source restarts from zero",
            info.state == JobState::Done && same(&src.join("other.bin"), &down2.join("other.bin")) && count_log("ripreso da") == resumed,
        );

        // The connection drops midway: the job fails, a new session resumes it.
        let dropped = connect("dropped").await;
        let down5 = work.join("down5");
        std::fs::create_dir_all(&down5).unwrap();
        let id = manager.start(ep(4, &dropped), Direction::Download, s(&src), names(&["big.bin"]), s(&down5), None);
        loop {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if manager.list().into_iter().find(|j| j.id == id).unwrap().done_bytes > 4 * 1024 * 1024 {
                break;
            }
        }
        dropped.launcher.stop_master().await;
        let info = wait(&manager, id).await;
        check(
            "dropped connection: job failed, resumable, part kept",
            info.state == JobState::Failed && info.resumable && down5.join("big.bin.gf-part").exists(),
        );
        let fresh = connect("fresh").await;
        let resumed = count_log("ripreso da");
        let id = manager.resume(id, ep(5, &fresh), None).unwrap();
        let info = wait(&manager, id).await;
        check(
            "resumed on a new session of the same connection",
            info.state == JobState::Done && same(&src.join("big.bin"), &down5.join("big.bin")) && count_log("ripreso da") == resumed + 1,
        );

        // Restart: a new manager reads the interrupted job from the manifest.
        let down3 = work.join("down3");
        std::fs::create_dir_all(&down3).unwrap();
        let id = manager.start(ep(1, &a), Direction::Download, s(&src), names(&["big.bin"]), s(&down3), None);
        cancel_midway(&manager, id).await;
        drop(manager);
        let restarted = TransferManager::new(logger.clone(), Some(manifest.clone()), Arc::new(|_| {}));
        let restored: Vec<JobInfo> = restarted.list().into_iter().filter(|j| j.dst_dir == s(&down3)).collect();
        check(
            "restart restores the interrupted job without a session",
            restored.len() == 1 && restored[0].resumable && restored[0].session_id.is_none(),
        );
        let resumed = count_log("ripreso da");
        let id = restarted.resume(restored[0].id, ep(3, &a), None).unwrap();
        let info = wait(&restarted, id).await;
        check(
            "restored job resumes from the part",
            info.state == JobState::Done && same(&src.join("big.bin"), &down3.join("big.bin")) && count_log("ripreso da") == resumed + 1,
        );

        // Clearing an interrupted job removes its part file.
        let down4 = work.join("down4");
        std::fs::create_dir_all(&down4).unwrap();
        let id = restarted.start(ep(3, &a), Direction::Download, s(&src), names(&["big.bin"]), s(&down4), None);
        cancel_midway(&restarted, id).await;
        let had_part = down4.join("big.bin.gf-part").exists();
        restarted.clear_finished();
        check(
            "clearing removes the part file and the manifest",
            had_part && !down4.join("big.bin.gf-part").exists() && restarted.list().is_empty() && !manifest.exists(),
        );

        for session in [a, b] {
            match Arc::try_unwrap(session) {
                Ok(s) => s.close().await,
                Err(s) => s.launcher.stop_master().await,
            }
        }
        let _ = std::fs::remove_dir_all(&work);
        tokio::time::sleep(Duration::from_millis(200)).await;
    });
}

/// Cancels the job once a few MiB went through, then waits for it to stop.
async fn cancel_midway(manager: &TransferManager, id: u64) -> JobInfo {
    loop {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let info = manager.list().into_iter().find(|j| j.id == id).unwrap();
        if info.done_bytes > 4 * 1024 * 1024 || is_final(&info) {
            break;
        }
    }
    manager.cancel(id);
    wait(manager, id).await
}

async fn wait(manager: &TransferManager, id: u64) -> JobInfo {
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let info = manager.list().into_iter().find(|j| j.id == id).unwrap();
        if is_final(&info) {
            return info;
        }
    }
}

fn is_final(info: &JobInfo) -> bool {
    matches!(info.state, JobState::Done | JobState::Failed | JobState::Cancelled)
}

fn same(a: &Path, b: &Path) -> bool {
    matches!((std::fs::read(a), std::fs::read(b)), (Ok(x), Ok(y)) if x == y)
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
