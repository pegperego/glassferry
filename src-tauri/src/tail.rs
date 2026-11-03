//! Following a file as it grows (`tail -F`), remote or local.
//!
//! Two paths, like search and folder sizes:
//! - **tail**: with a shell and `tail` on the server (and locally, with the
//!   system's), a one-shot command reads the size and the last lines before
//!   it, then `tail -c +<size+1> -F` streams everything after: no line lost or
//!   repeated, rotation recognized by inode, truncation reported by tail.
//! - **sftp**: on SFTP-only accounts, the last lines read in blocks from the
//!   end, then the size polled and only the new bytes read, with an adaptive
//!   interval. Imitates `tail -F`: a smaller size is a truncation, a changed
//!   first block is a rotation (SFTP v3 has no inode), a missing file is
//!   waited for.
//!
//! Both emit the same `TailEvent`s. Both count the file offset, so a tail
//! that stopped (dropped connection) restarts from where it was.
//!
//! Log: the tail path is one entry with the exact command, finished when it
//! stops. The SFTP path is an exception to "every request in the log" (a poll
//! a second would flood it): start, stop with a summary, notices and errors.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::BytesMut;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeek, BufReader};
use tokio::process::{Child, Command};

use crate::error::{AppError, Result};
use crate::log::{shell_line, Channel, LogOp, Logger};
use crate::search::Cancel;
use crate::session::{sh_command, Session};

/// Lines longer than this are cut (a minified file or a binary blob).
const MAX_LINE_BYTES: usize = 16 * 1024;
/// At most this much is read for the initial lines.
const MAX_INITIAL_BYTES: u64 = 1024 * 1024;
/// Lines are sent to the UI at most this often.
const BATCH_INTERVAL: Duration = Duration::from_millis(100);
/// SFTP polling: fast while the file grows, slower while it is idle.
const POLL_MIN: Duration = Duration::from_millis(500);
const POLL_MAX: Duration = Duration::from_secs(3);
/// SFTP: largest read per poll, so a burst does not stall the UI.
const READ_CHUNK: u64 = 256 * 1024;
/// SFTP: at most this many first bytes are compared to detect a rotation.
const HEAD_BYTES: u64 = 512;
/// Test hook: `GF_TAIL_STRATEGY=sftp` uses the SFTP path even with a shell.
const STRATEGY_ENV: &str = "GF_TAIL_STRATEGY";

/// Size of the file, then its last lines (at most `MAX_INITIAL_BYTES`).
const INITIAL_SCRIPT: &str = r#"s=$(wc -c < "$1") || exit 1
s=$((s + 0))
echo "$s"
head -c "$s" -- "$1" | tail -n "$2" | tail -c 1048576"#;
/// Size of the file, before continuing from an offset.
const SIZE_SCRIPT: &str = r#"wc -c < "$1""#;
/// Everything after byte `$2`, following the file by name.
const STREAM_SCRIPT: &str = r#"exec tail -c +"$(($2 + 1))" -F -- "$1""#;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TailMethod {
    /// `tail -F`, remote through the shell or local.
    Tail,
    /// Polling over SFTP.
    Sftp,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TailEvent {
    /// New complete lines; `offset` is the file position after them.
    Lines { tail_id: u64, lines: Vec<String>, offset: u64 },
    /// Something happened to the file (truncated, rotated, missing…).
    Notice { tail_id: u64, message: String },
    /// The tail stopped: by request (`error` null) or because of an error.
    Ended { tail_id: u64, offset: u64, error: Option<String> },
}

pub type TailSink = Arc<dyn Fn(TailEvent) + Send + Sync>;

/// Splits a byte stream into lines: keeps an incomplete last line, strips
/// `\r`, decodes UTF-8 leniently and cuts very long lines.
#[derive(Default)]
pub struct LineSplitter {
    partial: Vec<u8>,
    /// The current line was already cut: skip until its end.
    skipping: bool,
}

impl LineSplitter {
    pub fn push(&mut self, data: &[u8], out: &mut Vec<String>) {
        let mut rest = data;
        while let Some(pos) = rest.iter().position(|&b| b == b'\n') {
            let (line, tail) = rest.split_at(pos);
            if !self.skipping {
                self.append(line, out);
                if !self.skipping {
                    out.push(decode(&self.partial));
                }
            }
            self.partial.clear();
            self.skipping = false;
            rest = &tail[1..];
        }
        if !self.skipping {
            self.append(rest, out);
        }
    }

