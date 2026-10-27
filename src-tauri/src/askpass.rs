//! `SSH_ASKPASS` integration.
//!
//! The application binary doubles as the askpass helper: ssh runs it with
//! `SFM_ASKPASS_SOCKET` set, the helper forwards the prompt over a Unix socket
//! to the running app, which shows a dialog in the UI and sends the answer
//! back. The helper prints it on stdout (or exits 1 when cancelled), which is
//! the contract ssh expects from an askpass program.
//!
//! Each connect registers an *attempt* (`register`): it carries the password
//! saved in the keychain, used once to answer the first password prompt
//! without a dialog, and collects a password typed with "save" checked, which
//! the caller stores only after the connection succeeded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

use crate::error::Result;
use crate::log::{Channel, Logger};

pub const ENV_SOCKET: &str = "SFM_ASKPASS_SOCKET";
pub const ENV_TOKEN: &str = "SFM_ASKPASS_TOKEN";
pub const ENV_ALIAS: &str = "SFM_ASKPASS_ALIAS";
pub const ENV_ATTEMPT: &str = "SFM_ASKPASS_ATTEMPT";

#[derive(Debug, Serialize, Deserialize)]
struct HelperRequest {
    token: String,
    alias: String,
    #[serde(default)]
    attempt: u64,
    prompt: String,
    /// Value of `SSH_ASKPASS_PROMPT`: "confirm", "none" or empty.
    hint: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HelperResponse {
    value: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PromptKind {
    /// Password, passphrase, OTP: hidden text input.
    Secret,
    /// Yes/no confirmation (FIDO touch, agent confirm): exit status only.
    Confirm,
    /// Informational message, no answer expected.
    Notice,
    /// Unknown host key: the user must answer yes/no (or a fingerprint).
    HostKey,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskpassRequest {
    pub id: u64,
    /// Connection name.
    pub alias: String,
    pub prompt: String,
    pub kind: PromptKind,
    /// Password prompt of a connection that saves passwords: the dialog
    /// offers "save in keychain".
    pub can_save: bool,
    /// The saved password was sent and ssh asked again: it is no longer valid.
    pub saved_invalid: bool,
}

/// Options of one connect for the askpass server.
#[derive(Default)]
pub struct AttemptOptions {
    /// Password from the keychain, used for the first password prompt only.
    pub saved_password: Option<String>,
    /// The connection saves passwords (offer "save" in the dialog).
    pub save_enabled: bool,
}

#[derive(Default)]
struct Attempt {
    options: AttemptOptions,
    saved_used: bool,
    to_save: Option<String>,
}

/// Password prompts (not passphrases, OTP codes or confirmations).
fn is_password_prompt(prompt: &str) -> bool {
    let prompt = prompt.to_ascii_lowercase();
    prompt.contains("password") && !prompt.contains("passphrase")
}

/// If the process was launched by ssh as askpass helper, handle the request
/// and return the exit code. Must run before any GUI initialisation.
pub fn run_helper_if_requested() -> Option<i32> {
    let socket = std::env::var_os(ENV_SOCKET)?;
    Some(match helper(Path::new(&socket)) {
        Ok(Some(value)) => {
            println!("{value}");
            0
        }
        Ok(None) => 1,
        Err(e) => {
            eprintln!("askpass helper: {e}");
            1
        }
    })
}

fn helper(socket: &Path) -> std::io::Result<Option<String>> {
    use std::io::{BufRead, Write};

    let request = HelperRequest {
        token: std::env::var(ENV_TOKEN).unwrap_or_default(),
        alias: std::env::var(ENV_ALIAS).unwrap_or_default(),
        attempt: std::env::var(ENV_ATTEMPT).ok().and_then(|v| v.parse().ok()).unwrap_or(0),
        prompt: std::env::args().nth(1).unwrap_or_default(),
        hint: std::env::var("SSH_ASKPASS_PROMPT").unwrap_or_default(),
    };
    let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
    serde_json::to_writer(&mut stream, &request)?;
    stream.write_all(b"\n")?;

    let mut line = String::new();
    std::io::BufReader::new(stream).read_line(&mut line)?;
    let response: HelperResponse = serde_json::from_str(&line)?;
    Ok(response.value)
}

impl PromptKind {
    fn log_name(self) -> &'static str {
        match self {
            PromptKind::Secret => "secret",
            PromptKind::Confirm => "confirm",
            PromptKind::Notice => "notice",
            PromptKind::HostKey => "host-key",
        }
    }
}

fn classify(prompt: &str, hint: &str) -> PromptKind {
    match hint {
        "confirm" => PromptKind::Confirm,
        "none" => PromptKind::Notice,
        _ if prompt.contains("(yes/no") => PromptKind::HostKey,
        _ => PromptKind::Secret,
    }
}

type Notifier = Arc<dyn Fn(AskpassRequest) + Send + Sync>;

/// An open prompt: answer channel, attempt and whether it asks a password.
type Pending = (oneshot::Sender<Option<String>>, u64, bool);

pub struct AskpassServer {
    socket_path: PathBuf,
    token: String,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, Pending>>,
    attempts: Mutex<HashMap<u64, Attempt>>,
    notify: Notifier,
    logger: Arc<Logger>,
}

impl AskpassServer {
    /// Binds the socket and starts accepting helper connections.
    /// `notify` is called for every prompt that must be shown to the user.
    pub fn start(runtime_dir: &Path, notify: Notifier, logger: Arc<Logger>) -> Result<Arc<Self>> {
        crate::runtime_dir::remove_stale_files(runtime_dir);
        let socket_path = runtime_dir.join(format!("askpass-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket_path);
        // Bound with std: `start` may run outside the Tokio runtime (Tauri setup).
        let listener = std::os::unix::net::UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        let server = Arc::new(Self {
            socket_path,
            token: crate::runtime_dir::random_token()?,
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            attempts: Mutex::new(HashMap::new()),
            notify,
            logger,
        });

        let accept_server = server.clone();
        tauri::async_runtime::spawn(async move {
            let listener = match UnixListener::from_std(listener) {
                Ok(listener) => listener,
                Err(e) => {
                    eprintln!("askpass listener error: {e}");
                    return;
                }
            };
            while let Ok((stream, _)) = listener.accept().await {
                let server = accept_server.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = server.handle(stream).await {
                        eprintln!("askpass connection error: {e}");
                    }
                });
            }
        });
        Ok(server)
    }

    /// Starts tracking one connect; returns the id to pass to `env_for`.
    pub fn register(&self, options: AttemptOptions) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.attempts.lock().unwrap().insert(
            id,
            Attempt {
                options,
                ..Default::default()
            },
        );
        id
    }

    /// Ends an attempt; returns the password the user asked to save, if any.
    pub fn finish(&self, attempt: u64) -> Option<String> {
        self.attempts.lock().unwrap().remove(&attempt).and_then(|a| a.to_save)
    }

    /// Environment that makes ssh use this server for every prompt.
    /// `label` is the name shown in the dialog; `attempt` comes from `register` (0 = none).
    pub fn env_for(&self, label: &str, attempt: u64) -> Vec<(String, String)> {
        let helper = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        vec![
            ("SSH_ASKPASS".into(), helper),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            (ENV_SOCKET.into(), self.socket_path.to_string_lossy().into_owned()),
            (ENV_TOKEN.into(), self.token.clone()),
            (ENV_ALIAS.into(), label.into()),
            (ENV_ATTEMPT.into(), attempt.to_string()),
        ]
    }

    /// Removes the socket file; called on app exit, when Drop never runs.
    pub fn remove_socket(&self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }

    /// Delivers the user's answer; `None` means cancel. With `save`, a
    /// password is kept for the attempt (see `finish`).
    pub fn respond(&self, id: u64, value: Option<String>, save: bool) {
        let Some((tx, attempt, is_password)) = self.pending.lock().unwrap().remove(&id) else {
            return;
        };
        if let (true, true, Some(value)) = (save, is_password, &value)
            && let Some(attempt) = self.attempts.lock().unwrap().get_mut(&attempt)
            && attempt.options.save_enabled
        {
            attempt.to_save = Some(value.clone());
        }
        let _ = tx.send(value);
    }

    /// Answer from the keychain for the first password prompt of an attempt;
    /// otherwise the dialog flags (can_save, saved_invalid).
    fn saved_answer(&self, attempt: u64, kind: PromptKind, prompt: &str) -> (Option<String>, bool, bool) {
        if kind != PromptKind::Secret || !is_password_prompt(prompt) {
            return (None, false, false);
        }
        let mut attempts = self.attempts.lock().unwrap();
        let Some(attempt) = attempts.get_mut(&attempt) else {
            return (None, false, false);
        };
        let has_saved = attempt.options.saved_password.is_some();
        if has_saved && !attempt.saved_used {
            attempt.saved_used = true;
            return (attempt.options.saved_password.clone(), false, false);
        }
        (None, attempt.options.save_enabled, has_saved)
    }

    async fn handle(&self, stream: UnixStream) -> std::io::Result<()> {
        let (read, mut write) = stream.into_split();
        let mut line = String::new();
        BufReader::new(read).read_line(&mut line).await?;
        let request: HelperRequest = serde_json::from_str(&line)?;

        if request.token != self.token {
            self.logger
                .start(None, Channel::Askpass, "rejected", "richiesta con token non valido")
                .fail("token non valido");
            return write.write_all(b"{\"value\":null}\n").await;
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let kind = classify(&request.prompt, &request.hint);
        // Only the prompt is logged, never the answer.
        let op = self.logger.start(
            Some(&request.alias),
            Channel::Askpass,
            kind.log_name(),
            request.prompt.trim(),
        );
        let (saved, can_save, saved_invalid) = self.saved_answer(request.attempt, kind, &request.prompt);
        let is_password = kind == PromptKind::Secret && is_password_prompt(&request.prompt);
        let attempt = request.attempt;
        let request = AskpassRequest {
            id,
            alias: request.alias,
            kind,
            prompt: request.prompt,
            can_save,
            saved_invalid,
        };
        let value = if let Some(saved) = saved {
            op.ok(Some("risposta dal Portachiavi".into()));
            Some(saved)
        } else if kind == PromptKind::Notice {
            // ssh does not wait for an answer and kills the helper itself.
            (self.notify)(request);
            op.ok(Some("notifica".into()));
            None
        } else {
            let (tx, rx) = oneshot::channel();
            self.pending.lock().unwrap().insert(id, (tx, attempt, is_password));
            (self.notify)(request);
            let value = rx.await.ok().flatten();
            match (&value, kind) {
                (None, _) => op.fail("annullato dall'utente"),
                (Some(v), PromptKind::HostKey) if v == "yes" => op.ok(Some("host key accettata".into())),
                (Some(_), PromptKind::HostKey) => op.fail("host key rifiutata"),
                (Some(_), PromptKind::Confirm) => op.ok(Some("confermato".into())),
                (Some(_), _) => op.ok(Some("risposta fornita".into())),
            }
            value
        };

        let mut payload = serde_json::to_vec(&HelperResponse { value })?;
        payload.push(b'\n');
        write.write_all(&payload).await
    }
}

impl Drop for AskpassServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_prompts() {
        assert_eq!(classify("Enter passphrase for key '/x':", ""), PromptKind::Secret);
        assert_eq!(classify("user@host's password:", ""), PromptKind::Secret);
        assert_eq!(classify("Confirm user presence for key", "confirm"), PromptKind::Confirm);
        assert_eq!(classify("Touch your key", "none"), PromptKind::Notice);
        assert_eq!(
            classify(
                "Are you sure you want to continue connecting (yes/no/[fingerprint])?",
                ""
            ),
            PromptKind::HostKey
        );
    }

    /// Sends one prompt as the helper would and returns the answer.
    async fn ask(server: &AskpassServer, attempt: u64, prompt: &str) -> Option<String> {
        let mut stream = UnixStream::connect(&server.socket_path).await.unwrap();
        let request = HelperRequest {
            token: server.token.clone(),
            alias: "web".into(),
            attempt,
            prompt: prompt.into(),
            hint: String::new(),
        };
        let mut line = serde_json::to_vec(&request).unwrap();
        line.push(b'\n');
        stream.write_all(&line).await.unwrap();
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).await.unwrap();
        serde_json::from_str::<HelperResponse>(&answer).unwrap().value
    }

