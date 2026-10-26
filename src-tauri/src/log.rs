//! Activity log: every ssh process, SFTP request, askpass prompt and local
//! operation the app performs.
//!
//! Entries live in a ring buffer (survives UI reloads), are pushed to the UI
//! in batches and appended to a rotating text file. Long operations are one
//! entry that starts `Pending` and is updated with outcome and duration.
//!
//! Never log secrets: askpass answers, tokens and environment variables.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

pub const DEFAULT_CAPACITY: usize = 1000;
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const ROTATED_FILES: usize = 3;
const FILE_NAME: &str = "activity.log";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Channel {
    Ssh,
    Sftp,
    Exec,
    Askpass,
    Local,
}

impl Channel {
    fn label(self) -> &'static str {
        match self {
            Channel::Ssh => "SSH",
            Channel::Sftp => "SFTP",
            Channel::Exec => "EXEC",
            Channel::Askpass => "ASKPASS",
            Channel::Local => "LOCAL",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Pending,
    Ok,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: u64,
    /// Milliseconds since the Unix epoch.
    pub time: u64,
    pub host: Option<String>,
    pub channel: Channel,
    pub op: String,
    /// Exact command line, protocol request or prompt text.
    pub detail: String,
    pub status: Status,
    pub duration_ms: Option<u64>,
    /// Short result summary: "203 voci", "exit 0", "1.2 MB".
    pub extra: Option<String>,
    pub error: Option<String>,
    /// Long diagnostic output (ssh stderr / -E log, -v debug).
    pub output: Option<String>,
    /// Command to reproduce the operation by hand in a terminal.
    pub equivalent: Option<String>,
}

type Sink = Arc<dyn Fn(Vec<LogEntry>) + Send + Sync>;

pub struct Logger {
    capacity: AtomicUsize,
    next_id: AtomicU64,
    buffer: Mutex<VecDeque<LogEntry>>,
    /// Entries created or updated since the last flush to the sink.
    outbox: Mutex<Vec<LogEntry>>,
    sink: Option<Sink>,
    file_dir: Option<PathBuf>,
    file_lock: Mutex<()>,
}

impl Logger {
    /// `sink` receives batches of new/updated entries every 100 ms.
    /// `file_dir`, when set, receives a rotating human-readable log.
    pub fn new(sink: Option<Sink>, file_dir: Option<PathBuf>) -> Arc<Self> {
        if let Some(dir) = &file_dir {
            let _ = std::fs::create_dir_all(dir);
        }
        let logger = Arc::new(Self {
            capacity: AtomicUsize::new(DEFAULT_CAPACITY),
            next_id: AtomicU64::new(1),
            buffer: Mutex::new(VecDeque::new()),
            outbox: Mutex::new(Vec::new()),
            sink,
            file_dir,
            file_lock: Mutex::new(()),
        });
        if logger.sink.is_some() {
            let weak = Arc::downgrade(&logger);
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(FLUSH_INTERVAL);
                loop {
                    interval.tick().await;
                    let Some(logger) = weak.upgrade() else { break };
                    logger.flush();
                }
            });
        }
        logger
    }

    /// Default log directory: ~/Library/Logs/SSH File Manager on macOS,
    /// $XDG_STATE_HOME/sshfilemanager/logs elsewhere.
    pub fn default_dir() -> Option<PathBuf> {
        if cfg!(target_os = "macos") {
            dirs::home_dir().map(|h| h.join("Library/Logs/SSH File Manager"))
        } else {
            dirs::state_dir().map(|d| d.join("sshfilemanager/logs"))
        }
    }

    pub fn file_dir(&self) -> Option<&Path> {
        self.file_dir.as_deref()
    }

    pub fn set_capacity(&self, capacity: usize) {
        let capacity = capacity.max(1);
        self.capacity.store(capacity, Ordering::Relaxed);
        let mut buffer = self.buffer.lock().unwrap();
        while buffer.len() > capacity {
            buffer.pop_front();
        }
    }

    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.buffer.lock().unwrap().iter().cloned().collect()
    }

    pub fn clear(&self) {
        self.buffer.lock().unwrap().clear();
    }

    /// Starts an operation; finish it through the returned handle.
    pub fn start(
        self: &Arc<Self>,
        host: Option<&str>,
        channel: Channel,
        op: &str,
        detail: impl Into<String>,
    ) -> LogOp {
        let entry = LogEntry {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            time: now_ms(),
            host: host.map(str::to_string),
            channel,
            op: op.to_string(),
            detail: detail.into(),
            status: Status::Pending,
            duration_ms: None,
            extra: None,
            error: None,
            output: None,
            equivalent: None,
        };
        self.upsert(&entry);
        LogOp {
            logger: self.clone(),
            entry,
            started: Instant::now(),
            finished: false,
        }
    }

    fn upsert(&self, entry: &LogEntry) {
        {
            let mut buffer = self.buffer.lock().unwrap();
            // Updates target recent entries: search from the back.
            if let Some(slot) = buffer.iter_mut().rev().find(|e| e.id == entry.id) {
                *slot = entry.clone();
            } else if buffer.front().is_some_and(|first| entry.id < first.id) {
                // Update of an entry already evicted: the file still gets it.
            } else {
                buffer.push_back(entry.clone());
                let capacity = self.capacity.load(Ordering::Relaxed);
                while buffer.len() > capacity {
                    buffer.pop_front();
                }
            }
        }
        if self.sink.is_some() {
            let mut outbox = self.outbox.lock().unwrap();
            if let Some(slot) = outbox.iter_mut().find(|e| e.id == entry.id) {
                *slot = entry.clone();
            } else {
                outbox.push(entry.clone());
            }
        }
        if entry.status != Status::Pending {
            self.append_to_file(entry);
        }
    }

    fn flush(&self) {
        let batch = std::mem::take(&mut *self.outbox.lock().unwrap());
        if let (false, Some(sink)) = (batch.is_empty(), &self.sink) {
            sink(batch);
        }
    }

    fn append_to_file(&self, entry: &LogEntry) {
        let Some(dir) = &self.file_dir else { return };
        let _guard = self.file_lock.lock().unwrap();
        let path = dir.join(FILE_NAME);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
            rotate(dir);
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = file.write_all(format_line(entry).as_bytes());
        }
    }
}

