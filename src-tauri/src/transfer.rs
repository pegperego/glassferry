//! Upload/download jobs between the local filesystem and an SFTP session,
//! and copies between two remote panes (`Direction::Remote`).
//!
//! A remote copy within one session is done on the server: the `copy-data`
//! SFTP extension when available (OpenSSH >= 9.0, also on SFTP-only
//! accounts), else `cp -p` through the shell. Between two sessions (or when
//! neither is available) the data streams through this machine, pipelined
//! on both sides, without touching the local disk.
//!
//! A job copies a selection of entries from one directory to another,
//! recursively. Files are written to a `.gf-part` temporary and renamed into
//! place once complete, so the destination never holds a truncated file.
//! Each file keeps several read/write requests in flight (the SFTP protocol
//! is request/response: one request at a time would be latency-bound), and a
//! global semaphore limits how many files transfer at once.
//!
//! A failed or cancelled job can be resumed on new sessions of the same
//! connections (`resume`). Its `.gf-part` files are kept with the length
//! known to be complete (writes finish out of order, so only the contiguous
//! prefix counts) and the source's size and mtime: the resume continues from
//! there when the source is unchanged, skips files whose destination already
//! matches the source, and redoes the rest. Server-side copies always
//! restart. Interrupted jobs are recorded in a manifest, so they can be
//! resumed after a restart or a crash too.

use std::collections::{BTreeMap, HashMap};
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
use tokio::io::{AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{oneshot, Semaphore};

use crate::entry::EntryKind;
use crate::error::{AppError, Result};
use crate::log::{shell_line, Channel, Logger};
use crate::session::Session;

/// Files transferred at the same time, across all jobs.
const PARALLEL_FILES: usize = 3;
/// Read/write requests in flight per file.
const REQUESTS_IN_FLIGHT: usize = 16;
/// Bytes per request slot; the client splits it to the server limits.
const CHUNK_LEN: u32 = 256 * 1024;
const PART_SUFFIX: &str = ".gf-part";
const UPDATE_INTERVAL: Duration = Duration::from_millis(200);
const MAX_REPORTED_ERRORS: usize = 50;
const INTERRUPTED_BY_EXIT: &str = "interrotto: l'app è stata chiusa";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Upload,
    Download,
    /// From one remote pane to another (same or different session).
    Remote,
}

/// How a remote → remote file copy is carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyStrategy {
    /// SFTP `copy-data` extension, on the server.
    CopyData,
    /// `cp -p` through an exec channel, on the server.
    Shell,
    /// Read from the source session, write to the destination session.
    Stream,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Scanning,
    Running,
    WaitingConflict,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub id: u64,
    /// Remote side (source for remote copies); `None` for a job restored
    /// from the manifest, which has no session until it is resumed.
    pub session_id: Option<u64>,
    pub connection_id: String,
    pub host: String,
    /// Destination of a remote copy.
    pub dst_session_id: Option<u64>,
    pub dst_connection_id: Option<String>,
    pub dst_host: Option<String>,
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
    /// Failed or cancelled: `resume` can continue it.
    pub resumable: bool,
}

/// One side of a job: a session and the connection it was opened for.
pub struct Endpoint {
    pub session_id: u64,
    pub connection_id: String,
    pub session: Arc<Session>,
}

/// Where an interrupted file can continue: its `.gf-part` holds the first
/// `offset` bytes of a source that had this size and mtime.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PartPoint {
    offset: u64,
    size: u64,
    modified: Option<u64>,
}

/// An interrupted job as the manifest keeps it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedJob {
    info: JobInfo,
    /// "Apply to all" answer given before the interruption.
    policy: Policy,
    /// Relative path → part file left in the destination.
    parts: HashMap<String, PartPoint>,
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
    pub src: String,
    /// How the overwrite is carried out, as commands (preview).
    pub commands: Vec<String>,
    pub src_size: u64,
    pub src_modified: Option<u64>,
    pub dst_size: Option<u64>,
    pub dst_modified: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    /// Remote side; the source of a remote copy.
    session: Arc<Session>,
    /// Destination of a remote copy (may be the same session).
    dst_session: Option<Arc<Session>>,
    info: Mutex<JobInfo>,
    done_bytes: AtomicU64,
    cancelled: AtomicBool,
    dirty: AtomicBool,
    /// Serialises conflict prompts so "apply to all" takes effect.
    policy: tokio::sync::Mutex<Policy>,
    /// Running files: relative path -> start order, for the `current` field.
    running: Mutex<Vec<String>>,
    /// Started by `resume`: destinations matching the source count as done.
    resumed: bool,
    /// Part files to continue (from the interrupted run) or left by this one.
    parts: Mutex<HashMap<String, PartPoint>>,
}

