//! Headless spike: connect through askpass, probe, list home, optionally
//! measure download throughput.
//!
//! cargo run --example spike -- <ssh-bin> <alias> <secret-answer> [remote-file]

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use sshfilemanager_lib::askpass::{AskpassServer, PromptKind};
use sshfilemanager_lib::log::Logger;
use sshfilemanager_lib::runtime_dir;
use sshfilemanager_lib::session::{Session, SshLauncher};
use tokio::io::AsyncReadExt;

fn main() {
    if let Some(code) = sshfilemanager_lib::askpass::run_helper_if_requested() {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().collect();
    let [_, ssh_bin, alias, secret, rest @ ..] = args.as_slice() else {
        eprintln!("usage: spike <ssh-bin> <alias> <secret-answer> [remote-file]");
        std::process::exit(2);
    };
    let bench_file = rest.first().cloned();
    let secret = secret.clone();

    tauri::async_runtime::block_on(async move {
        let dir = runtime_dir::create().unwrap();
        let logger = Logger::new(
            Some(Arc::new(|batch| {
                for e in batch.iter().filter(|e| e.status != sshfilemanager_lib::log::Status::Pending) {
                    println!("[log] {:?} {} {:?} {}ms {}", e.channel, e.op, e.status, e.duration_ms.unwrap_or(0), e.detail);
                }
            })),
            None,
        );
        let server: Arc<OnceLock<Arc<AskpassServer>>> = Arc::new(OnceLock::new());
        let responder = server.clone();
        let askpass = AskpassServer::start(
            &dir,
            Arc::new(move |req| {
                println!("[askpass] {:?} for {}: {}", req.kind, req.alias, req.prompt.trim());
                let answer = match req.kind {
                    PromptKind::Secret => Some(secret.clone()),
                    PromptKind::HostKey => Some("yes".into()),
                    PromptKind::Confirm => Some(String::new()),
                    PromptKind::Notice => None,
                };
                if req.kind != PromptKind::Notice {
                    responder.get().unwrap().respond(req.id, answer);
                }
            }),
            logger.clone(),
        )
        .unwrap();
        let _ = server.set(askpass.clone());

        let launcher = SshLauncher {
            ssh_bin: PathBuf::from(ssh_bin),
            alias: alias.clone(),
            control_path: dir.join("spike.sock"),
            log_path: dir.join("spike.log"),
            env: askpass.env_for(alias),
            verbose: std::env::var_os("SPIKE_VERBOSE").is_some(),
            logger: logger.clone(),
        };

        let t = Instant::now();
        let session = match Session::connect(launcher).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("connect failed: {e}");
                std::process::exit(1);
            }
        };
        println!("connected in {:?}, home = {}", t.elapsed(), session.home.display());
        println!("capabilities = {:?}", session.capabilities);

        let t = Instant::now();
        let entries = session.list_dir(&session.home).await.unwrap();
        println!("listed {} entries in {:?}", entries.len(), t.elapsed());
        for e in entries.iter().take(5) {
            println!("  {:?}", e);
        }

        if let Some(path) = bench_file {
            let t = Instant::now();
            let file = session.sftp.open(&path).await.unwrap();
            let mut file = Box::pin(openssh_sftp_client::file::TokioCompatFile::with_capacity(
                file,
                std::num::NonZeroUsize::new(256 * 1024).unwrap(),
            ));
            let mut buf = vec![0u8; 1 << 20];
            let mut total = 0u64;
            loop {
                let n = file.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                total += n as u64;
            }
            drop(file);
            let secs = t.elapsed().as_secs_f64();
            println!(
                "downloaded {} MiB in {:.2}s = {:.1} MiB/s",
                total >> 20,
                secs,
                total as f64 / secs / 1048576.0
            );
        }

        session.close().await;
        println!("closed");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });
}