    /// Adds to the current line; past the limit, emits it cut.
    fn append(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        let room = MAX_LINE_BYTES.saturating_sub(self.partial.len());
        if bytes.len() <= room {
            self.partial.extend_from_slice(bytes);
            return;
        }
        self.partial.extend_from_slice(&bytes[..room]);
        let mut line = decode(&self.partial);
        line.push_str(" … [riga troncata]");
        out.push(line);
        self.partial.clear();
        self.skipping = true;
    }

    /// Drops the incomplete line (the file was truncated or rotated).
    pub fn reset(&mut self) {
        self.partial.clear();
        self.skipping = false;
    }
}

fn decode(bytes: &[u8]) -> String {
    let line = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    String::from_utf8_lossy(line).into_owned()
}

/// A NUL byte in the first block: not a text file.
pub fn looks_binary(data: &[u8]) -> bool {
    data.iter().take(8192).any(|&b| b == 0)
}

/// Last `count` lines of `data`, the bytes that end a file at `offset`
/// (`at_start`: `data` begins at byte 0, so its first line is complete).
pub fn last_lines(data: &[u8], count: usize, at_start: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let mut splitter = LineSplitter::default();
    let body = if at_start {
        data
    } else {
        // The first line is probably cut: start after its end.
        match data.iter().position(|&b| b == b'\n') {
            Some(pos) => &data[pos + 1..],
            None => data,
        }
    };
    // A last line without `\n` is still being written: shown when complete.
    let complete = match body.iter().rposition(|&b| b == b'\n') {
        Some(pos) => &body[..=pos],
        None => &[][..],
    };
    splitter.push(complete, &mut lines);
    let skip = lines.len().saturating_sub(count);
    lines.split_off(skip)
}

/// Batches lines for the UI.
struct Emitter {
    tail_id: u64,
    sink: TailSink,
    pending: Vec<String>,
    offset: u64,
    last: Instant,
}

impl Emitter {
    fn lines(&mut self, lines: Vec<String>, offset: u64) {
        self.offset = offset;
        self.pending.extend(lines);
        if self.last.elapsed() >= BATCH_INTERVAL {
            self.flush();
        }
    }

    fn flush(&mut self) {
        self.last = Instant::now();
        if self.pending.is_empty() {
            return;
        }
        (self.sink)(TailEvent::Lines {
            tail_id: self.tail_id,
            lines: std::mem::take(&mut self.pending),
            offset: self.offset,
        });
    }

    fn notice(&mut self, message: impl Into<String>) {
        self.flush();
        (self.sink)(TailEvent::Notice { tail_id: self.tail_id, message: message.into() });
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailStarted {
    pub method: TailMethod,
}

/// Where the file is: on a session or on this machine.
pub enum TailTarget {
    Local,
    /// Session id and session.
    Remote(u64, Arc<Session>),
}

/// A running tail: the session it reads from (none for local files) and its cancellation.
struct Running {
    session_id: Option<u64>,
    cancel: Arc<Cancel>,
}

pub struct TailManager {
    logger: Arc<Logger>,
    sink: TailSink,
    running: Mutex<HashMap<u64, Running>>,
}

impl TailManager {
    pub fn new(logger: Arc<Logger>, sink: TailSink) -> Arc<Self> {
        Arc::new(Self { logger, sink, running: Mutex::new(HashMap::new()) })
    }

    /// Starts following `path`: the last `lines` lines, or from byte
    /// `from_offset` when continuing a tail that stopped.
    pub fn start(
        self: &Arc<Self>,
        tail_id: u64,
        target: TailTarget,
        path: String,
        lines: u32,
        from_offset: Option<u64>,
    ) -> Result<TailStarted> {
        if !Path::new(&path).is_absolute() {
            return Err(AppError::Other(format!("percorso non assoluto: {path}")));
        }
        let method = match &target {
            TailTarget::Local => TailMethod::Tail,
            TailTarget::Remote(_, session) => {
                let forced_sftp = std::env::var(STRATEGY_ENV).is_ok_and(|v| v == "sftp");
                let has_tail = session.capabilities.commands.iter().any(|c| c == "tail");
                if has_tail && !forced_sftp { TailMethod::Tail } else { TailMethod::Sftp }
            }
        };
        let session_id = match &target {
            TailTarget::Local => None,
            TailTarget::Remote(id, _) => Some(*id),
        };
        let cancel = Arc::new(Cancel::default());
        self.running.lock().unwrap().insert(tail_id, Running { session_id, cancel: cancel.clone() });
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut emitter = Emitter {
                tail_id,
                sink: manager.sink.clone(),
                pending: Vec::new(),
                offset: from_offset.unwrap_or(0),
                last: Instant::now(),
            };
            let request = Request { path: &path, lines, from_offset };
            let result = match (&target, method) {
                (TailTarget::Remote(_, session), TailMethod::Sftp) => {
                    sftp_tail(session, &request, &cancel, &mut emitter).await
                }
                (TailTarget::Remote(_, session), TailMethod::Tail) => {
                    shell_tail(&Runner::Remote(session), &manager.logger, &request, &cancel, &mut emitter).await
                }
                (TailTarget::Local, _) => {
                    shell_tail(&Runner::Local, &manager.logger, &request, &cancel, &mut emitter).await
                }
            };
            emitter.flush();
            manager.running.lock().unwrap().remove(&tail_id);
            (manager.sink)(TailEvent::Ended {
                tail_id,
                offset: emitter.offset,
                error: match result {
                    Ok(()) | Err(AppError::Cancelled) => None,
                    Err(e) => Some(e.to_string()),
                },
            });
        });
        Ok(TailStarted { method })
    }