impl Job {
    /// Session written to: the destination of uploads and remote copies.
    fn target(&self) -> &Arc<Session> {
        self.dst_session.as_ref().unwrap_or(&self.session)
    }

    fn copy_strategy(&self) -> CopyStrategy {
        copy_strategy(&self.session, self.target())
    }

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

    fn saved(&self) -> SavedJob {
        SavedJob {
            info: self.snapshot(),
            // Locked only while a conflict is being asked: no answer yet.
            policy: self.policy.try_lock().map(|p| *p).unwrap_or(Policy::Ask),
            parts: self.parts.lock().unwrap().clone(),
        }
    }

    fn is_finished(&self) -> bool {
        matches!(self.info.lock().unwrap().state, JobState::Done | JobState::Failed | JobState::Cancelled)
    }
}

pub struct TransferManager {
    logger: Arc<Logger>,
    sink: Sink,
    /// Interrupted jobs, for resuming after a restart; `None` in tests.
    manifest: Option<PathBuf>,
    next_id: AtomicU64,
    jobs: Mutex<HashMap<u64, Arc<Job>>>,
    /// Interrupted jobs read from the manifest: no session until resumed.
    restored: Mutex<HashMap<u64, SavedJob>>,
    file_slots: Arc<Semaphore>,
    /// Pending conflict prompts: id -> (job id, answer channel).
    conflicts: Mutex<HashMap<u64, (u64, ConflictReply)>>,
}

impl TransferManager {
    pub fn new(logger: Arc<Logger>, manifest: Option<PathBuf>, sink: Sink) -> Arc<Self> {
        let manager = Arc::new(Self {
            logger,
            sink,
            manifest,
            next_id: AtomicU64::new(1),
            jobs: Mutex::new(HashMap::new()),
            restored: Mutex::new(HashMap::new()),
            file_slots: Arc::new(Semaphore::new(PARALLEL_FILES)),
            conflicts: Mutex::new(HashMap::new()),
        });
        manager.restore();
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
        jobs.extend(self.restored.lock().unwrap().values().map(|s| s.info.clone()));
        jobs.sort_by_key(|j| j.id);
        jobs
    }

    fn restore(&self) {
        let Some(path) = &self.manifest else { return };
        let Ok(bytes) = std::fs::read(path) else { return };
        let op = self.logger.start(None, Channel::Local, "transfer-restore", path.to_string_lossy());
        let saved: Vec<SavedJob> = match serde_json::from_slice(&bytes) {
            Ok(saved) => saved,
            Err(e) => return op.fail(e),
        };
        let count = saved.len();
        let mut restored = self.restored.lock().unwrap();
        for mut job in saved {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let info = &mut job.info;
            info.id = id;
            info.session_id = None;
            info.dst_session_id = None;
            info.bytes_per_sec = 0;
            info.current = None;
            info.resumable = true;
            if !matches!(info.state, JobState::Failed | JobState::Cancelled) {
                // Still running when the app stopped.
                info.state = JobState::Failed;
                info.finished_at = Some(now_ms());
                info.errors.push(INTERRUPTED_BY_EXIT.into());
            }
            restored.insert(id, job);
        }
        op.ok(Some(format!("{count} trasferimenti interrotti")));
    }

