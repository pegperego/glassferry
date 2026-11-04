//! Windows feasibility check, run inside the VM against the sshd fixture of
//! the Mac (`GF_SSHD_LISTEN=192.168.64.1 scripts/test-sshd.sh start`, client
//! files from `.dev-sshd/vm/` in `~/.ssh/`).
//!
//! Like the app, one GUI-subsystem binary is both the program and the
//! `SSH_ASKPASS` helper: started by ssh with `GF_SPIKE_PIPE` set, it sends
//! the prompt to the running program over a named pipe (with a token) and
//! prints the answer. The report goes to stdout and to `%TEMP%\gf-spike.txt`
//! (a GUI-subsystem program may have no stdout when started from Explorer).
//!
//!   cargo run --release            # then read the report (GF_SPIKE_SFTP=1: SFTP only,
//!                                  # GF_SPIKE_ONLY_STD=1: exec variants only)
//!   (or double-click target\release\gf-windows-spike.exe: no console window
//!   must appear, neither for the program nor for ssh)

#![cfg_attr(not(feature = "console"), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use openssh_sftp_client::{Sftp, SftpOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::process::Command;

const ENV_PIPE: &str = "GF_SPIKE_PIPE";
const ENV_TOKEN: &str = "GF_SPIKE_TOKEN";
/// No console window for console programs started by a GUI program.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PASSPHRASE: &str = "segreta";

fn main() {
    if let Ok(pipe) = std::env::var(ENV_PIPE) {
        std::process::exit(helper(&pipe));
    }
    let report = Report::default();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(run(&report));
    report.save();
}

/// The askpass side: prompt in, answer out (exit 1 = no answer).
fn helper(pipe: &str) -> i32 {
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let token = std::env::var(ENV_TOKEN).unwrap_or_default();
    let mut file = match std::fs::OpenOptions::new().read(true).write(true).open(pipe) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("askpass helper: cannot open {pipe}: {e}");
            return 1;
        }
    };
    if writeln!(file, "{token}\t{}", prompt.replace(['\r', '\n'], " ")).is_err() {
        return 1;
    }
    let mut line = String::new();
    if BufReader::new(&file).read_line(&mut line).is_err() {
        return 1;
    }
    let answer = line.trim_end_matches(['\r', '\n']);
    if answer.is_empty() {
        return 1;
    }
    println!("{answer}");
    0
}

#[derive(Default)]
struct Report {
    lines: Mutex<Vec<String>>,
}

impl Report {
    /// Printed and appended to the report file at once: a hang later on
    /// must not lose what was found so far.
    fn line(&self, text: impl Into<String>) {
        let text = text.into();
        println!("{text}");
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(Self::path()) {
            let _ = write!(file, "{text}\r\n");
        }
        self.lines.lock().unwrap().push(text);
    }

    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join("gf-spike.txt")
    }

    fn check(&self, label: &str, ok: bool, detail: impl AsRef<str>) {
        self.line(format!("{} {label}: {}", if ok { "PASS" } else { "FAIL" }, detail.as_ref()));
    }

    fn save(&self) {}
}

/// Answers the helpers: the passphrase for secrets, "yes" for confirmations.
async fn serve(name: String, token: String, prompts: Arc<Mutex<Vec<String>>>) -> std::io::Result<()> {
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(&name)?;
    loop {
        server.connect().await?;
        let connected = server;
        server = ServerOptions::new().reject_remote_clients(true).create(&name)?;
        let token = token.clone();
        let prompts = prompts.clone();
        tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(connected);
            let mut line = String::new();
            if AsyncBufReader::new(read).read_line(&mut line).await.is_err() {
                return;
            }
            let (got_token, prompt) = line.trim_end().split_once('\t').unwrap_or(("", ""));
            let answer = if got_token != token {
                prompts.lock().unwrap().push(format!("token sbagliato, rifiutato: {prompt}"));
                ""
            } else if prompt.contains("yes/no") {
                "yes"
            } else if prompt.to_lowercase().contains("passphrase") || prompt.to_lowercase().contains("password") {
                PASSPHRASE
            } else {
                ""
            };
            if got_token == token {
                prompts.lock().unwrap().push(prompt.to_string());
            }
            let _ = write.write_all(format!("{answer}\n").as_bytes()).await;
        });
    }
}

fn ssh(args: &[&str], env: &[(String, String)]) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true);
    cmd
}