fn rotate(dir: &Path) {
    for i in (1..ROTATED_FILES).rev() {
        let _ = std::fs::rename(
            dir.join(format!("{FILE_NAME}.{i}")),
            dir.join(format!("{FILE_NAME}.{}", i + 1)),
        );
    }
    let _ = std::fs::rename(dir.join(FILE_NAME), dir.join(format!("{FILE_NAME}.1")));
}

fn format_line(entry: &LogEntry) -> String {
    let time = chrono::DateTime::from_timestamp_millis(entry.time as i64)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string()
        })
        .unwrap_or_default();
    let mut line = format!(
        "{time} [{}] {} {} {} {}",
        entry.host.as_deref().unwrap_or("-"),
        entry.channel.label(),
        entry.op,
        if entry.status == Status::Ok { "OK" } else { "ERROR" },
        entry.detail.replace('\n', " "),
    );
    if let Some(ms) = entry.duration_ms {
        line.push_str(&format!(" ({ms} ms)"));
    }
    if let Some(extra) = &entry.extra {
        line.push_str(&format!(" | {extra}"));
    }
    if let Some(error) = &entry.error {
        line.push_str(&format!(" | {}", error.replace('\n', " ")));
    }
    line.push('\n');
    if let Some(output) = &entry.output {
        for out in output.lines() {
            line.push_str("    ");
            line.push_str(out);
            line.push('\n');
        }
    }
    line
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// An operation in progress. Dropping it unfinished records an error, so
/// early returns via `?` still show up in the log.
pub struct LogOp {
    logger: Arc<Logger>,
    entry: LogEntry,
    started: Instant,
    finished: bool,
}

impl LogOp {
    pub fn equivalent(&mut self, command: impl Into<String>) -> &mut Self {
        self.entry.equivalent = Some(command.into());
        self
    }

    pub fn output(&mut self, output: impl Into<String>) -> &mut Self {
        let output = output.into();
        self.entry.output = (!output.trim().is_empty()).then_some(output);
        self
    }