    pub fn stop(&self, tail_id: u64) {
        if let Some(running) = self.running.lock().unwrap().get(&tail_id) {
            running.cancel.cancel();
        }
    }

    /// The session is closing: its tails end.
    pub fn stop_session(&self, session_id: u64) {
        for running in self.running.lock().unwrap().values() {
            if running.session_id == Some(session_id) {
                running.cancel.cancel();
            }
        }
    }
}

struct Request<'a> {
    path: &'a str,
    lines: u32,
    from_offset: Option<u64>,
}

/// Runs the scripts on the server (through the session) or locally.
enum Runner<'a> {
    Remote(&'a Session),
    Local,
}

impl Runner<'_> {
    fn log(&self, logger: &Arc<Logger>, op: &str, detail: &str) -> LogOp {
        match self {
            Runner::Remote(session) => session.launcher.log(Channel::Exec, op, detail),
            Runner::Local => logger.start(None, Channel::Local, op, detail),
        }
    }

    fn local_command(script: &str, args: &[&str]) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.env("LC_ALL", "C").arg("-c").arg(script).arg("sh").args(args).stdin(Stdio::null());
        cmd
    }

    async fn output(&self, logger: &Arc<Logger>, op: &str, script: &str, args: &[&str]) -> Result<std::process::Output> {
        match self {
            Runner::Remote(session) => session.launcher.exec(op, &sh_command(script, args)).await,
            Runner::Local => {
                let mut cmd = Self::local_command(script, args);
                let log = self.log(logger, op, &shell_line(["/bin/sh", "-c", script, "sh"].iter().copied().chain(args.iter().copied())));
                let output = cmd.output().await;
                match &output {
                    Ok(o) if o.status.success() => log.ok(Some("exit 0".into())),
                    Ok(o) => log.fail(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                    Err(e) => log.fail(e),
                }
                Ok(output?)
            }
        }
    }

    fn spawn(&self, logger: &Arc<Logger>, op: &str, script: &str, args: &[&str]) -> Result<(Child, LogOp)> {
        match self {
            Runner::Remote(session) => session.launcher.spawn_exec(op, &sh_command(script, args)),
            Runner::Local => {
                let mut cmd = Self::local_command(script, args);
                cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
                let log = self.log(logger, op, &shell_line(["/bin/sh", "-c", script, "sh"].iter().copied().chain(args.iter().copied())));
                match cmd.spawn() {
                    Ok(child) => Ok((child, log)),
                    Err(e) => {
                        log.fail(&e);
                        Err(e.into())
                    }
                }
            }
        }
    }
}