async fn run(report: &Report) {
    let exe = std::env::current_exe().expect("current exe");
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let pipe = format!(r"\\.\pipe\gf-spike-{}", std::process::id());
    let token = format!("{nonce:x}");
    let prompts: Arc<Mutex<Vec<String>>> = Arc::default();
    report.line(format!("gf-windows-spike, helper = {}", exe.display()));
    match Command::new("ssh").arg("-V").creation_flags(CREATE_NO_WINDOW).output().await {
        Ok(out) => report.line(format!("ssh -V: {}", String::from_utf8_lossy(&out.stderr).trim())),
        Err(e) => report.line(format!("ssh -V: {e}")),
    }

    tokio::spawn(serve(pipe.clone(), token.clone(), prompts.clone()));
    let env = vec![
        ("SSH_ASKPASS".to_string(), exe.to_string_lossy().into_owned()),
        ("SSH_ASKPASS_REQUIRE".to_string(), "force".to_string()),
        (ENV_PIPE.to_string(), pipe.clone()),
        (ENV_TOKEN.to_string(), token.clone()),
    ];

    let cmd_helper = std::env::var("USERPROFILE").unwrap_or_default() + r"\askpass.cmd";

    // 0. Baseline with std::process (blocking, no tokio pipes), output to files.
    // How stdout / stderr are connected: file, std pipe (Rust: named pipe,
    // overlapped on our side) or anonymous pipe (CreatePipe, via os_pipe).
    #[derive(Clone, Copy, Debug)]
    enum Out { File, Std, Anon }
    let only_sftp = std::env::var("GF_SPIKE_SFTP").is_ok();
    for (label, helper, out_mode, err_mode) in if only_sftp { vec![] } else { vec![
        ("std, exe, out file / err file", None, Out::File, Out::File),
        ("std, exe, out pipe std / err file", None, Out::Std, Out::File),
        ("std, exe, out file / err file (2)", None, Out::File, Out::File),
        ("std, exe, out pipe std / err file (2)", None, Out::Std, Out::File),
    ] } {
        let helper: Option<String> = helper;
        let mut env = env.clone();
        if let Some(helper) = helper {
            env[0].1 = helper;
        }
        let _ = &cmd_helper;
        let started = Instant::now();
        let result = tokio::task::spawn_blocking(move || {
            use std::os::windows::process::CommandExt;
            let dir = std::env::temp_dir();
            let mut c = std::process::Command::new("ssh");
            let elog = dir.join("gf-std-e.log");
            let _ = std::fs::remove_file(&elog);
            c.arg("-vv").arg("-E").arg(&elog).args(["-n", "-T", "-o", "ConnectTimeout=8", "--", "gf-local", "echo spike-exec-ok"])
                .envs(env.iter().map(|(k, v)| (k, v)))
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null());
            // Readers drain the pipes on threads, so a full pipe never blocks ssh.
            let mut readers: Vec<std::thread::JoinHandle<String>> = Vec::new();
            let mut anon_out = None;
            match out_mode {
                Out::File => { c.stdout(std::fs::File::create(dir.join("gf-std-out.txt")).unwrap()); }
                Out::Std => { c.stdout(Stdio::piped()); }
                Out::Anon => {
                    let (reader, writer) = os_pipe::pipe().unwrap();
                    c.stdout(writer);
                    anon_out = Some(reader);
                }
            }
            let mut anon_err = None;
            match err_mode {
                Out::File => { c.stderr(std::fs::File::create(dir.join("gf-std-err.txt")).unwrap()); }
                Out::Std => { c.stderr(Stdio::piped()); }
                Out::Anon => {
                    let (reader, writer) = os_pipe::pipe().unwrap();
                    c.stderr(writer);
                    anon_err = Some(reader);
                }
            }
            let mut child = c.spawn().map_err(|e| e.to_string())?;
            // Our copies of the write ends were moved into the Command (dropped with it).
            drop(c);
            let drain = |mut r: Box<dyn std::io::Read + Send>| std::thread::spawn(move || { let mut s = String::new(); let _ = r.read_to_string(&mut s); s });
            let out_reader = match out_mode {
                Out::File => None,
                Out::Std => Some(drain(Box::new(child.stdout.take().unwrap()))),
                Out::Anon => Some(drain(Box::new(anon_out.take().unwrap()))),
            };
            match err_mode {
                Out::File => {}
                Out::Std => readers.push(drain(Box::new(child.stderr.take().unwrap()))),
                Out::Anon => readers.push(drain(Box::new(anon_err.take().unwrap()))),
            }
            let deadline = Instant::now() + std::time::Duration::from_secs(20);
            loop {
                if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                    let out = match out_reader {
                        Some(reader) => reader.join().unwrap_or_default(),
                        None => std::fs::read_to_string(dir.join("gf-std-out.txt")).unwrap_or_default(),
                    };
                    let _ = readers;
                    return Ok((status.success(), out));
                }
                if Instant::now() > deadline {
                    let _ = child.kill();
                    return Err("bloccato oltre 20 s".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        })
        .await
        .unwrap();
        let failed = !matches!(&result, Ok((true, _)));
        match result {
            Ok((ok, out)) => report.check(label, ok && out.contains("spike-exec-ok"), format!("{:?}, {} ms", out.trim(), started.elapsed().as_millis())),
            Err(e) => report.check(label, false, e),
        }
        if failed {
            let debug = std::fs::read_to_string(std::env::temp_dir().join("gf-std-e.log")).unwrap_or_default();
            for line in debug.lines().rev().take(10).collect::<Vec<_>>().into_iter().rev() {
                report.line(format!("    {line}"));
            }
        }
        report.line(format!("  richieste: {:?}", std::mem::take(&mut *prompts.lock().unwrap())));
    }

    // 0b. stdout on a named pipe overlapped on BOTH ends (as sftp.exe does).
    for round in 1..=(if only_sftp { 0 } else { 2 }) {
        let label = format!("std, exe, out pipe sovrapposta da entrambi i lati ({round})");
        let started = Instant::now();
        match overlapped_variant(&env).await {
            Ok(out) => report.check(&label, out.contains("spike-exec-ok"), format!("{:?}, {} ms", out.trim(), started.elapsed().as_millis())),
            Err(e) => report.check(&label, false, e),
        }
        report.line(format!("  richieste: {:?}", std::mem::take(&mut *prompts.lock().unwrap())));
    }

    if std::env::var("GF_SPIKE_ONLY_STD").is_ok() {
        report.line("fine (solo varianti std)");
        return;
    }
    let skip_exec = std::env::var("GF_SPIKE_SFTP").is_ok();

    // 1. A command over a new connection: askpass through the named pipe.
    // Variants, to find what blocks: console flags, stdin, helper kind.
    for (label, no_window, null_stdin, helper) in if skip_exec { vec![] } else { vec![
        ("exe, CREATE_NO_WINDOW, stdin null", true, true, None),
        ("exe, CREATE_NO_WINDOW, stdin pipe", true, false, None),
        ("exe, console ereditata, stdin null", false, true, None),
        (".cmd, CREATE_NO_WINDOW, stdin null", true, true, Some(cmd_helper.clone())),
    ] } {
        let mut env = env.clone();
        if let Some(helper) = &helper {
            env[0].1 = helper.clone();
        }
        let started = Instant::now();
        let mut cmd = ssh(&["-v", "-o", "ConnectTimeout=8", "--", "gf-local", "echo spike-exec-ok"], &env);
        if !no_window {
            cmd.creation_flags(0);
        }
        let log = std::env::temp_dir().join("gf-spike-ssh.log");
        let log_file = std::fs::File::create(&log).expect("ssh log");
        cmd.stdin(if null_stdin { Stdio::null() } else { Stdio::piped() })
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log_file));
        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                report.check(label, false, e.to_string());
                continue;
            }
        };
        let out = tokio::time::timeout(std::time::Duration::from_secs(20), child.wait_with_output()).await;
        match out {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                report.check(
                    label,
                    out.status.success() && stdout.contains("spike-exec-ok"),
                    format!("{:?}, {} ms", stdout.trim(), started.elapsed().as_millis()),
                );
                let _ = stderr;
            }
            Ok(Err(e)) => report.check(label, false, e.to_string()),
            Err(_) => report.check(label, false, "bloccato oltre 20 s (ssh terminato)"),
        }
        let debug = std::fs::read_to_string(&log).unwrap_or_default();
        for line in debug.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
            report.line(format!("    {line}"));
        }
        report.line(format!("  richieste: {:?}", std::mem::take(&mut *prompts.lock().unwrap())));
    }

    // 2. SFTP over ssh's stdin/stdout, on the SFTP-only account.
    let started = Instant::now();
    // stdin and stdout on pipes overlapped on both ends (see step 0b).
    let sftp_log = std::env::temp_dir().join("gf-spike-sftp.log");
    let (stdout, child_out) = overlapped_pipe(true).expect("stdout pipe");
    let (stdin, child_in) = overlapped_pipe(false).expect("stdin pipe");
    let mut child = {
        use std::os::windows::process::CommandExt;
        // Variants to compare with the app: host, the app's channel options,
        // stderr on a pipe instead of a file.
        let host = std::env::var("GF_SPIKE_HOST").unwrap_or_else(|_| "gf-sftponly".into());
        let mut c = std::process::Command::new("ssh");
        if std::env::var("GF_SPIKE_APPARGS").is_ok() {
            c.args(["-o", "RemoteCommand=none", "-T"]);
        }
        c.args(["-s", "--", &host, "sftp"])
            .envs(env.iter().map(|(k, v)| (k, v)))
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::from(child_in))
            .stdout(Stdio::from(child_out));
        let stderr_pipe = std::env::var("GF_SPIKE_STDERR_PIPE").is_ok();
        if stderr_pipe {
            c.stderr(Stdio::piped());
        } else {
            c.stderr(Stdio::from(std::fs::File::create(&sftp_log).expect("sftp log")));
        }
        report.line(format!("  varianti: host {host}, opzioni app {}, stderr su pipe {stderr_pipe}, tokio {}", std::env::var("GF_SPIKE_APPARGS").is_ok(), std::env::var("GF_SPIKE_TOKIO").is_ok()));
        if std::env::var("GF_SPIKE_TOKIO").is_ok() {
            let mut t = tokio::process::Command::from(c);
            t.kill_on_drop(true);
            let spawned = t.spawn();
            drop(t);
            match spawned {
                Ok(child) => {
                    // Keep it alive for the test; tokio's Child is not a std Child.
                    std::mem::forget(child);
                }
                Err(e) => {
                    report.check("avvio di ssh -s sftp (tokio)", false, e.to_string());
                    return;
                }
            }
            let opened = tokio::time::timeout(std::time::Duration::from_secs(20), Sftp::new(stdin, stdout, SftpOptions::default())).await;
            report.check("SFTP su stdin/stdout (tokio)", matches!(opened, Ok(Ok(_))), match &opened { Ok(Ok(_)) => "ok".to_string(), Ok(Err(e)) => e.to_string(), Err(_) => "bloccato oltre 20 s".to_string() });
            report.line(format!("  richieste: {:?}", std::mem::take(&mut *prompts.lock().unwrap())));
            return;
        }
        match c.spawn() {
            Ok(mut child) => {
                if let Some(mut err) = child.stderr.take() {
                    std::thread::spawn(move || { let mut sink = Vec::new(); let _ = std::io::Read::read_to_end(&mut err, &mut sink); });
                }
                child
            }
            Err(e) => {
                report.check("avvio di ssh -s sftp", false, e.to_string());
                return;
            }
        }
    };
    let opened = tokio::time::timeout(std::time::Duration::from_secs(20), Sftp::new(stdin, stdout, SftpOptions::default())).await;
    let sftp = match opened {
        Ok(Ok(sftp)) => sftp,
        Ok(Err(e)) => {
            report.check("SFTP su stdin/stdout", false, e.to_string());
            return;
        }
        Err(_) => {
            report.check("SFTP su stdin/stdout", false, "bloccato oltre 20 s");
            report.line(format!("  richieste: {:?}", std::mem::take(&mut *prompts.lock().unwrap())));
            let debug = std::fs::read_to_string(&sftp_log).unwrap_or_default();
            for line in debug.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
                report.line(format!("    {line}"));
            }
            return;
        }
    };
    report.check("SFTP su stdin/stdout", true, format!("{} ms", started.elapsed().as_millis()));
    report.line(format!("  richieste: {:?}", std::mem::take(&mut *prompts.lock().unwrap())));

    let mut fs = sftp.fs();
    match fs.canonicalize(".").await {
        Ok(home) => report.check("realpath .", true, home.display().to_string()),
        Err(e) => report.check("realpath .", false, e.to_string()),
    }
    match fs.open_dir(".").await {
        Ok(dir) => {
            use futures_util::TryStreamExt;
            let entries: Result<Vec<_>, _> = dir.read_dir().try_collect().await;
            match entries {
                Ok(entries) => report.check("readdir .", true, format!("{} voci", entries.len())),
                Err(e) => report.check("readdir .", false, e.to_string()),
            }
        }
        Err(e) => report.check("readdir .", false, e.to_string()),
    }
    let name = ".gf-windows-spike.txt";
    let data = format!("scritto da Windows {nonce}\n");
    let roundtrip = async {
        fs.write(name, data.as_bytes()).await?;
        let back = fs.read(name).await?;
        fs.remove_file(name).await?;
        Ok::<_, openssh_sftp_client::Error>(back)
    }
    .await;
    match roundtrip {
        Ok(back) => report.check("scrittura, lettura, cancellazione", back[..] == *data.as_bytes(), format!("{} byte", back.len())),
        Err(e) => report.check("scrittura, lettura, cancellazione", false, e.to_string()),
    }
    let started = Instant::now();
    let closed = tokio::time::timeout(std::time::Duration::from_secs(10), sftp.close()).await;
    report.line(format!(
        "  close: {} dopo {} ms",
        match &closed { Ok(Ok(())) => "ok".to_string(), Ok(Err(e)) => format!("errore {e}"), Err(_) => "bloccato".to_string() },
        started.elapsed().as_millis()
    ));
    let mut exited = None;
    for _ in 0..50 {
        if let Some(status) = child.try_wait().ok().flatten() {
            exited = Some(status);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    report.check("ssh termina dopo la chiusura", exited.is_some(), format!("{exited:?} dopo {} ms", started.elapsed().as_millis()));
    let _ = child.kill();
    report.line("fine");
}

/// A pipe whose both ends are overlapped: the parent's (read, wrapped in a
/// tokio named pipe) and the child's (write, inheritable).
fn overlapped_pipe(inbound: bool) -> Result<(tokio::net::windows::named_pipe::NamedPipeServer, std::os::windows::io::OwnedHandle), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND};
    use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};

    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!(r"\\.\pipe\gf-spike-out-{}-{n}", std::process::id());
    let wide: Vec<u16> = std::ffi::OsStr::new(&name).encode_wide().chain(Some(0)).collect();
    unsafe {
        let server = CreateNamedPipeW(
            wide.as_ptr(),
            (if inbound { PIPE_ACCESS_INBOUND } else { PIPE_ACCESS_OUTBOUND }) | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            65536,
            65536,
            0,
            std::ptr::null(),
        );
        if server == INVALID_HANDLE_VALUE {
            return Err(format!("CreateNamedPipeW: {}", std::io::Error::last_os_error()));
        }
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: std::ptr::null_mut(), bInheritHandle: 1 };
        let access = if inbound { GENERIC_WRITE } else { GENERIC_READ };
        let client = CreateFileW(wide.as_ptr(), access, 0, &sa, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, std::ptr::null_mut());
        if client == INVALID_HANDLE_VALUE {
            return Err(format!("CreateFileW: {}", std::io::Error::last_os_error()));
        }
        let server = tokio::net::windows::named_pipe::NamedPipeServer::from_raw_handle(server as RawHandle).map_err(|e| e.to_string())?;
        Ok((server, OwnedHandle::from_raw_handle(client as RawHandle)))
    }
}

async fn overlapped_variant(env: &[(String, String)]) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    use tokio::io::AsyncReadExt;
    let (mut read, write) = overlapped_pipe(true)?;
    let mut c = std::process::Command::new("ssh");
    c.args(["-n", "-T", "-o", "ConnectTimeout=8", "--", "gf-local", "echo spike-exec-ok"])
        .envs(env.iter().map(|(k, v)| (k, v)))
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::from(write))
        .stderr(std::fs::File::create(std::env::temp_dir().join("gf-ovl-err.txt")).map_err(|e| e.to_string())?);
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    drop(c); // our copy of the child's end
    let mut out = Vec::new();
    let read_all = tokio::time::timeout(std::time::Duration::from_secs(20), read.read_to_end(&mut out)).await;
    let status = child.try_wait().ok().flatten();
    if status.is_none() {
        let _ = child.kill();
    }
    match read_all {
        Ok(_) => Ok(String::from_utf8_lossy(&out).into_owned()),
        Err(_) => Err(format!("bloccato oltre 20 s, letto finora {:?}", String::from_utf8_lossy(&out))),
    }
}