    pub fn ok(mut self, extra: Option<String>) {
        self.entry.extra = extra;
        self.finish(Status::Ok);
    }

    pub fn fail(mut self, error: impl ToString) {
        self.entry.error = Some(error.to_string());
        self.finish(Status::Error);
    }

    /// Records the outcome of `result` and passes it through.
    pub fn result<T, E: ToString>(self, result: Result<T, E>, extra: impl FnOnce(&T) -> Option<String>) -> Result<T, E> {
        match &result {
            Ok(value) => self.ok(extra(value)),
            Err(e) => self.fail(e.to_string()),
        }
        result
    }

    fn finish(&mut self, status: Status) {
        self.finished = true;
        self.entry.status = status;
        self.entry.duration_ms = Some(self.started.elapsed().as_millis() as u64);
        self.logger.upsert(&self.entry);
    }
}

impl Drop for LogOp {
    fn drop(&mut self) {
        if !self.finished {
            self.entry.error = Some("interrotto".into());
            self.finish(Status::Error);
        }
    }
}

/// Renders argv as a command line that can be pasted into a POSIX shell.
pub fn shell_line<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    args.into_iter()
        .map(|a| {
            let a = a.as_ref().to_string_lossy();
            let safe = !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
            if safe {
                a.into_owned()
            } else {
                crate::session::sh_quote(&a)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quotes a path for an `sftp` batch command line.
fn sftp_quote(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `printf '%s\n' '<batch>' | sftp -q -b - <alias>`: reproduces an SFTP
/// request with the stock OpenSSH client.
pub fn sftp_equivalent(alias: &str, command: &str, paths: &[&str]) -> String {
    let mut batch = command.to_string();
    for path in paths {
        batch.push(' ');
        batch.push_str(&sftp_quote(path));
    }
    format!(
        "printf '%s\\n' {} | sftp -q -b - {}",
        crate::session::sh_quote(&batch),
        shell_line([alias])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_keeps_last_entries_and_updates_in_place() {
        let logger = Logger::new(None, None);
        logger.set_capacity(2);
        let first = logger.start(Some("h"), Channel::Sftp, "readdir", "/a");
        logger.start(None, Channel::Local, "readdir", "/b").ok(None);
        logger.start(None, Channel::Local, "readdir", "/c").ok(None);
        // `first` was evicted: finishing it must not bring it back.
        drop(first);
        let details: Vec<_> = logger.snapshot().iter().map(|e| e.detail.clone()).collect();
        assert_eq!(details, vec!["/b", "/c"]);

        let op = logger.start(None, Channel::Local, "readdir", "/d");
        op.fail("denied");
        let last = logger.snapshot().pop().unwrap();
        assert_eq!((last.detail.as_str(), last.status), ("/d", Status::Error));
        assert_eq!(logger.snapshot().len(), 2);
    }

    #[test]
    fn writes_completed_entries_to_file_and_rotates() {
        let dir = std::env::temp_dir().join(format!("sfm-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let logger = Logger::new(None, Some(dir.clone()));
        let mut op = logger.start(Some("web"), Channel::Ssh, "connect", "ssh -- web");
        op.output("debug1: hello\nPermission denied");
        op.fail("Permission denied");
        let content = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(content.contains("[web] SSH connect ERROR ssh -- web"), "{content}");
        assert!(content.contains("    debug1: hello\n"), "{content}");

        rotate(&dir);
        logger.start(None, Channel::Local, "readdir", "/").ok(None);
        assert!(dir.join(format!("{FILE_NAME}.1")).exists());
        assert!(std::fs::read_to_string(dir.join(FILE_NAME)).unwrap().contains("LOCAL readdir OK"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shell_line_quotes_only_when_needed() {
        assert_eq!(
            shell_line(["/usr/bin/ssh", "-S", "/tmp/a b.sock", "--", "host"]),
            "/usr/bin/ssh -S '/tmp/a b.sock' -- host"
        );
    }

    #[test]
    fn sftp_equivalent_quotes_paths() {
        assert_eq!(
            sftp_equivalent("web", "ls -la", &["/srv/it's \"x\""]),
            r#"printf '%s\n' 'ls -la "/srv/it'\''s \"x\""' | sftp -q -b - web"#
        );
    }
}