/// `tail -F` path (remote with a shell, or local).
async fn shell_tail(runner: &Runner<'_>, logger: &Arc<Logger>, request: &Request<'_>, cancel: &Cancel, emitter: &mut Emitter) -> Result<()> {
    let path = request.path;
    let mut offset = match request.from_offset {
        // GNU tail reports truncations and rotations, BSD tail does not: the
        // offset counted may be past the end of a new file, so check first.
        Some(offset) => {
            let output = runner.output(logger, "tail-size", SIZE_SCRIPT, &[path]).await?;
            let size: Option<u64> = String::from_utf8_lossy(&output.stdout).trim().parse().ok();
            match size {
                Some(size) if size < offset => {
                    emitter.notice("— il file è più corto di prima (troncato o ruotato): riparte dall'inizio —");
                    emitter.offset = 0;
                    0
                }
                Some(_) => offset,
                None => return Err(AppError::Other(String::from_utf8_lossy(&output.stderr).trim().to_string())),
            }
        }
        None => {
            let lines = request.lines.to_string();
            let output = runner.output(logger, "tail-start", INITIAL_SCRIPT, &[path, &lines]).await?;
            if !output.status.success() {
                let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(AppError::Other(if message.is_empty() { "lettura del file non riuscita".into() } else { message }));
            }
            let stdout = output.stdout;
            let newline = stdout.iter().position(|&b| b == b'\n').unwrap_or(stdout.len());
            let size: u64 = String::from_utf8_lossy(&stdout[..newline])
                .trim()
                .parse()
                .map_err(|_| AppError::Other("risposta inattesa leggendo la dimensione del file".into()))?;
            let body = stdout.get(newline + 1..).unwrap_or_default();
            if looks_binary(body) {
                return Err(AppError::Other("il file sembra binario: niente da seguire".into()));
            }
            // `tail -c` may cut the first line when the last lines exceed 1 MiB.
            let at_start = (body.len() as u64) < MAX_INITIAL_BYTES;
            emitter.lines(last_lines(body, request.lines as usize, at_start), size);
            emitter.flush();
            size
        }
    };

    let start = offset.to_string();
    let (mut child, mut log) = runner.spawn(logger, "tail", STREAM_SCRIPT, &[path, &start])?;
    log.equivalent(format!("tail -c +{} -F {}", offset + 1, shell_line([path])));
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = BufReader::new(child.stderr.take().expect("piped stderr")).lines();
    let mut splitter = LineSplitter::default();
    let mut buf = vec![0u8; 64 * 1024];
    let mut received: u64 = 0;
    let mut stderr_open = true;
    let mut last_error = String::new();
    let result = loop {
        tokio::select! {
            _ = cancel.cancelled() => break Err(AppError::Cancelled),
            read = stdout.read(&mut buf) => match read {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    let chunk = &buf[..n];
                    if received == 0 && offset == 0 && looks_binary(chunk) {
                        break Err(AppError::Other("il file sembra binario: niente da seguire".into()));
                    }
                    received += n as u64;
                    offset += n as u64;
                    let mut lines = Vec::new();
                    splitter.push(chunk, &mut lines);
                    emitter.lines(lines, offset);
                }
                Err(e) => break Err(e.into()),
            },
            line = stderr.next_line(), if stderr_open => match line {
                Ok(Some(line)) => {
                    let line = line.trim().to_string();
                    if line.contains("truncated") {
                        // tail starts again from the beginning of the file.
                        offset = 0;
                        splitter.reset();
                        emitter.notice("— file troncato: riparte dall'inizio —");
                    } else if line.contains("has been replaced") || line.contains("has appeared") {
                        offset = 0;
                        splitter.reset();
                        emitter.notice("— file sostituito (ruotato): segue il nuovo —");
                    } else if line.contains("inaccessible") || line.contains("No such file") {
                        emitter.notice("— file non trovato, in attesa che ricompaia… —");
                    } else if !line.is_empty() {
                        last_error = line.clone();
                        emitter.notice(line);
                    }
                }
                _ => stderr_open = false,
            },
            _ = tokio::time::sleep(BATCH_INTERVAL) => emitter.flush(),
        }
    };
    emitter.flush();
    let summary = format!("{received} byte ricevuti, fermo a {offset}");
    match &result {
        Ok(()) => {
            // tail never ends by itself: the connection or the process died.
            let status = child.wait().await.ok().and_then(|s| s.code());
            let message = if last_error.is_empty() { format!("tail terminato (exit {})", status.unwrap_or(-1)) } else { last_error };
            log.fail(format!("{message}; {summary}"));
            return Err(AppError::Other(message));
        }
        Err(AppError::Cancelled) => log.ok(Some(summary)),
        Err(e) => log.fail(format!("{e}; {summary}")),
    }
    result
}

