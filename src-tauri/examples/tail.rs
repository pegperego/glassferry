//! Headless check of following a file against the sshd fixture.
//!
//! cargo run --example tail -- "$PWD/../.dev-sshd/ssh" gf-local segreta <work-dir>
//! (gf-local uses `tail -F`; gf-sftponly, or GF_TAIL_STRATEGY=sftp, the SFTP
//! polling; the local tail runs every time)

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use glassferry_lib::askpass::{AskpassServer, PromptKind};
use glassferry_lib::log::{Logger, Status};
use glassferry_lib::runtime_dir;
use glassferry_lib::session::{Session, SshLauncher};
use glassferry_lib::tail::{TailEvent, TailManager, TailMethod, TailTarget};

#[derive(Default)]
struct Seen {
    lines: Vec<String>,
    notices: Vec<String>,
    ended: Option<(u64, Option<String>)>,
    offset: u64,
}

fn main() {
    if let Some(code) = glassferry_lib::askpass::run_helper_if_requested() {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().collect();
    let [_, ssh_bin, alias, secret, work] = args.as_slice() else {
        eprintln!("usage: tail <ssh-bin> <alias> <secret> <work-dir>");
        std::process::exit(2);
    };
    let (alias, secret, work) = (alias.clone(), secret.clone(), work.clone());

    tauri::async_runtime::block_on(async move {
        let dir = runtime_dir::create().unwrap();
        let logger = Logger::new(
            Some(Arc::new(|batch| {
                for e in batch.iter().filter(|e| e.status != Status::Pending) {
                    println!("  [log] {:?} {} {:?} {} {} {}", e.channel, e.op, e.status, e.detail.lines().next().unwrap_or(""), e.extra.clone().unwrap_or_default(), e.error.clone().unwrap_or_default());
                }
            })),
            None,
        );
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
                control_path: dir.join("tail.sock"),
                log_path: dir.join("tail.log"),
                env: askpass.env_for(&alias, 0),
                verbose: false,
                logger: logger.clone(),
            })
            .await
            .expect("connect"),
        );
        println!("shell commands: {:?}", session.capabilities.commands);

        let seen: Arc<Mutex<std::collections::HashMap<u64, Seen>>> = Arc::default();
        let sink_seen = seen.clone();
        let tails = TailManager::new(
            logger.clone(),
            Arc::new(move |event| {
                let mut all = sink_seen.lock().unwrap();
                match event {
                    TailEvent::Lines { tail_id, lines, offset } => {
                        let s = all.entry(tail_id).or_default();
                        s.lines.extend(lines);
                        s.offset = offset;
                    }
                    TailEvent::Notice { tail_id, message } => {
                        println!("  [notice {tail_id}] {message}");
                        all.entry(tail_id).or_default().notices.push(message);
                    }
                    TailEvent::Ended { tail_id, offset, error } => {
                        let s = all.entry(tail_id).or_default();
                        s.ended = Some((offset, error));
                        s.offset = offset;
                    }
                }
            }),
        );

        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let mut failures = 0;
        let mut check = |label: &str, ok: bool| {
            println!("{} {label}", if ok { "PASS" } else { "FAIL" });
            if !ok {
                failures += 1;
            }
        };
        let wait = |id: u64, what: &dyn Fn(&Seen) -> bool| {
            let start = Instant::now();
            loop {
                if what(seen.lock().unwrap().entry(id).or_default()) {
                    return true;
                }
                if start.elapsed() > Duration::from_secs(12) {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        let append = |path: &str, text: &str| {
            let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
            f.write_all(text.as_bytes()).unwrap();
        };

        for (id, label, remote) in [(1u64, "remote", true), (2, "local", false)] {
            println!("--- {label}");
            let path = format!("{work}/{label}.log");
            let numbered: String = (1..=10).map(|i| format!("line {i}\n")).collect();
            std::fs::write(&path, &numbered).unwrap();
            let target = if remote { TailTarget::Remote(1, session.clone()) } else { TailTarget::Local };
            let started = tails.start(id, target, path.clone(), 3, None).unwrap();
            println!("method: {:?}", started.method);
            check(
                &format!("{label}: last 3 lines"),
                wait(id, &|s| s.lines.len() >= 3) && seen.lock().unwrap()[&id].lines[..3] == ["line 8", "line 9", "line 10"],
            );

            append(&path, "new A\nhalf");
            check(&format!("{label}: appended line"), wait(id, &|s| s.lines.last().map(String::as_str) == Some("new A")));
            append(&path, " done\n");
            check(&format!("{label}: line completed later"), wait(id, &|s| s.lines.last().map(String::as_str) == Some("half done")));

            std::thread::sleep(Duration::from_millis(1500));
            std::fs::write(&path, "after truncate\n").unwrap();
            check(&format!("{label}: truncation"), wait(id, &|s| s.lines.last().map(String::as_str) == Some("after truncate")));

            // Rotation: rename, then a new, bigger file.
            std::thread::sleep(Duration::from_millis(1500));
            std::fs::rename(&path, format!("{path}.1")).unwrap();
            let rotated: String = (1..=60).map(|i| format!("rotated {i:03} padding padding\n")).collect();
            std::fs::write(&path, &rotated).unwrap();
            check(&format!("{label}: rotation"), wait(id, &|s| s.lines.last().map(String::as_str) == Some("rotated 060 padding padding")));
            if started.method == TailMethod::Sftp {
                let seen_now = &seen.lock().unwrap()[&id];
                check(&format!("{label}: rotation noticed"), seen_now.notices.iter().any(|n| n.contains("ruotato")));
                check(&format!("{label}: rotated file read from its start"), seen_now.lines.iter().any(|l| l == "rotated 001 padding padding"));
            }

            // Missing, then back.
            std::thread::sleep(Duration::from_millis(1500));
            std::fs::remove_file(&path).unwrap();
            std::thread::sleep(Duration::from_millis(4000));
            std::fs::write(&path, "back again\n").unwrap();
            check(&format!("{label}: file back"), wait(id, &|s| s.lines.last().map(String::as_str) == Some("back again")));

            tails.stop(id);
            check(&format!("{label}: stopped without error"), wait(id, &|s| matches!(s.ended, Some((_, None)))));
            let size = std::fs::metadata(&path).unwrap().len();
            let mut offset = seen.lock().unwrap()[&id].offset;
            if started.method == TailMethod::Sftp {
                check(&format!("{label}: offset {offset} = file size"), offset == size);
            } else {
                // BSD tail does not report truncations and rotations: the
                // counted offset is only exact without them.
                println!("offset counted {offset}, file size {size}");
                offset = size;
            }

            // Continue from the offset: only what was added meanwhile.
            append(&path, "while stopped\n");
            let resumed = id + 10;
            let target = if remote { TailTarget::Remote(1, session.clone()) } else { TailTarget::Local };
            tails.start(resumed, target, path.clone(), 3, Some(offset)).unwrap();
            check(
                &format!("{label}: resumed from offset"),
                wait(resumed, &|s| s.lines == ["while stopped"]),
            );
            tails.stop(resumed);
            wait(resumed, &|s| s.ended.is_some());

            // Continue from past the end (the file was replaced meanwhile).
            std::fs::write(&path, "short\n").unwrap();
            let target = if remote { TailTarget::Remote(1, session.clone()) } else { TailTarget::Local };
            tails.start(id + 30, target, path.clone(), 3, Some(offset + 100)).unwrap();
            check(&format!("{label}: resume past the end restarts"), wait(id + 30, &|s| s.lines == ["short"]));
            tails.stop(id + 30);
            wait(id + 30, &|s| s.ended.is_some());

            // A binary file is refused.
            let bin = format!("{work}/{label}.bin");
            std::fs::write(&bin, b"\x00\x01\x02binary\n").unwrap();
            let target = if remote { TailTarget::Remote(1, session.clone()) } else { TailTarget::Local };
            tails.start(id + 20, target, bin, 3, None).unwrap();
            check(
                &format!("{label}: binary refused"),
                wait(id + 20, &|s| matches!(&s.ended, Some((_, Some(e))) if e.contains("binario"))),
            );
            println!("notices: {:?}", seen.lock().unwrap()[&id].notices);
        }

        let session = Arc::try_unwrap(session).ok().expect("session still shared");
        session.close().await;
        println!("{}", if failures == 0 { "ALL PASSED" } else { "SOME CHECKS FAILED" });
        std::process::exit(if failures == 0 { 0 } else { 1 });
    });
}
