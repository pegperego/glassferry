//! Upload/download jobs between the local filesystem and an SFTP session.
//!
//! A job copies a selection of entries from one directory to another,
//! recursively. Files are written to a `.sfm-part` temporary and renamed into
//! place once complete, so the destination never holds a truncated file.
//! Each file keeps several read/write requests in flight (the SFTP protocol
//! is request/response: one request at a time would be latency-bound), and a
//! global semaphore limits how many files transfer at once.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::{Bytes, BytesMut};
use futures_util::stream::{FuturesOrdered, FuturesUnordered, StreamExt};
use openssh_sftp_client::file::File as SftpFile;
use openssh_sftp_client::metadata::{MetaDataBuilder, Permissions, RawPermissions};
use openssh_sftp_client::UnixTimeStamp;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeek, AsyncWriteExt};
use tokio::sync::{oneshot, Semaphore};

use crate::entry::EntryKind;
use crate::error::{AppError, Result};
use crate::log::{sftp_equivalent, shell_line, Channel, Logger};
use crate::session::Session;

/// Files transferred at the same time, across all jobs.
const PARALLEL_FILES: usize = 3;
/// Read/write requests in flight per file.
const REQUESTS_IN_FLIGHT: usize = 16;
/// Bytes per request slot; the client splits it to the server limits.
const CHUNK_LEN: u32 = 256 * 1024;
const PART_SUFFIX: &str = ".sfm-part";
const UPDATE_INTERVAL: Duration = Duration::from_millis(200);
const MAX_REPORTED_ERRORS: usize = 50;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Scanning,
    Running,
    WaitingConflict,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub id: u64,
    pub session_id: u64,
    pub host: String,
    pub direction: Direction,
    pub src_dir: String,
    pub dst_dir: String,
    pub names: Vec<String>,
    pub state: JobState,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub total_files: u64,
    pub done_files: u64,
    pub skipped_files: u64,
    pub failed_files: u64,
    /// Relative path of a file being transferred.
    pub current: Option<String>,
    pub bytes_per_sec: u64,
    /// Milliseconds since the Unix epoch.
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConflictAction {
    Overwrite,
    Skip,
    Cancel,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictRequest {
    pub id: u64,
    pub job_id: u64,
    pub host: String,
    pub direction: Direction,
    /// Destination path that already exists.
    pub path: String,
    pub src_size: u64,
    pub src_modified: Option<u64>,
    pub dst_size: Option<u64>,
    pub dst_modified: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Policy {
    Ask,
    OverwriteAll,
    SkipAll,
}

pub enum TransferEvent {
    Updates(Vec<JobInfo>),
    Conflict(ConflictRequest),
}

type Sink = Arc<dyn Fn(TransferEvent) + Send + Sync>;
type ConflictReply = oneshot::Sender<(ConflictAction, bool)>;

struct Job {
    session: Arc<Session>,
    info: Mutex<JobInfo>,
    done_bytes: AtomicU64,
    cancelled: AtomicBool,
    dirty: AtomicBool,
    /// Serialises conflict prompts so "apply to all" takes effect.
    policy: tokio::sync::Mutex<Policy>,
    /// Running files: relative path -> start order, for the `current` field.
    running: Mutex<Vec<String>>,
}

impl Job {
    fn update(&self, f: impl FnOnce(&mut JobInfo)) {
        f(&mut self.info.lock().unwrap());
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn check_cancelled(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(AppError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn add_bytes(&self, n: u64) {
        self.done_bytes.fetch_add(n, Ordering::Relaxed);
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn push_error(&self, message: String) {
        self.update(|info| {
            info.failed_files += 1;
            if info.errors.len() < MAX_REPORTED_ERRORS {
                info.errors.push(message);
            }
        });
    }

    fn snapshot(&self) -> JobInfo {
        let mut info = self.info.lock().unwrap().clone();
        info.done_bytes = self.done_bytes.load(Ordering::Relaxed);
        info.current = self.running.lock().unwrap().first().cloned();
        info
    }
}

pub struct TransferManager {
    logger: Arc<Logger>,
    sink: Sink,
    next_id: AtomicU64,
    jobs: Mutex<HashMap<u64, Arc<Job>>>,
    file_slots: Arc<Semaphore>,
    /// Pending conflict prompts: id -> (job id, answer channel).
    conflicts: Mutex<HashMap<u64, (u64, ConflictReply)>>,
}

impl TransferManager {
    pub fn new(logger: Arc<Logger>, sink: Sink) -> Arc<Self> {
        let manager = Arc::new(Self {
            logger,
            sink,
            next_id: AtomicU64::new(1),
            jobs: Mutex::new(HashMap::new()),
            file_slots: Arc::new(Semaphore::new(PARALLEL_FILES)),
            conflicts: Mutex::new(HashMap::new()),
        });
        let weak = Arc::downgrade(&manager);
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(UPDATE_INTERVAL);
            let mut samples: HashMap<u64, (Instant, u64, f64)> = HashMap::new();
            loop {
                interval.tick().await;
                let Some(manager) = weak.upgrade() else { break };
                manager.emit_updates(&mut samples);
            }
        });
        manager
    }

    /// Sends dirty jobs to the UI, with a smoothed transfer rate.
    fn emit_updates(&self, samples: &mut HashMap<u64, (Instant, u64, f64)>) {
        let jobs: Vec<Arc<Job>> = self.jobs.lock().unwrap().values().cloned().collect();
        let now = Instant::now();
        let mut batch = Vec::new();
        for job in jobs {
            if !job.dirty.swap(false, Ordering::Relaxed) {
                continue;
            }
            let mut info = job.snapshot();
            let (last_time, last_bytes, rate) = samples
                .get(&info.id)
                .copied()
                .unwrap_or((now, info.done_bytes, 0.0));
            let elapsed = now.duration_since(last_time).as_secs_f64();
            let rate = if elapsed > 0.0 {
                let instant = info.done_bytes.saturating_sub(last_bytes) as f64 / elapsed;
                if rate == 0.0 { instant } else { rate * 0.7 + instant * 0.3 }
            } else {
                rate
            };
            samples.insert(info.id, (now, info.done_bytes, rate));
            info.bytes_per_sec = if info.state == JobState::Running { rate as u64 } else { 0 };
            batch.push(info);
        }
        if !batch.is_empty() {
            (self.sink)(TransferEvent::Updates(batch));
        }
    }

    pub fn list(&self) -> Vec<JobInfo> {
        let mut jobs: Vec<JobInfo> = self.jobs.lock().unwrap().values().map(|j| j.snapshot()).collect();
        jobs.sort_by_key(|j| j.id);
        jobs
    }

    /// Forgets finished jobs.
    pub fn clear_finished(&self) {
        self.jobs.lock().unwrap().retain(|_, job| {
            !matches!(
                job.info.lock().unwrap().state,
                JobState::Done | JobState::Failed | JobState::Cancelled
            )
        });
    }

    pub fn cancel(&self, job_id: u64) {
        if let Some(job) = self.jobs.lock().unwrap().get(&job_id) {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        // A job waiting on a conflict prompt must wake up.
        let mut conflicts = self.conflicts.lock().unwrap();
        let ids: Vec<u64> = conflicts.iter().filter(|(_, (job, _))| *job == job_id).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some((_, tx)) = conflicts.remove(&id) {
                let _ = tx.send((ConflictAction::Skip, false));
            }
        }
    }

    pub fn cancel_session(&self, session_id: u64) {
        let ids: Vec<u64> = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .filter(|j| j.info.lock().unwrap().session_id == session_id)
            .map(|j| j.info.lock().unwrap().id)
            .collect();
        for id in ids {
            self.cancel(id);
        }
    }

    pub fn respond_conflict(&self, conflict_id: u64, action: ConflictAction, apply_to_all: bool) {
        if let Some((_, tx)) = self.conflicts.lock().unwrap().remove(&conflict_id) {
            let _ = tx.send((action, apply_to_all));
        }
    }

    pub fn start(
        self: &Arc<Self>,
        session_id: u64,
        session: Arc<Session>,
        direction: Direction,
        src_dir: String,
        names: Vec<String>,
        dst_dir: String,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let info = JobInfo {
            id,
            session_id,
            host: session.launcher.alias.clone(),
            direction,
            src_dir,
            dst_dir,
            names,
            state: JobState::Scanning,
            total_bytes: 0,
            done_bytes: 0,
            total_files: 0,
            done_files: 0,
            skipped_files: 0,
            failed_files: 0,
            current: None,
            bytes_per_sec: 0,
            started_at: now_ms(),
            finished_at: None,
            errors: Vec::new(),
        };
        let job = Arc::new(Job {
            session,
            info: Mutex::new(info),
            done_bytes: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            dirty: AtomicBool::new(true),
            policy: tokio::sync::Mutex::new(Policy::Ask),
            running: Mutex::new(Vec::new()),
        });
        self.jobs.lock().unwrap().insert(id, job.clone());
        let manager = self.clone();
        tauri::async_runtime::spawn(async move { manager.run(job).await });
        id
    }

    async fn run(self: Arc<Self>, job: Arc<Job>) {
        let (host, direction, label, src_dir, dst_dir, names) = {
            let info = job.info.lock().unwrap();
            let label = format!(
                "{} {} {} → {}",
                match info.direction {
                    Direction::Upload => "upload",
                    Direction::Download => "download",
                },
                describe_names(&info.names),
                info.src_dir,
                info.dst_dir
            );
            (info.host.clone(), info.direction, label, info.src_dir.clone(), info.dst_dir.clone(), info.names.clone())
        };
        let summary = self.logger.start(Some(&host), Channel::Sftp, "transfer", label);

        let plan = match self.scan(&job, direction, &src_dir, &names).await {
            Ok(plan) => plan,
            Err(e) => {
                let state = if matches!(e, AppError::Cancelled) { JobState::Cancelled } else { JobState::Failed };
                job.update(|i| {
                    i.state = state;
                    i.errors.push(e.to_string());
                    i.finished_at = Some(now_ms());
                });
                summary.fail(e);
                return;
            }
        };
        job.update(|i| {
            i.state = JobState::Running;
            i.total_files = plan.files.len() as u64;
            i.total_bytes = plan.files.iter().map(|f| f.size).sum();
        });

        for dir in &plan.dirs {
            if job.check_cancelled().is_err() {
                break;
            }
            if let Err(e) = self.make_dir(&job, direction, &join(&dst_dir, dir)).await {
                job.push_error(format!("{dir}: {e}"));
            }
        }

        futures_util::stream::iter(plan.files.iter())
            .for_each_concurrent(None, |file| {
                let manager = self.clone();
                let job = job.clone();
                let (src_dir, dst_dir) = (src_dir.clone(), dst_dir.clone());
                async move {
                    let Ok(_slot) = manager.file_slots.clone().acquire_owned().await else { return };
                    if job.check_cancelled().is_err() {
                        return;
                    }
                    let src = join(&src_dir, &file.rel);
                    let dst = join(&dst_dir, &file.rel);
                    match manager.transfer_file(&job, direction, file, &src, &dst).await {
                        Ok(true) => job.update(|i| i.done_files += 1),
                        Ok(false) => job.update(|i| i.skipped_files += 1),
                        Err(AppError::Cancelled) => {}
                        Err(e) => job.push_error(format!("{}: {e}", file.rel)),
                    }
                }
            })
            .await;

        let info = job.snapshot();
        let state = if job.cancelled.load(Ordering::Relaxed) {
            JobState::Cancelled
        } else if info.failed_files > 0 {
            JobState::Failed
        } else {
            JobState::Done
        };
        job.update(|i| {
            i.state = state;
            i.finished_at = Some(now_ms());
        });
        let secs = (now_ms() - info.started_at) as f64 / 1000.0;
        let extra = format!(
            "{} file, {}{}{}",
            info.done_files,
            format_bytes(info.done_bytes),
            if secs > 0.0 { format!(", {}/s", format_bytes((info.done_bytes as f64 / secs) as u64)) } else { String::new() },
            if info.skipped_files > 0 { format!(", {} saltati", info.skipped_files) } else { String::new() },
        );
        match state {
            JobState::Done => summary.ok(Some(extra)),
            JobState::Cancelled => summary.fail(format!("annullato ({extra})")),
            _ => summary.fail(format!("{} errori ({extra})", info.failed_files)),
        }
    }

    // ---- scanning ----------------------------------------------------------

    async fn scan(&self, job: &Job, direction: Direction, src_dir: &str, names: &[String]) -> Result<Plan> {
        let host = job.session.launcher.alias.clone();
        let op = self.logger.start(
            Some(&host),
            if direction == Direction::Upload { Channel::Local } else { Channel::Sftp },
            "scan",
            format!("{src_dir}: {}", describe_names(names)),
        );
        let result = match direction {
            Direction::Upload => {
                let (src_dir, names) = (PathBuf::from(src_dir), names.to_vec());
                tokio::task::spawn_blocking(move || scan_local(&src_dir, &names))
                    .await
                    .map_err(|e| AppError::Other(e.to_string()))
                    .and_then(|r| r)
            }
            Direction::Download => scan_remote(job, src_dir, names).await,
        };
        op.result(result, |plan| {
            Some(format!(
                "{} file, {} cartelle, {}{}",
                plan.files.len(),
                plan.dirs.len(),
                format_bytes(plan.files.iter().map(|f| f.size).sum()),
                if plan.skipped.is_empty() { String::new() } else { format!(", {} link a cartelle ignorati", plan.skipped.len()) }
            ))
        })
    }

    // ---- directories ---------------------------------------------------------

    async fn make_dir(&self, job: &Job, direction: Direction, path: &str) -> Result<()> {
        let host = job.session.launcher.alias.clone();
        match direction {
            Direction::Download => {
                let path = PathBuf::from(path);
                match tokio::fs::symlink_metadata(&path).await {
                    Ok(meta) if meta.is_dir() => return Ok(()),
                    Ok(_) => return Err(AppError::Other("esiste già un file con lo stesso nome".into())),
                    Err(_) => {}
                }
                let mut op = self.logger.start(Some(&host), Channel::Local, "mkdir", path.to_string_lossy());
                op.equivalent(format!("mkdir {}", shell_line([&path])));
                op.result(tokio::fs::create_dir(&path).await.map_err(AppError::from), |_| None)
            }
            Direction::Upload => {
                let mut fs = job.session.sftp.fs();
                match fs.metadata(path).await {
                    Ok(meta) if meta.file_type().is_some_and(|t| t.is_dir()) => return Ok(()),
                    Ok(_) => return Err(AppError::Other("esiste già un file con lo stesso nome".into())),
                    Err(_) => {}
                }
                let mut op = self.logger.start(Some(&host), Channel::Sftp, "mkdir", path);
                op.equivalent(sftp_equivalent(&host, "mkdir", &[path]));
                op.result(fs.create_dir(path).await.map_err(AppError::from), |_| None)
            }
        }
    }

    // ---- files -------------------------------------------------------------

    /// Returns `Ok(false)` when the file was skipped because of a conflict.
    async fn transfer_file(&self, job: &Arc<Job>, direction: Direction, file: &PlannedFile, src: &str, dst: &str) -> Result<bool> {
        let existing = match direction {
            Direction::Download => tokio::fs::metadata(dst).await.ok().map(|m| {
                let modified = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs());
                (m.is_dir(), m.len(), modified)
            }),
            Direction::Upload => job.session.sftp.fs().metadata(dst).await.ok().map(|m| {
                (
                    m.file_type().is_some_and(|t| t.is_dir()),
                    m.len().unwrap_or(0),
                    m.modified().map(|t| t.as_duration().as_secs()),
                )
            }),
        };
        if let Some((is_dir, size, modified)) = existing {
            if is_dir {
                return Err(AppError::Other("esiste già una cartella con lo stesso nome".into()));
            }
            if !self.resolve_conflict(job, direction, file, dst, size, modified).await? {
                return Ok(false);
            }
        }

        job.running.lock().unwrap().push(file.rel.clone());
        let host = job.session.launcher.alias.clone();
        let (op_name, detail, equivalent) = match direction {
            Direction::Upload => ("upload", format!("{src} → {dst}"), sftp_equivalent(&host, "put", &[src, dst])),
            Direction::Download => ("download", format!("{src} → {dst}"), sftp_equivalent(&host, "get", &[src, dst])),
        };
        let mut op = self.logger.start(Some(&host), Channel::Sftp, op_name, detail);
        op.equivalent(equivalent);
        let started = Instant::now();
        let result = match direction {
            Direction::Download => download(job, src, Path::new(dst), file).await,
            Direction::Upload => upload(job, Path::new(src), dst, file).await,
        };
        job.running.lock().unwrap().retain(|r| r != &file.rel);
        let secs = started.elapsed().as_secs_f64();
        op.result(result, |bytes| {
            Some(if secs > 0.05 {
                format!("{} · {}/s", format_bytes(*bytes), format_bytes((*bytes as f64 / secs) as u64))
            } else {
                format_bytes(*bytes)
            })
        })?;
        Ok(true)
    }

    async fn resolve_conflict(
        &self,
        job: &Arc<Job>,
        direction: Direction,
        file: &PlannedFile,
        dst: &str,
        dst_size: u64,
        dst_modified: Option<u64>,
    ) -> Result<bool> {
        let mut policy = job.policy.lock().await;
        match *policy {
            Policy::OverwriteAll => return Ok(true),
            Policy::SkipAll => return Ok(false),
            Policy::Ask => {}
        }
        job.check_cancelled()?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let job_id = job.info.lock().unwrap().id;
        self.conflicts.lock().unwrap().insert(id, (job_id, tx));
        job.update(|i| i.state = JobState::WaitingConflict);
        (self.sink)(TransferEvent::Conflict(ConflictRequest {
            id,
            job_id,
            host: job.session.launcher.alias.clone(),
            direction,
            path: dst.to_string(),
            src_size: file.size,
            src_modified: file.modified,
            dst_size: Some(dst_size),
            dst_modified,
        }));
        let answer = rx.await.unwrap_or((ConflictAction::Cancel, false));
        job.update(|i| i.state = JobState::Running);
        job.check_cancelled()?;
        match answer {
            (ConflictAction::Cancel, _) => {
                job.cancelled.store(true, Ordering::Relaxed);
                Err(AppError::Cancelled)
            }
            (ConflictAction::Overwrite, all) => {
                if all {
                    *policy = Policy::OverwriteAll;
                }
                Ok(true)
            }
            (ConflictAction::Skip, all) => {
                if all {
                    *policy = Policy::SkipAll;
                }
                Ok(false)
            }
        }
    }
}

// ---- plan ------------------------------------------------------------------

struct PlannedFile {
    /// Path relative to the source/destination directory, `/`-separated.
    rel: String,
    size: u64,
    /// Seconds since the Unix epoch.
    modified: Option<u64>,
    permissions: Option<u32>,
}

#[derive(Default)]
struct Plan {
    /// Directories to create, parents first.
    dirs: Vec<String>,
    files: Vec<PlannedFile>,
    /// Symlinks to directories: not followed, to avoid loops.
    skipped: Vec<String>,
}

fn scan_local(src_dir: &Path, names: &[String]) -> Result<Plan> {
    fn visit(base: &Path, rel: String, plan: &mut Plan) -> Result<()> {
        let path = base.join(&rel);
        let link = std::fs::symlink_metadata(&path)?;
        let meta = std::fs::metadata(&path)?;
        if meta.is_dir() {
            if link.file_type().is_symlink() {
                plan.skipped.push(rel);
                return Ok(());
            }
            let mut children: Vec<String> = std::fs::read_dir(&path)?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            children.sort();
            plan.dirs.push(rel.clone());
            for child in children {
                visit(base, format!("{rel}/{child}"), plan)?;
            }
        } else if meta.is_file() {
            use std::os::unix::fs::PermissionsExt;
            plan.files.push(PlannedFile {
                rel,
                size: meta.len(),
                modified: meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
                permissions: Some(meta.permissions().mode() & 0o777),
            });
        }
        Ok(())
    }
    let mut plan = Plan::default();
    for name in names {
        visit(src_dir, name.clone(), &mut plan)?;
    }
    Ok(plan)
}

async fn scan_remote(job: &Job, src_dir: &str, names: &[String]) -> Result<Plan> {
    let mut plan = Plan::default();
    // Explicit stack instead of async recursion; children are pushed in
    // reverse so the traversal stays parents-first and sorted.
    let mut stack: Vec<String> = names.iter().rev().cloned().collect();
    let mut fs = job.session.sftp.fs();
    while let Some(rel) = stack.pop() {
        job.check_cancelled()?;
        let path = join(src_dir, &rel);
        let link = fs.symlink_metadata(&path).await?;
        let is_symlink = link.file_type().is_some_and(|t| t.is_symlink());
        let meta = if is_symlink { fs.metadata(&path).await? } else { link };
        if meta.file_type().is_some_and(|t| t.is_dir()) {
            if is_symlink {
                plan.skipped.push(rel);
                continue;
            }
            let mut children: Vec<_> = job.session.read_dir(Path::new(&path)).await?;
            children.sort_by(|a, b| b.name.cmp(&a.name));
            plan.dirs.push(rel.clone());
            for child in children {
                if child.kind == EntryKind::Symlink && child.target_is_dir {
                    plan.skipped.push(format!("{rel}/{}", child.name));
                } else if matches!(child.kind, EntryKind::Dir | EntryKind::File | EntryKind::Symlink) {
                    stack.push(format!("{rel}/{}", child.name));
                }
            }
        } else if meta.file_type().is_some_and(|t| t.is_file()) {
            plan.files.push(PlannedFile {
                rel,
                size: meta.len().unwrap_or(0),
                modified: meta.modified().map(|t| t.as_duration().as_secs()),
                permissions: meta.permissions().map(|p| p.as_raw().bits() & 0o777),
            });
        }
    }
    Ok(plan)
}

// ---- data transfer -----------------------------------------------------------

fn part_name(path: &str) -> String {
    format!("{path}{PART_SUFFIX}")
}

/// Reads `len` bytes at `offset` (fewer only at end of file).
async fn read_at(mut file: SftpFile, offset: u64, len: u32) -> Result<BytesMut> {
    Pin::new(&mut file).start_seek(SeekFrom::Start(offset))?;
    let mut out = BytesMut::with_capacity(len as usize);
    while out.len() < len as usize {
        let want = len - out.len() as u32;
        match file.read(want, BytesMut::with_capacity(want as usize)).await? {
            Some(chunk) if !chunk.is_empty() => out.extend_from_slice(&chunk),
            _ => break,
        }
    }
    Ok(out)
}

async fn write_at(mut file: SftpFile, offset: u64, data: Bytes) -> Result<u64> {
    Pin::new(&mut file).start_seek(SeekFrom::Start(offset))?;
    file.write_all(&data).await?;
    Ok(data.len() as u64)
}

async fn download(job: &Job, src: &str, dst: &Path, file: &PlannedFile) -> Result<u64> {
    let part = PathBuf::from(part_name(&dst.to_string_lossy()));
    let result = download_to(job, src, &part).await;
    match result {
        Ok(bytes) => {
            if let Some(mtime) = file.modified {
                set_local_mtime(&part, mtime);
            }
            if let Some(mode) = file.permissions {
                // Only for new files: an overwritten file keeps the user's mode.
                if !dst.exists() {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&part, std::fs::Permissions::from_mode(mode));
                }
            }
            tokio::fs::rename(&part, dst).await?;
            Ok(bytes)
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(e)
        }
    }
}

async fn download_to(job: &Job, src: &str, part: &Path) -> Result<u64> {
    let remote = job.session.sftp.open(src).await?;
    let chunk = CHUNK_LEN;
    let mut out = tokio::fs::File::create(part).await?;
    let mut offset = 0u64;
    let mut written = 0u64;
    let mut eof = false;
    let mut inflight = FuturesOrdered::new();
    loop {
        while !eof && inflight.len() < REQUESTS_IN_FLIGHT {
            inflight.push_back(read_at(remote.clone(), offset, chunk));
            // Requests go past the size seen while scanning (the file may
            // have grown): a short read below marks the real end of file.
            offset += chunk as u64;
        }
        let Some(data) = inflight.next().await else { break };
        let data = data?;
        job.check_cancelled()?;
        out.write_all(&data).await?;
        written += data.len() as u64;
        job.add_bytes(data.len() as u64);
        if data.len() < chunk as usize {
            // End of file: drop the requests issued beyond it.
            eof = true;
            inflight = FuturesOrdered::new();
        }
    }
    out.flush().await?;
    drop(remote);
    Ok(written)
}

async fn upload(job: &Job, src: &Path, dst: &str, file: &PlannedFile) -> Result<u64> {
    let part = part_name(dst);
    let mut fs = job.session.sftp.fs();
    let existed = fs.metadata(dst).await.is_ok();
    match upload_to(job, src, &part).await {
        Ok(bytes) => {
            let mut meta = MetaDataBuilder::new();
            if let Some(mtime) = file.modified.and_then(|s| u32::try_from(s).ok()).and_then(UnixTimeStamp::from_raw) {
                meta.time(mtime, mtime);
            }
            if let (false, Some(mode)) = (existed, file.permissions) {
                meta.permissions(Permissions::from_raw(RawPermissions::from_bits_truncate(mode)));
            }
            let _ = fs.set_metadata(&part, meta.create()).await;
            if existed && !job.session.sftp.support_posix_rename() {
                fs.remove_file(dst).await?;
            }
            if let Err(e) = fs.rename(&part, dst).await {
                let _ = fs.remove_file(&part).await;
                return Err(e.into());
            }
            Ok(bytes)
        }
        Err(e) => {
            let _ = fs.remove_file(&part).await;
            Err(e)
        }
    }
}

async fn upload_to(job: &Job, src: &Path, part: &str) -> Result<u64> {
    let mut local = tokio::fs::File::open(src).await?;
    let remote = job
        .session
        .sftp
        .options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(part)
        .await?;
    let chunk = CHUNK_LEN as usize;
    let mut offset = 0u64;
    let mut written = 0u64;
    let mut local_eof = false;
    let mut inflight = FuturesUnordered::new();
    loop {
        while !local_eof && inflight.len() < REQUESTS_IN_FLIGHT {
            let mut buf = vec![0u8; chunk];
            let mut filled = 0;
            while filled < chunk {
                let n = local.read(&mut buf[filled..]).await?;
                if n == 0 {
                    local_eof = true;
                    break;
                }
                filled += n;
            }
            if filled == 0 {
                break;
            }
            buf.truncate(filled);
            inflight.push(write_at(remote.clone(), offset, Bytes::from(buf)));
            offset += filled as u64;
        }
        let Some(result) = inflight.next().await else { break };
        let n = result?;
        job.check_cancelled()?;
        written += n;
        job.add_bytes(n);
    }
    remote.close().await?;
    Ok(written)
}

fn set_local_mtime(path: &Path, epoch_secs: u64) {
    let time = UNIX_EPOCH + Duration::from_secs(epoch_secs);
    if let Ok(file) = std::fs::File::options().write(true).open(path) {
        let _ = file.set_times(std::fs::FileTimes::new().set_modified(time).set_accessed(SystemTime::now()));
    }
}

// ---- helpers -----------------------------------------------------------------

fn join(dir: &str, rel: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{rel}")
    } else {
        format!("{dir}/{rel}")
    }
}

fn describe_names(names: &[String]) -> String {
    match names {
        [one] => one.clone(),
        _ => format!("{} elementi", names.len()),
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_local_tree_parents_first_and_skips_dir_symlinks() {
        let dir = std::env::temp_dir().join(format!("sfm-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("top/sub")).unwrap();
        std::fs::write(dir.join("top/a.txt"), b"aa").unwrap();
        std::fs::write(dir.join("top/sub/b.txt"), b"bbb").unwrap();
        std::fs::write(dir.join("single.txt"), b"s").unwrap();
        std::os::unix::fs::symlink(dir.join("top"), dir.join("top/sub/loop")).unwrap();

        let plan = scan_local(&dir, &["top".into(), "single.txt".into()]).unwrap();
        assert_eq!(plan.dirs, vec!["top", "top/sub"]);
        let files: Vec<_> = plan.files.iter().map(|f| (f.rel.as_str(), f.size)).collect();
        assert_eq!(files, vec![("top/a.txt", 2), ("top/sub/b.txt", 3), ("single.txt", 1)]);
        assert_eq!(plan.skipped, vec!["top/sub/loop"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn formats_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