/// SFTP path: the last lines, then polling.
async fn sftp_tail(session: &Session, request: &Request<'_>, cancel: &Cancel, emitter: &mut Emitter) -> Result<()> {
    let path = request.path;
    let mut log = session.launcher.log(Channel::Sftp, "tail", path);
    log.equivalent(format!("tail -n {} -F {}", request.lines, shell_line([path])));
    let mut stats = SftpStats::default();
    let result = sftp_loop(session, request, cancel, emitter, &mut stats).await;
    let summary = format!("{} byte letti in {} richieste, fermo a {}", stats.bytes, stats.requests, emitter.offset);
    match &result {
        Ok(()) | Err(AppError::Cancelled) => log.ok(Some(summary)),
        Err(e) => log.fail(format!("{e}; {summary}")),
    }
    result
}

#[derive(Default)]
struct SftpStats {
    requests: u64,
    bytes: u64,
}

fn is_not_found(error: &AppError) -> bool {
    use openssh_sftp_client::error::SftpErrorKind;
    matches!(error, AppError::Sftp(openssh_sftp_client::Error::SftpError(SftpErrorKind::NoSuchFile, _)))
}

async fn file_size(session: &Session, path: &str, stats: &mut SftpStats) -> Result<u64> {
    stats.requests += 1;
    let meta = session.sftp.fs().metadata(path).await?;
    Ok(meta.len().unwrap_or(0))
}

/// Reads `len` bytes at `offset` (fewer at end of file).
async fn read_range(session: &Session, path: &str, offset: u64, len: u64, stats: &mut SftpStats) -> Result<Vec<u8>> {
    let mut file = session.sftp.open(path).await?;
    stats.requests += 2;
    Pin::new(&mut file).start_seek(SeekFrom::Start(offset))?;
    let mut out = Vec::with_capacity(len as usize);
    while (out.len() as u64) < len {
        let want = (len - out.len() as u64).min(u32::MAX as u64) as u32;
        stats.requests += 1;
        match file.read(want, BytesMut::with_capacity(want as usize)).await? {
            Some(chunk) if !chunk.is_empty() => out.extend_from_slice(&chunk),
            _ => break,
        }
    }
    stats.bytes += out.len() as u64;
    let _ = file.close().await;
    Ok(out)
}

/// The first bytes of the file: when they change, it is another file
/// (rotated). SFTP v3 has no inode to compare.
#[derive(PartialEq, Eq)]
struct Head {
    len: u64,
    hash: [u8; 32],
}

async fn read_head(session: &Session, path: &str, len: u64, stats: &mut SftpStats) -> Result<Option<Head>> {
    if len == 0 {
        return Ok(None);
    }
    let data = read_range(session, path, 0, len.min(HEAD_BYTES), stats).await?;
    Ok(Some(Head { len: data.len() as u64, hash: Sha256::digest(&data).into() }))
}

