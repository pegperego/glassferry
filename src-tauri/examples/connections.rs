//! Headless check of connections configured in the app: generated ssh_config,
//! `-F`, ProxyJump between two app connections, `ssh -G` validation and the
//! keychain round trip (one temporary item, removed at the end).
//!
//! scripts/test-sshd.sh start
//! cargo run --example connections -- "$PWD/../.dev-sshd" segreta <work-dir>

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use glassferry_lib::askpass::{AskpassServer, PromptKind};
use glassferry_lib::connections::{self, keychain, Connection, ConnectionStore, Source, Store};
use glassferry_lib::log::Logger;
use glassferry_lib::runtime_dir;
use glassferry_lib::session::{Session, SshLauncher};

const SSH_BIN: &str = "/usr/bin/ssh";

fn main() {
    if let Some(code) = glassferry_lib::askpass::run_helper_if_requested() {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().collect();
    let [_, fixture, secret, work] = args.as_slice() else {
        eprintln!("usage: connections <fixture-dir> <secret> <work-dir>");
        std::process::exit(2);
    };
    let (fixture, secret, work) = (PathBuf::from(fixture), secret.clone(), PathBuf::from(work));
    let mut failures = 0;
    let mut check = |label: &str, ok: bool| {
        println!("{} {label}", if ok { "PASS" } else { "FAIL" });
        failures += usize::from(!ok);
    };

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

        let fixture_source = |proxy_jump: Option<String>| Source::App {
            host: "127.0.0.1".into(),
            user: Some(whoami()),
            port: Some(2222),
            identity_file: Some(fixture.join("userkey").to_string_lossy().into_owned()),
            proxy_jump,
            extra_options: vec![
                "IdentitiesOnly yes".into(),
                "IdentityAgent none".into(),
                format!("UserKnownHostsFile {}", fixture.join("known_hosts").display()),
                "StrictHostKeyChecking ask".into(),
            ],
        };
        let mut store = Store::default();
        for (id, name, jump) in [("direct", "Diretta", None), ("jumped", "Via jump", Some(connections::app_alias("direct")))] {
            store.connections.push(Connection {
                id: id.into(),
                name: name.into(),
                category_id: "test".into(),
                source: fixture_source(jump),
                remote_dir: None,
                local_dir: None,
                save_password: false,
            });
        }
        let _ = std::fs::remove_dir_all(&work);
        let connection_store = ConnectionStore::new(work.clone());
        connection_store.save(&store).unwrap();
        let generated = std::fs::read_to_string(connection_store.ssh_config_path()).unwrap();
        check("generated ssh_config has both hosts", generated.contains("Host gf-direct\n") && generated.contains("ProxyJump gf-direct\n"));

        for connection in &store.connections {
            let target = connection_store.target(connection);
            let launcher = SshLauncher {
                ssh_bin: PathBuf::from(SSH_BIN),
                alias: target.alias,
                config_file: target.config_file,
                label: connection.name.clone(),
                control_path: dir.join(format!("conn-{}.sock", connection.id)),
                log_path: dir.join(format!("conn-{}.log", connection.id)),
                env: askpass.env_for(&connection.name, 0),
                verbose: false,
                logger: logger.clone(),
            };
            match Session::connect(launcher).await {
                Ok(session) => {
                    let listed = session.list_dir(&session.home).await.is_ok();
                    check(&format!("connect + list via -F: {}", connection.name), listed);
                    session.close().await;
                }
                Err(e) => check(&format!("connect via -F: {} ({e})", connection.name), false),
            }
        }

        let resolved = connections::check_source(SSH_BIN.as_ref(), &dir, &fixture_source(None)).await;
        check(
            "ssh -G of an unsaved source",
            resolved.is_ok_and(|m| m.get("port").map(String::as_str) == Some("2222")),
        );
        let mut bad = fixture_source(None);
        if let Source::App { extra_options, .. } = &mut bad {
            extra_options.push("NoSuchOption yes".into());
        }
        let error = connections::check_source(SSH_BIN.as_ref(), &dir, &bad).await;
        if let Err(e) = &error {
            println!("     ssh -G: {e}");
        }
        check("ssh -G reports a bad option", error.is_err_and(|e| e.to_string().to_lowercase().contains("nosuchoption")));
        if let Source::App { extra_options, .. } = &mut bad {
            extra_options.push("Include /etc/passwd".into());
        }
        check("Include is refused", connections::validate_source(&bad).is_err());

        let id = format!("selftest-{}", std::process::id());
        let round_trip = keychain::set(&id, "p4ss").is_ok()
            && keychain::get(&id).ok().flatten().as_deref() == Some("p4ss")
            && keychain::delete(&id).is_ok()
            && keychain::get(&id).is_ok_and(|p| p.is_none());
        check("keychain set/get/delete", round_trip);
        let _ = std::fs::remove_dir_all(&work);
    });
    std::process::exit(i32::from(failures > 0));
}

fn whoami() -> String {
    String::from_utf8(std::process::Command::new("id").arg("-un").output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_string()
}