    #[test]
    fn answers_once_from_keychain_then_asks_and_keeps_the_new_password() {
        tauri::async_runtime::block_on(async {
            let dir = std::env::temp_dir().join(format!("sfm-askpass-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let shown: Arc<Mutex<Vec<AskpassRequest>>> = Arc::default();
            let sink = shown.clone();
            let slot: Arc<std::sync::OnceLock<Arc<AskpassServer>>> = Arc::default();
            let responder = slot.clone();
            let server = AskpassServer::start(
                &dir,
                Arc::new(move |request| {
                    responder.get().unwrap().respond(request.id, Some("new".into()), true);
                    sink.lock().unwrap().push(request);
                }),
                Logger::new(None, None),
            )
            .unwrap();
            let _ = slot.set(server.clone());
            let attempt = server.register(AttemptOptions {
                saved_password: Some("old".into()),
                save_enabled: true,
            });

            assert_eq!(ask(&server, attempt, "u@h's password: ").await.as_deref(), Some("old"));
            assert!(shown.lock().unwrap().is_empty(), "the saved password needs no dialog");
            assert_eq!(ask(&server, attempt, "u@h's password: ").await.as_deref(), Some("new"));
            let request = shown.lock().unwrap()[0].clone();
            assert!(request.can_save && request.saved_invalid);
            assert_eq!(server.finish(attempt).as_deref(), Some("new"));
            std::fs::remove_dir_all(dir).unwrap();
        });
    }

    #[test]
    fn recognises_password_prompts() {
        assert!(is_password_prompt("deploy@10.0.0.5's password: "));
        assert!(is_password_prompt("(deploy@web) Password:"));
        assert!(!is_password_prompt("Enter passphrase for key '/x':"));
        assert!(!is_password_prompt("Verification code:"));
    }
}