async fn sftp_loop(session: &Session, request: &Request<'_>, cancel: &Cancel, emitter: &mut Emitter, stats: &mut SftpStats) -> Result<()> {
    let path = request.path;
    let mut offset = match request.from_offset {
        Some(offset) => offset,
        None => {
            let size = file_size(session, path, stats).await?;
            // Blocks from the end until there are enough lines.
            let mut start = size;
            let mut data = Vec::new();
            loop {
                let block = (64 * 1024).min(start);
                start -= block;
                let mut chunk = read_range(session, path, start, block, stats).await?;
                chunk.extend_from_slice(&data);
                data = chunk;
                let newlines = data.iter().filter(|&&b| b == b'\n').count();
                if start == 0 || newlines > request.lines as usize || data.len() as u64 >= MAX_INITIAL_BYTES {
                    break;
                }
            }
            if looks_binary(&data) {
                return Err(AppError::Other("il file sembra binario: niente da seguire".into()));
            }
            // Only what ends with a complete line counts as read.
            let complete = data.iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
            emitter.lines(last_lines(&data[..complete], request.lines as usize, start == 0), start + complete as u64);
            emitter.flush();
            start + complete as u64
        }
    };

    let mut size = file_size(session, path, stats).await?;
    let mut head = read_head(session, path, size, stats).await?;
    let mut splitter = LineSplitter::default();
    let mut interval = POLL_MIN;
    let mut missing = false;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            _ = tokio::time::sleep(interval) => {}
        }
        size = match file_size(session, path, stats).await {
            Ok(size) => size,
            Err(e) if is_not_found(&e) => {
                if !missing {
                    missing = true;
                    emitter.notice("— file non trovato, in attesa che ricompaia… —");
                }
                interval = POLL_MAX;
                continue;
            }
            Err(e) => return Err(e),
        };
        let mut restarted = false;
        if missing {
            missing = false;
            emitter.notice("— file ricomparso: riparte dall'inizio —");
            restarted = true;
        } else if size < offset {
            emitter.notice("— file troncato: riparte dall'inizio —");
            restarted = true;
        } else if size > offset
            && let Some(known) = &head
        {
            // New data: is it still the same file? (one small read)
            let current = read_head(session, path, known.len, stats).await?;
            if current.as_ref() != Some(known) {
                emitter.notice("— file sostituito (ruotato): segue il nuovo —");
                restarted = true;
            } else if known.len < HEAD_BYTES && size > known.len {
                head = read_head(session, path, size, stats).await?;
            }
        }
        if restarted {
            offset = 0;
            splitter.reset();
            emitter.offset = 0;
        }
        if head.is_none() || restarted {
            head = read_head(session, path, size, stats).await?;
        }
        if size > offset {
            let len = (size - offset).min(READ_CHUNK);
            let data = read_range(session, path, offset, len, stats).await?;
            offset += data.len() as u64;
            let mut lines = Vec::new();
            splitter.push(&data, &mut lines);
            emitter.lines(lines, offset);
            interval = POLL_MIN;
        } else {
            interval = (interval.mul_f32(1.5)).min(POLL_MAX);
        }
        emitter.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(chunks: &[&[u8]]) -> Vec<String> {
        let mut splitter = LineSplitter::default();
        let mut out = Vec::new();
        for chunk in chunks {
            splitter.push(chunk, &mut out);
        }
        out
    }

    #[test]
    fn splits_lines_across_chunks() {
        assert_eq!(split(&[b"a\nb", b"c\r\nd"]), vec!["a", "bc"]);
        assert_eq!(split(&[b"\n\n"]), vec!["", ""]);
        assert_eq!(split(&[b"caf\xc3", b"\xa9\n"]), vec!["café"]);
        assert_eq!(split(&[b"bad \xff\n"]), vec!["bad \u{fffd}"]);
    }

    #[test]
    fn cuts_long_lines_once() {
        let long = vec![b'x'; MAX_LINE_BYTES + 10];
        let lines = split(&[&long[..MAX_LINE_BYTES - 5], &long[MAX_LINE_BYTES - 5..], b"\nnext\n"]);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("[riga troncata]"));
        assert_eq!(lines[0].len(), MAX_LINE_BYTES + " … [riga troncata]".len());
        assert_eq!(lines[1], "next");
    }

    #[test]
    fn reset_drops_the_partial_line() {
        let mut splitter = LineSplitter::default();
        let mut out = Vec::new();
        splitter.push(b"half", &mut out);
        splitter.reset();
        splitter.push(b"new\n", &mut out);
        assert_eq!(out, vec!["new"]);
    }

    #[test]
    fn takes_the_last_lines() {
        assert_eq!(last_lines(b"1\n2\n3\n", 2, true), vec!["2", "3"]);
        assert_eq!(last_lines(b"1\n2\n3\n", 10, true), vec!["1", "2", "3"]);
        // Not at the start: the first (cut) line is dropped.
        assert_eq!(last_lines(b"ut\n2\n3\n", 10, false), vec!["2", "3"]);
        // An unfinished last line waits.
        assert_eq!(last_lines(b"1\n2\npartial", 10, true), vec!["1", "2"]);
        assert!(last_lines(b"", 10, true).is_empty());
    }

    #[test]
    fn detects_binary() {
        assert!(looks_binary(b"ab\0cd"));
        assert!(!looks_binary(b"plain text\n"));
    }

    #[test]
    fn scripts_run_on_this_system() {
        let dir = std::env::temp_dir().join(format!("gf-tail-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("app.log");
        std::fs::write(&file, "one\ntwo\nthree\nfour\n").unwrap();
        let path = file.to_str().unwrap();
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", INITIAL_SCRIPT, "sh", path, "2"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "19\nthree\nfour\n");
        let missing = std::process::Command::new("/bin/sh")
            .args(["-c", INITIAL_SCRIPT, "sh", "/nonexistent/x.log", "2"])
            .output()
            .unwrap();
        assert!(!missing.status.success());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