    /// Records unfinished and interrupted jobs; rewritten whenever one starts,
    /// ends, is resumed or cleared, so a crash leaves the running ones too.
    fn save_manifest(&self) {
        let Some(path) = &self.manifest else { return };
        let mut saved: Vec<SavedJob> = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .filter(|j| j.info.lock().unwrap().state != JobState::Done)
            .map(|j| j.saved())
            .collect();
        saved.extend(self.restored.lock().unwrap().values().cloned());
        saved.sort_by_key(|s| s.info.id);
        let result = (|| -> std::io::Result<()> {
            if saved.is_empty() {
                return match std::fs::remove_file(path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                };
            }
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&saved).map_err(std::io::Error::other)?)?;
            std::fs::rename(&tmp, path)
        })();
        if let Err(e) = result {
            self.logger.start(None, Channel::Local, "transfer-manifest", path.to_string_lossy()).fail(e);
        }
    }

    /// Forgets finished jobs; part files they left behind are removed
    /// (remote ones only while their session is open).
    pub fn clear_finished(&self) {
        let mut cleared: Vec<(SavedJob, Option<Arc<Session>>)> = Vec::new();
        self.jobs.lock().unwrap().retain(|_, job| {
            if !job.is_finished() {
                return true;
            }
            cleared.push((job.saved(), Some(job.target().clone())));
            false
        });
        cleared.extend(self.restored.lock().unwrap().drain().map(|(_, saved)| (saved, None)));
        self.save_manifest();
        for (saved, target) in cleared {
            self.remove_parts(saved, target);
        }
    }

    fn remove_parts(&self, saved: SavedJob, target: Option<Arc<Session>>) {
        if saved.parts.is_empty() {
            return;
        }
        let info = saved.info;
        let parts: Vec<String> = saved.parts.keys().map(|rel| part_name(&join(&info.dst_dir, rel))).collect();
        let host = info.dst_host.clone().unwrap_or(info.host.clone());
        let op = self.logger.start(Some(&host), Channel::Local, "transfer-cleanup", parts.join(", "));
        if info.direction == Direction::Download {
            for part in &parts {
                let _ = std::fs::remove_file(part);
            }
            return op.ok(Some(format!("{} file parziali rimossi", parts.len())));
        }
        let Some(target) = target else {
            return op.fail("connessione chiusa: i file parziali restano sul server");
        };
        tauri::async_runtime::spawn(async move {
            let mut fs = target.sftp.fs();
            for part in &parts {
                let _ = fs.remove_file(part).await;
            }
            op.ok(Some(format!("{} file parziali rimossi", parts.len())));
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
            .filter(|j| {
                let info = j.info.lock().unwrap();
                info.session_id == Some(session_id) || info.dst_session_id == Some(session_id)
            })
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

    /// `dst`: destination of a remote copy (`Direction::Remote`).
    pub fn start(
        self: &Arc<Self>,
        src: Endpoint,
        direction: Direction,
        src_dir: String,
        names: Vec<String>,
        dst_dir: String,
        dst: Option<Endpoint>,
    ) -> u64 {
        let info = JobInfo {
            id: 0,
            session_id: None,
            connection_id: src.connection_id.clone(),
            host: src.session.launcher.label.clone(),
            dst_session_id: None,
            dst_connection_id: dst.as_ref().map(|d| d.connection_id.clone()),
            dst_host: dst.as_ref().map(|d| d.session.launcher.label.clone()),
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
            started_at: 0,
            finished_at: None,
            errors: Vec::new(),
            resumable: false,
        };
        self.spawn_job(info, src, dst, Policy::Ask, HashMap::new(), false)
    }

    /// Runs an interrupted job again, as a new job, on sessions of the same
    /// connections: files already complete are skipped, part files continue
    /// where they stopped when the source is unchanged.
    pub fn resume(self: &Arc<Self>, job_id: u64, src: Endpoint, dst: Option<Endpoint>) -> Result<u64> {
        let live = self.jobs.lock().unwrap().get(&job_id).cloned();
        let saved = match &live {
            Some(job) if job.is_finished() && job.info.lock().unwrap().resumable => job.saved(),
            Some(_) => return Err(AppError::Other("il trasferimento non è interrotto".into())),
            None => self
                .restored
                .lock()
                .unwrap()
                .get(&job_id)
                .cloned()
                .ok_or_else(|| AppError::Other("trasferimento non trovato".into()))?,
        };
        let info = &saved.info;
        let same_dst = info.dst_connection_id.as_deref() == dst.as_ref().map(|d| d.connection_id.as_str());
        if info.connection_id != src.connection_id || !same_dst {
            let both = info.dst_host.as_ref().map(|h| format!(" e {h}")).unwrap_or_default();
            return Err(AppError::Other(format!("per riprendere servono le connessioni {}{both}", info.host)));
        }
        self.jobs.lock().unwrap().remove(&job_id);
        self.restored.lock().unwrap().remove(&job_id);
        let mut info = saved.info;
        info.host = src.session.launcher.label.clone();
        info.dst_host = dst.as_ref().map(|d| d.session.launcher.label.clone());
        info.errors.clear();
        Ok(self.spawn_job(info, src, dst, saved.policy, saved.parts, true))
    }

    fn spawn_job(
        self: &Arc<Self>,
        mut info: JobInfo,
        src: Endpoint,
        dst: Option<Endpoint>,
        policy: Policy,
        parts: HashMap<String, PartPoint>,
        resumed: bool,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        info.id = id;
        info.session_id = Some(src.session_id);
        info.dst_session_id = dst.as_ref().map(|d| d.session_id);
        info.state = JobState::Scanning;
        info.total_bytes = 0;
        info.done_bytes = 0;
        info.total_files = 0;
        info.done_files = 0;
        info.skipped_files = 0;
        info.failed_files = 0;
        info.started_at = now_ms();
        info.finished_at = None;
        info.resumable = false;
        let job = Arc::new(Job {
            session: src.session,
            dst_session: dst.map(|d| d.session),
            info: Mutex::new(info),
            done_bytes: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            dirty: AtomicBool::new(true),
            policy: tokio::sync::Mutex::new(policy),
            running: Mutex::new(Vec::new()),
            resumed,
            parts: Mutex::new(parts),
        });
        self.jobs.lock().unwrap().insert(id, job.clone());
        self.save_manifest();
        let manager = self.clone();
        tauri::async_runtime::spawn(async move { manager.run(job).await });
        id
    }

    async fn run(self: Arc<Self>, job: Arc<Job>) {
        let (host, direction, label, src_dir, dst_dir, names) = {
            let info = job.info.lock().unwrap();
            let dst = info.dst_host.as_ref().map(|h| format!("{h}:")).unwrap_or_default();
            let label = format!(
                "{} {} {} → {dst}{}",
                match info.direction {
                    Direction::Upload => "upload",
                    Direction::Download => "download",
                    Direction::Remote => "copy",
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
                    i.resumable = true;
                });
                self.save_manifest();
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
        // The part files are final now: record them before the end becomes
        // visible, then again with the final state.
        self.save_manifest();
        job.update(|i| {
            i.state = state;
            i.finished_at = Some(now_ms());
            i.resumable = state != JobState::Done;
        });
        self.save_manifest();
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
        let host = job.session.launcher.label.clone();
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
            Direction::Download | Direction::Remote => scan_remote(job, src_dir, names).await,
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
        let host = job.session.launcher.label.clone();
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
            Direction::Upload | Direction::Remote => {
                let session = job.target();
                let mut fs = session.sftp.fs();
                match fs.metadata(path).await {
                    Ok(meta) if meta.file_type().is_some_and(|t| t.is_dir()) => return Ok(()),
                    Ok(_) => return Err(AppError::Other("esiste già un file con lo stesso nome".into())),
                    Err(_) => {}
                }
                let mut op = self.logger.start(Some(&session.launcher.label), Channel::Sftp, "mkdir", path);
                op.equivalent(session.launcher.sftp_equivalent("mkdir", &[path]));
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
            Direction::Upload | Direction::Remote => job.target().sftp.fs().metadata(dst).await.ok().map(|m| {
                (
                    m.file_type().is_some_and(|t| t.is_dir()),
                    m.len().unwrap_or(0),
                    m.modified().map(|t| t.as_duration().as_secs()),
                )
            }),
        };
        let part = part_name(dst);
        // Part file left by the interrupted run, if any.
        let point = job.parts.lock().unwrap().remove(&file.rel);
        let host = job.session.launcher.label.clone();
        if let Some((is_dir, size, modified)) = existing {
            if is_dir {
                return Err(AppError::Other("esiste già una cartella con lo stesso nome".into()));
            }
            // Resume: a destination matching the source (transfers keep the
            // mtime) was completed before the interruption.
            if job.resumed && size == file.size && modified.is_some() && modified == file.modified {
                if point.is_some() {
                    self.remove_part(job, direction, &part).await;
                }
                job.add_bytes(file.size);
                self.logger.start(Some(&host), Channel::Local, "resume-skip", dst).ok(Some("già completo".into()));
                return Ok(true);
            }
            if !self.resolve_conflict(job, direction, file, (src, dst), (size, modified)).await? {
                return Ok(false);
            }
        }
        let offset = match point {
            Some(p) if p.size == file.size && p.modified == file.modified && resumable(job, direction) => {
                self.part_len(job, direction, &part).await.map_or(0, |len| len.min(p.offset))
            }
            _ => 0,
        };

        job.running.lock().unwrap().push(file.rel.clone());
        let resumed_from = if offset > 0 { format!(" (ripreso da {})", format_bytes(offset)) } else { String::new() };
        let (op_name, detail, equivalent) = match direction {
            Direction::Upload => (
                "upload",
                format!("{src} → {dst}{resumed_from}"),
                job.session.launcher.sftp_equivalent(if offset > 0 { "reput" } else { "put" }, &[src, dst]),
            ),
            Direction::Download => (
                "download",
                format!("{src} → {dst}{resumed_from}"),
                job.session.launcher.sftp_equivalent(if offset > 0 { "reget" } else { "get" }, &[src, dst]),
            ),
            Direction::Remote => (
                "copy",
                format!("{src} → {}:{dst}{resumed_from}", job.target().launcher.label),
                copy_equivalent(job, src, dst),
            ),
        };
        let mut op = self.logger.start(Some(&host), Channel::Sftp, op_name, detail);
        op.equivalent(equivalent);
        let started = Instant::now();
        let committed = AtomicU64::new(0);
        let result = match direction {
            Direction::Download => download(job, src, Path::new(dst), file, offset, &committed).await,
            Direction::Upload => upload(job, Path::new(src), dst, file, offset, &committed).await,
            Direction::Remote => remote_copy(job, src, dst, file, offset, &committed).await,
        };
        job.running.lock().unwrap().retain(|r| r != &file.rel);
        if result.is_err() {
            // Keep what is known to be written for a resume, else clean up.
            match committed.load(Ordering::Relaxed) {
                0 => self.remove_part(job, direction, &part).await,
                offset => {
                    let point = PartPoint { offset, size: file.size, modified: file.modified };
                    job.parts.lock().unwrap().insert(file.rel.clone(), point);
                }
            }
        }
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

    async fn part_len(&self, job: &Job, direction: Direction, part: &str) -> Option<u64> {
        match direction {
            Direction::Download => tokio::fs::metadata(part).await.ok().map(|m| m.len()),
            Direction::Upload | Direction::Remote => job.target().sftp.fs().metadata(part).await.ok()?.len(),
        }
    }

    async fn remove_part(&self, job: &Job, direction: Direction, part: &str) {
        let _ = match direction {
            Direction::Download => tokio::fs::remove_file(part).await.map_err(AppError::from),
            Direction::Upload | Direction::Remote => job.target().sftp.fs().remove_file(part).await.map_err(AppError::from),
        };
    }

    async fn resolve_conflict(
        &self,
        job: &Arc<Job>,
        direction: Direction,
        file: &PlannedFile,
        (src, dst): (&str, &str),
        (dst_size, dst_modified): (u64, Option<u64>),
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
            // The destination: that is where an overwrite happens.
            host: job.target().launcher.label.clone(),
            direction,
            path: dst.to_string(),
            src: src.to_string(),
            commands: overwrite_commands(job, direction, src, dst),
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

/// The requests of an overwrite: copy to `.gf-part`, then replace.
fn overwrite_commands(job: &Job, direction: Direction, src: &str, dst: &str) -> Vec<String> {
    use crate::log::sftp_quote as q;
    let part = part_name(dst);
    let replace = |lines: &mut Vec<String>| {
        if job.target().sftp.support_posix_rename() {
            lines.push(format!("sftp> rename {} {}   # posix-rename: sostituisce in un passo", q(&part), q(dst)));
        } else {
            lines.push(format!("sftp> rm {}", q(dst)));
            lines.push(format!("sftp> rename {} {}", q(&part), q(dst)));
        }
    };
    match direction {
        Direction::Upload => {
            let mut lines = vec![format!("sftp> put {} {}", q(src), q(&part))];
            replace(&mut lines);
            lines
        }
        Direction::Remote => {
            let (src_host, dst_host) = (&job.session.launcher.label, &job.target().launcher.label);
            let mut lines = match job.copy_strategy() {
                CopyStrategy::CopyData => vec![format!("sftp> cp {} {}   # copy-data: copia sul server", q(src), q(&part))],
                CopyStrategy::Shell => vec![crate::session::sh_command(r#"cp -p -- "$1" "$2""#, &[src, &part])],
                CopyStrategy::Stream => vec![
                    format!("{src_host}: sftp> get {}   # letto in streaming", q(src)),
                    format!("{dst_host}: sftp> put … {}   # scritto senza passare dal disco locale", q(&part)),
                ],
            };
            replace(&mut lines);
            lines
        }
        Direction::Download => vec![
            format!("sftp> get {} {}", q(src), q(&part)),
            format!("mv -f {}", shell_line([&part, &dst.to_string()])),
        ],
    }
}

pub(crate) fn part_name(path: &str) -> String {
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

/// Returns the offset and length written.
async fn write_at(mut file: SftpFile, offset: u64, data: Bytes) -> Result<(u64, u64)> {
    Pin::new(&mut file).start_seek(SeekFrom::Start(offset))?;
    file.write_all(&data).await?;
    Ok((offset, data.len() as u64))
}

/// End of the part of a file known to be written: writes complete out of
/// order, so there can be holes past it.
struct WrittenPrefix {
    end: u64,
    /// Completed writes beyond `end`: offset → length.
    ahead: BTreeMap<u64, u64>,
}

impl WrittenPrefix {
    fn new(start: u64) -> Self {
        Self { end: start, ahead: BTreeMap::new() }
    }

    fn complete(&mut self, offset: u64, len: u64) -> u64 {
        self.ahead.insert(offset, len);
        while let Some(len) = self.ahead.remove(&self.end) {
            self.end += len;
        }
        self.end
    }
}

/// Opens the remote part file: from scratch, or kept as is to continue at `offset`.
async fn open_remote_part(session: &Session, part: &str, offset: u64) -> Result<SftpFile> {
    let mut options = session.sftp.options();
    options.write(true);
    if offset == 0 {
        options.create(true).truncate(true);
    }
    Ok(options.open(part).await?)
}

/// `offset`: bytes of the part file to keep (resume). `committed` tracks the
/// length known to be written, for a later resume if this fails.
async fn download(job: &Job, src: &str, dst: &Path, file: &PlannedFile, offset: u64, committed: &AtomicU64) -> Result<u64> {
    let part = PathBuf::from(part_name(&dst.to_string_lossy()));
    let bytes = download_to(job, src, &part, offset, committed).await?;
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

async fn download_to(job: &Job, src: &str, part: &Path, start: u64, committed: &AtomicU64) -> Result<u64> {
    let remote = job.session.sftp.open(src).await?;
    let chunk = CHUNK_LEN;
    let mut out = if start > 0 {
        let mut out = tokio::fs::OpenOptions::new().write(true).open(part).await?;
        out.set_len(start).await?;
        out.seek(SeekFrom::Start(start)).await?;
        out
    } else {
        tokio::fs::File::create(part).await?
    };
    job.add_bytes(start);
    committed.store(start, Ordering::Relaxed);
    let mut offset = start;
    let mut written = start;
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
        // Sequential writes: everything up to here is in the file.
        committed.store(written, Ordering::Relaxed);
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

async fn upload(job: &Job, src: &Path, dst: &str, file: &PlannedFile, offset: u64, committed: &AtomicU64) -> Result<u64> {
    let part = part_name(dst);
    let existed = job.session.sftp.fs().metadata(dst).await.is_ok();
    let result = upload_to(job, src, &part, offset, committed).await;
    finish_remote(&job.session, result, &part, dst, existed, file).await
}

/// Puts a completed `.gf-part` in place on `session`: mtime, mode (new
/// files only), then rename (posix-rename replaces in one step). On error
/// the part file stays: the caller keeps it for a resume or removes it.
async fn finish_remote(
    session: &Session,
    copied: Result<u64>,
    part: &str,
    dst: &str,
    existed: bool,
    file: &PlannedFile,
) -> Result<u64> {
    let mut fs = session.sftp.fs();
    let bytes = copied?;
    {
        {
            let mut meta = MetaDataBuilder::new();
            if let Some(mtime) = file.modified.and_then(|s| u32::try_from(s).ok()).and_then(UnixTimeStamp::from_raw) {
                meta.time(mtime, mtime);
            }
            if let (false, Some(mode)) = (existed, file.permissions) {
                meta.permissions(Permissions::from_raw(RawPermissions::from_bits_truncate(mode)));
            }
            let _ = fs.set_metadata(part, meta.create()).await;
            if existed && !session.sftp.support_posix_rename() {
                fs.remove_file(dst).await?;
            }
            fs.rename(part, dst).await?;
            Ok(bytes)
        }
    }
}

/// Within one session: refuses copying onto itself or into its own subtree.
pub fn check_same_session_copy(src_dir: &str, names: &[String], dst_dir: &str) -> Result<()> {
    let trim = |p: &str| p.trim_end_matches('/').to_string();
    if trim(src_dir) == trim(dst_dir) {
        return Err(AppError::Other("origine e destinazione sono la stessa cartella".into()));
    }
    for name in names {
        let src = join(&trim(src_dir), name);
        if trim(dst_dir) == src || trim(dst_dir).starts_with(&format!("{src}/")) {
            return Err(AppError::Other(format!("impossibile copiare {src} dentro sé stessa")));
        }
    }
    Ok(())
}

/// Can a file of this job continue from a part file? Server-side copies cannot.
fn resumable(job: &Job, direction: Direction) -> bool {
    direction != Direction::Remote || job.copy_strategy() == CopyStrategy::Stream
}

pub fn copy_strategy(src: &Arc<Session>, dst: &Arc<Session>) -> CopyStrategy {
    if !Arc::ptr_eq(src, dst) {
        CopyStrategy::Stream
    } else {
        // Test hook: `GF_COPY_STRATEGY=shell|stream` exercises the fallbacks
        // on a server that has copy-data.
        let forced = std::env::var("GF_COPY_STRATEGY").unwrap_or_default();
        let has_cp = src.capabilities.commands.iter().any(|c| c == "cp");
        if src.sftp.support_copy() && forced.is_empty() {
            CopyStrategy::CopyData
        } else if has_cp && forced != "stream" {
            CopyStrategy::Shell
        } else {
            CopyStrategy::Stream
        }
    }
}

/// Terminal equivalent of a remote copy.
fn copy_equivalent(job: &Job, src: &str, dst: &str) -> String {
    match job.copy_strategy() {
        CopyStrategy::CopyData => job.session.launcher.sftp_equivalent("cp", &[src, dst]),
        CopyStrategy::Shell => crate::session::sh_command(r#"cp -p -- "$1" "$2""#, &[src, dst]),
        CopyStrategy::Stream => shell_line([
            "scp".to_string(),
            "-3".into(),
            format!("{}:{src}", job.session.launcher.alias),
            format!("{}:{dst}", job.target().launcher.alias),
        ]),
    }
}

/// Only streamed copies resume (`offset`): server-side ones restart.
async fn remote_copy(job: &Job, src: &str, dst: &str, file: &PlannedFile, offset: u64, committed: &AtomicU64) -> Result<u64> {
    let target = job.target();
    let part = part_name(dst);
    let existed = target.sftp.fs().metadata(dst).await.is_ok();
    let copied = match job.copy_strategy() {
        CopyStrategy::CopyData => copy_data(job, src, &part).await,
        CopyStrategy::Shell => copy_shell(job, src, &part).await,
        CopyStrategy::Stream => relay_to(job, src, &part, offset, committed).await,
    };
    finish_remote(target, copied, &part, dst, existed, file).await
}

/// One `copy-data` request: the server copies the whole file.
async fn copy_data(job: &Job, src: &str, part: &str) -> Result<u64> {
    let sftp = &job.session.sftp;
    let mut from = sftp.open(src).await?;
    let mut to = sftp.options().write(true).create(true).truncate(true).open(part).await?;
    from.copy_all_to(&mut to).await?;
    to.close().await?;
    let size = sftp.fs().metadata(part).await?.len().unwrap_or(0);
    job.add_bytes(size);
    Ok(size)
}

async fn copy_shell(job: &Job, src: &str, part: &str) -> Result<u64> {
    let command = crate::session::sh_command(r#"cp -p -- "$1" "$2""#, &[src, part]);
    let output = job.session.launcher.exec("cp", &command).await?;
    if !output.status.success() {
        return Err(AppError::Other(String::from_utf8_lossy(&output.stderr).trim().to_string()));
    }
    let size = job.session.sftp.fs().metadata(part).await?.len().unwrap_or(0);
    job.add_bytes(size);
    Ok(size)
}

/// Streams `src` (source session) into `part` (destination session):
/// reads and writes both keep `REQUESTS_IN_FLIGHT` requests going, and at
/// most that many chunks are held in memory.
async fn relay_to(job: &Job, src: &str, part: &str, start: u64, committed: &AtomicU64) -> Result<u64> {
    let input = job.session.sftp.open(src).await?;
    let output = open_remote_part(job.target(), part, start).await?;
    let chunk = CHUNK_LEN;
    let (mut read_offset, mut write_offset, mut written) = (start, start, start);
    let mut prefix = WrittenPrefix::new(start);
    job.add_bytes(start);
    committed.store(start, Ordering::Relaxed);
    let mut eof = false;
    let mut reads = FuturesOrdered::new();
    let mut writes = FuturesUnordered::new();
    loop {
        while !eof && reads.len() < REQUESTS_IN_FLIGHT {
            reads.push_back(read_at(input.clone(), read_offset, chunk));
            read_offset += chunk as u64;
        }
        while writes.len() >= REQUESTS_IN_FLIGHT {
            let Some(done) = writes.next().await else { break };
            let (at, n): (u64, u64) = done?;
            written += n;
            job.add_bytes(n);
            committed.store(prefix.complete(at, n), Ordering::Relaxed);
        }
        let Some(data) = reads.next().await else { break };
        let data = data?;
        job.check_cancelled()?;
        let len = data.len();
        if len > 0 {
            writes.push(write_at(output.clone(), write_offset, data.freeze()));
            write_offset += len as u64;
        }
        if len < chunk as usize {
            eof = true;
            reads = FuturesOrdered::new();
        }
    }
    while let Some(done) = writes.next().await {
        let (at, n) = done?;
        written += n;
        job.add_bytes(n);
        committed.store(prefix.complete(at, n), Ordering::Relaxed);
    }
    output.close().await?;
    drop(input);
    Ok(written)
}

async fn upload_to(job: &Job, src: &Path, part: &str, start: u64, committed: &AtomicU64) -> Result<u64> {
    let mut local = tokio::fs::File::open(src).await?;
    local.seek(SeekFrom::Start(start)).await?;
    let remote = open_remote_part(&job.session, part, start).await?;
    let chunk = CHUNK_LEN as usize;
    let mut offset = start;
    let mut written = start;
    let mut prefix = WrittenPrefix::new(start);
    job.add_bytes(start);
    committed.store(start, Ordering::Relaxed);
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
        let (at, n) = result?;
        written += n;
        job.add_bytes(n);
        committed.store(prefix.complete(at, n), Ordering::Relaxed);
        job.check_cancelled()?;
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
    fn written_prefix_ignores_holes() {
        let mut prefix = WrittenPrefix::new(100);
        // Writes at 100, 110, 120 (10 bytes each) complete out of order.
        assert_eq!(prefix.complete(120, 10), 100);
        assert_eq!(prefix.complete(100, 10), 110);
        assert_eq!(prefix.complete(110, 10), 130);
        assert_eq!(prefix.complete(140, 5), 130);
    }

    #[test]
    fn scans_local_tree_parents_first_and_skips_dir_symlinks() {
        let dir = std::env::temp_dir().join(format!("gf-scan-{}", std::process::id()));
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
    fn refuses_copies_onto_themselves() {
        let names = vec!["tree".to_string()];
        assert!(check_same_session_copy("/a", &names, "/a/").is_err());
        assert!(check_same_session_copy("/a", &names, "/a/tree/sub").is_err());
        assert!(check_same_session_copy("/a", &names, "/a/treeish").is_ok());
        assert!(check_same_session_copy("/a", &names, "/b").is_ok());
    }

    #[test]
    fn formats_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
