//! Headless check of remote file operations against the sshd fixture.
//!
//! cargo run --example fileops -- "$PWD/../.dev-sshd/ssh" gf-local segreta <remote-work-dir>
//! (run it again with gf-sftponly to exercise the SFTP-only fallbacks)

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use glassferry_lib::archive;
use glassferry_lib::askpass::{AskpassServer, PromptKind};
use glassferry_lib::attrs::{self, AttrChange};
use glassferry_lib::dirsize;
use glassferry_lib::search::{self, Cancel, SearchKind, SearchQuery};
use glassferry_lib::fileops::{remote, Target};
use glassferry_lib::plan::{PlanMethod, PlanRequest, Planner};
use glassferry_lib::log::{Logger, Status};
use glassferry_lib::runtime_dir;
use glassferry_lib::session::{Session, SshLauncher};

fn main() {
    if let Some(code) = glassferry_lib::askpass::run_helper_if_requested() {
        std::process::exit(code);
    }
    let args: Vec<String> = std::env::args().collect();
    let [_, ssh_bin, alias, secret, work] = args.as_slice() else {
        eprintln!("usage: fileops <ssh-bin> <alias> <secret> <remote-work-dir>");
        std::process::exit(2);
    };
    let (alias, secret, work) = (alias.clone(), secret.clone(), work.clone());

    tauri::async_runtime::block_on(async move {
        let dir = runtime_dir::create().unwrap();
        let logger = Logger::new(
            Some(Arc::new(|batch| {
                for e in batch.iter().filter(|e| e.status != Status::Pending) {
                    println!("  [log] {:?} {} {:?} {} {}", e.channel, e.op, e.status, e.detail, e.error.clone().unwrap_or_default());
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
        let session = Session::connect(SshLauncher {
            ssh_bin: PathBuf::from(ssh_bin),
            alias: alias.clone(),
            config_file: None,
            label: alias.clone(),
            control_path: dir.join("fileops.sock"),
            log_path: dir.join("fileops.log"),
            env: askpass.env_for(&alias, 0),
            verbose: false,
            logger: logger.clone(),
        })
        .await
        .expect("connect");
        println!("shell commands: {:?}", session.capabilities.commands);

        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(format!("{work}/tree/deep/deeper")).unwrap();
        std::fs::write(format!("{work}/tree/deep/deeper/f.txt"), b"x").unwrap();
        std::fs::write(format!("{work}/tree/a.txt"), b"a").unwrap();
        std::fs::write(format!("{work}/b.txt"), b"b").unwrap();
        std::os::unix::fs::symlink("/etc", format!("{work}/tree/link-to-etc")).unwrap();

        let check = |label: &str, ok: bool| println!("{} {label}", if ok { "PASS" } else { "FAIL" });
        let exists = |p: &str| std::fs::symlink_metadata(format!("{work}/{p}")).is_ok();

        check("mkdir", remote::mkdir(&session, &work, "nuova cartella").await.is_ok() && exists("nuova cartella"));
        check("mkdir existing fails", remote::mkdir(&session, &work, "nuova cartella").await.is_err());
        check("rename onto existing refused", remote::rename(&session, &work, "b.txt", "tree").await.is_err() && exists("b.txt"));
        check("rename", remote::rename(&session, &work, "b.txt", "c 'quoted'.txt").await.is_ok() && exists("c 'quoted'.txt"));
        check(
            "move into folder",
            remote::move_to(&session, &work, &["c 'quoted'.txt".into()], &format!("{work}/nuova cartella")).await.is_ok()
                && exists("nuova cartella/c 'quoted'.txt"),
        );
        check("move dir into itself refused", remote::move_to(&session, &work, &["tree".into()], &format!("{work}/tree/deep")).await.is_err());
        check("invalid name refused", remote::rename(&session, &work, "tree", "../x").await.is_err());
        let method = remote::delete(&session, &work, &["tree".into(), "nuova cartella".into()]).await;
        println!("delete method: {method:?}");
        check("recursive delete", method.is_ok() && !exists("tree") && !exists("nuova cartella"));
        check("symlink target untouched", std::fs::metadata("/etc/hosts").is_ok());

        // Previewed plans: the preview lists what runs, and only that runs.
        std::fs::create_dir_all(format!("{work}/plan/sub")).unwrap();
        std::fs::write(format!("{work}/plan/sub/one.txt"), b"1").unwrap();
        std::fs::write(format!("{work}/plan/two.txt"), b"2").unwrap();
        let planner = Planner::new();
        let target = Target::Remote { session_id: 1 };
        let delete = PlanRequest::Delete { dir: work.clone(), names: vec!["plan".into()] };
        let preview = planner.plan(&logger, target, Some(&session), delete.clone()).await.unwrap();
        println!("plan: {:?} {} lines: {:?}", preview.method, preview.total, preview.lines);
        let expected = if session.capabilities.commands.iter().any(|c| c == "rm") {
            preview.method == PlanMethod::Shell && preview.lines[0].contains("rm -rf --")
        } else {
            // two files + two directories, deepest first
            preview.method == PlanMethod::Sftp && preview.total == 4 && preview.lines.last().unwrap().contains("rmdir")
        };
        check("delete plan matches the account", expected);
        if preview.method == PlanMethod::Sftp {
            // A file appearing after the preview must not be deleted unseen.
            std::fs::write(format!("{work}/plan/sub/late.txt"), b"late").unwrap();
            let result = planner.execute(&logger, preview.id, Some(&session)).await;
            check("sftp plan refuses unseen files", result.is_err() && exists("plan/sub/late.txt"));
            std::fs::remove_file(format!("{work}/plan/sub/late.txt")).unwrap();
            let preview = planner.plan(&logger, target, Some(&session), delete).await.unwrap();
            check("re-planned delete runs", planner.execute(&logger, preview.id, Some(&session)).await.is_ok() && !exists("plan"));
        } else {
            check("shell plan runs", planner.execute(&logger, preview.id, Some(&session)).await.is_ok() && !exists("plan"));
        }
        std::fs::write(format!("{work}/r.txt"), b"r").unwrap();
        let rename = PlanRequest::Rename { dir: work.clone(), from: "r.txt".into(), to: "s.txt".into() };
        let preview = planner.plan(&logger, target, Some(&session), rename).await.unwrap();
        check(
            "rename plan runs once",
            preview.lines[0].starts_with("sftp> rename")
                && planner.execute(&logger, preview.id, Some(&session)).await.is_ok()
                && exists("s.txt")
                && planner.execute(&logger, preview.id, Some(&session)).await.is_err(),
        );

        // chmod: shell `chmod -R` or an SFTP walk, same result; links not followed.
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(format!("{work}/perm/sub")).unwrap();
        std::fs::write(format!("{work}/perm/sub/f.txt"), b"f").unwrap();
        std::fs::write(format!("{work}/outside.txt"), b"o").unwrap();
        std::fs::set_permissions(format!("{work}/outside.txt"), std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(format!("{work}/outside.txt"), format!("{work}/perm/link")).unwrap();
        let mode = |p: &str| std::fs::metadata(format!("{work}/{p}")).unwrap().permissions().mode() & 0o7777;
        let chmod = AttrChange { mode: Some(0o750), owner: None, group: None, recursive: true };
        let lines = attrs::remote::commands(&session, &work, &["perm".into()], &chmod).unwrap();
        let by_shell = session.capabilities.commands.iter().any(|c| c == "chmod");
        check(
            "chmod preview matches the account",
            if by_shell { lines[0].contains("chmod -R -- 0750") } else { lines[0].starts_with("sftp> chmod 750") },
        );
        let applied = attrs::remote::apply(&session, &work, &["perm".into()], &chmod).await;
        check(
            "recursive chmod",
            applied.is_ok() && (mode("perm"), mode("perm/sub"), mode("perm/sub/f.txt")) == (0o750, 0o750, 0o750),
        );
        check("chmod does not follow links", mode("outside.txt") == 0o600);
        let chgrp = AttrChange { mode: None, owner: None, group: Some("staff".into()), recursive: false };
        check(
            "group change needs the shell",
            attrs::remote::commands(&session, &work, &["perm".into()], &chgrp).is_ok()
                == session.capabilities.commands.iter().any(|c| c == "chgrp"),
        );

        // Folder size: du on the server (BSD du: disk usage, approximate) or a walk.
        std::fs::create_dir_all(format!("{work}/sized/sub")).unwrap();
        std::fs::write(format!("{work}/sized/a.bin"), vec![1u8; 300_000]).unwrap();
        std::fs::write(format!("{work}/sized/sub/b.bin"), vec![2u8; 200_000]).unwrap();
        std::os::unix::fs::symlink("/usr", format!("{work}/sized/link")).unwrap();
        let sizes = dirsize::remote::sizes(&session, &work, &["sized".into()]).await.unwrap();
        let size = &sizes[0];
        println!("folder size: {size:?}");
        check(
            "folder size",
            if size.approx { (500_000..700_000).contains(&size.bytes) } else { size.bytes == 500_000 } && !size.partial,
        );

        // Search: find on the server, or an SFTP walk; same results.
        std::fs::create_dir_all(format!("{work}/found/conf.d/nginx")).unwrap();
        std::fs::write(format!("{work}/found/conf.d/Nginx.conf"), b"x").unwrap();
        std::fs::write(format!("{work}/found/notes.txt"), b"x").unwrap();
        let query = SearchQuery { text: "nginx".into(), kind: SearchKind::Any };
        let found = search::remote::search(&session, &format!("{work}/found"), &query, &Cancel::default()).await.unwrap();
        let mut paths: Vec<String> = found.matches.iter().map(|m| format!("{}{}", m.path.replace(&work, ""), if m.dir { "/" } else { "" })).collect();
        paths.sort();
        println!("search via {}: {paths:?}", found.method);
        check(
            "search by name",
            paths == ["/found/conf.d/Nginx.conf", "/found/conf.d/nginx/"]
                && found.method == if session.capabilities.commands.iter().any(|c| c == "find") { "find" } else { "sftp" },
        );
        let files = SearchQuery { text: "*.conf".into(), kind: SearchKind::File };
        let found = search::remote::search(&session, &format!("{work}/found"), &files, &Cancel::default()).await.unwrap();
        check("search files only", found.matches.len() == 1 && !found.matches[0].dir);

        // Archives: tar on the server; SFTP-only accounts refuse with a clear message.
        std::fs::create_dir_all(format!("{work}/pack/site/sub")).unwrap();
        std::fs::write(format!("{work}/pack/site/sub/a.txt"), b"alpha").unwrap();
        let pack = format!("{work}/pack");
        let compressed = archive::remote::compress(&session, &pack, &["site".into()], "backup.tar.gz").await;
        if session.capabilities.commands.iter().any(|c| c == "tar") {
            let extracted = archive::remote::extract(&session, &pack, "backup.tar.gz").await;
            check(
                "compress and extract on the server",
                compressed.is_ok()
                    && extracted.as_ref().is_ok_and(|f| f == "backup")
                    && std::fs::read(format!("{pack}/backup/site/sub/a.txt")).unwrap() == b"alpha",
            );
            // Extracting again: the folder exists now.
            check("extract never overwrites", archive::remote::extract(&session, &pack, "backup.tar.gz").await.is_err());
        } else {
            check("archives need tar", compressed.is_err_and(|e| e.to_string().contains("tar")));
        }

        session.close().await;
        let _ = std::fs::remove_dir_all(&work);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    });
}
