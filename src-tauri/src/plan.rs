//! Previewed file operations.
//!
//! A destructive operation is first *planned*: the plan fixes how it will be
//! carried out (Trash, `rm -rf` through the shell, the list of SFTP requests,
//! renames) and the preview shows exactly those commands. Executing a plan
//! runs what was planned, never a recomputed variant: an SFTP delete runs the
//! listed requests only, so files added in the meantime make it fail instead
//! of being deleted unseen. Plans are single use and expire.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::fileops::{self, remote::SftpStep, join, validate_name, DeleteMethod, Target};
use crate::log::{shell_line, sftp_quote, Channel, Logger};
use crate::session::Session;

/// Lines shown in the preview; the full list goes to the activity log.
const PREVIEW_LINES: usize = 20;
const PLAN_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PlanRequest {
    Delete { dir: String, names: Vec<String> },
    Move { src_dir: String, names: Vec<String>, dst_dir: String },
    Rename { dir: String, from: String, to: String },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PlanMethod {
    Trash,
    Shell,
    Sftp,
    /// Local rename(2).
    LocalRename,
    /// SFTP rename, `mv` as fallback when available.
    SftpRename,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanPreview {
    pub id: u64,
    pub method: PlanMethod,
    /// One sentence on how the operation is carried out.
    pub explanation: String,
    /// Commands as they will run (at most `PREVIEW_LINES`).
    pub lines: Vec<String>,
    /// Lines not shown.
    pub more: usize,
    /// Number of SFTP requests or commands.
    pub total: usize,
}

enum Planned {
    LocalDelete { dir: String, names: Vec<String> },
    ShellDelete { paths: Vec<String> },
    SftpDelete { steps: Vec<SftpStep> },
    Move { src_dir: String, names: Vec<String>, dst_dir: String },
    Rename { dir: String, from: String, to: String },
}

struct Plan {
    target: Target,
    planned: Planned,
    created: Instant,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanOutcome {
    Done,
    Deleted { method: DeleteMethod },
}

#[derive(Default)]
pub struct Planner {
    next_id: AtomicU64,
    plans: Mutex<HashMap<u64, Plan>>,
}

impl Planner {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Builds a plan; `session` is required for remote targets.
    pub async fn plan(
        &self,
        logger: &Arc<Logger>,
        target: Target,
        session: Option<&Session>,
        request: PlanRequest,
    ) -> Result<PlanPreview> {
        let (planned, method, explanation, lines) = match (&request, session) {
            (PlanRequest::Delete { dir, names }, None) => {
                for name in names {
                    validate_name(name)?;
                }
                let paths: Vec<String> = names.iter().map(|n| join(dir, n)).collect();
                let lines = vec![format!("mv {} ~/.Trash/", shell_line(&paths))];
                (
                    Planned::LocalDelete { dir: dir.clone(), names: names.clone() },
                    PlanMethod::Trash,
                    "Spostati nel Cestino: potrai recuperarli.".to_string(),
                    lines,
                )
            }
            (PlanRequest::Delete { dir, names }, Some(session)) => {
                let paths = fileops::remote::delete_paths(session, dir, names)?;
                if fileops::remote::delete_method(session) == DeleteMethod::Shell {
                    let lines = vec![fileops::remote::delete_command(&paths)];
                    (
                        Planned::ShellDelete { paths },
                        PlanMethod::Shell,
                        "Via shell (rm): definitiva e immediata, include tutto il contenuto delle cartelle.".to_string(),
                        lines,
                    )
                } else {
                    let steps = fileops::remote::plan_sftp_delete(session, &paths).await?;
                    let lines = steps.iter().map(|s| format!("sftp> {}", s.command())).collect();
                    (
                        Planned::SftpDelete { steps },
                        PlanMethod::Sftp,
                        "Via SFTP (account senza shell): definitiva, una richiesta per ogni file e cartella.".to_string(),
                        lines,
                    )
                }
            }
            (PlanRequest::Move { src_dir, names, dst_dir }, _) => {
                for name in names {
                    validate_name(name)?;
                }
                let pairs: Vec<(String, String)> =
                    names.iter().map(|n| (join(src_dir, n), join(dst_dir, n))).collect();
                let (method, explanation, lines) = move_lines(session, &pairs);
                (
                    Planned::Move { src_dir: src_dir.clone(), names: names.clone(), dst_dir: dst_dir.clone() },
                    method,
                    explanation,
                    lines,
                )
            }
            (PlanRequest::Rename { dir, from, to }, _) => {
                validate_name(from)?;
                validate_name(to)?;
                let pairs = [(join(dir, from), join(dir, to))];
                let (method, explanation, lines) = move_lines(session, &pairs);
                (
                    Planned::Rename { dir: dir.clone(), from: from.clone(), to: to.clone() },
                    method,
                    explanation,
                    lines,
                )
            }
        };

        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let host = session.map(|s| s.launcher.label.as_str());
        let mut op = logger.start(host, Channel::Local, "plan", format!("{method:?}: {} righe", lines.len()));
        op.output(lines.join("\n"));
        op.ok(Some(format!("piano {id}")));

        let total = lines.len();
        let preview = PlanPreview {
            id,
            method,
            explanation,
            more: total.saturating_sub(PREVIEW_LINES),
            lines: lines.into_iter().take(PREVIEW_LINES).collect(),
            total,
        };
        let mut plans = self.plans.lock().unwrap();
        plans.retain(|_, p| p.created.elapsed() < PLAN_TTL);
        plans.insert(id, Plan { target, planned, created: Instant::now() });
        Ok(preview)
    }

    pub fn discard(&self, id: u64) {
        self.plans.lock().unwrap().remove(&id);
    }

    /// Target of a pending plan (to find the session before executing).
    pub fn target(&self, id: u64) -> Result<Target> {
        self.plans
            .lock()
            .unwrap()
            .get(&id)
            .map(|p| p.target)
            .ok_or_else(expired)
    }

    pub async fn execute(&self, logger: &Arc<Logger>, id: u64, session: Option<&Session>) -> Result<PlanOutcome> {
        let plan = self.plans.lock().unwrap().remove(&id).ok_or_else(expired)?;
        if plan.created.elapsed() >= PLAN_TTL {
            return Err(expired());
        }
        match (plan.planned, session) {
            (Planned::LocalDelete { dir, names }, None) => {
                let method = fileops::local::delete(logger, &dir, &names).await?;
                Ok(PlanOutcome::Deleted { method })
            }
            (Planned::ShellDelete { paths }, Some(session)) => {
                fileops::remote::delete_shell(session, &paths).await?;
                Ok(PlanOutcome::Deleted { method: DeleteMethod::Shell })
            }
            (Planned::SftpDelete { steps }, Some(session)) => {
                fileops::remote::run_sftp_delete(session, &steps).await?;
                Ok(PlanOutcome::Deleted { method: DeleteMethod::Sftp })
            }
            (Planned::Move { src_dir, names, dst_dir }, None) => {
                fileops::local::move_to(logger, &src_dir, &names, &dst_dir).await?;
                Ok(PlanOutcome::Done)
            }
            (Planned::Move { src_dir, names, dst_dir }, Some(session)) => {
                fileops::remote::move_to(session, &src_dir, &names, &dst_dir).await?;
                Ok(PlanOutcome::Done)
            }
            (Planned::Rename { dir, from, to }, None) => {
                fileops::local::rename(logger, &dir, &from, &to).await?;
                Ok(PlanOutcome::Done)
            }
            (Planned::Rename { dir, from, to }, Some(session)) => {
                fileops::remote::rename(session, &dir, &from, &to).await?;
                Ok(PlanOutcome::Done)
            }
            _ => Err(AppError::Other("piano non valido per questo pannello".into())),
        }
    }
}

fn expired() -> AppError {
    AppError::Other("anteprima scaduta: ripeti l'operazione".into())
}

/// Commands of a move or rename, local (`mv -n`) or remote (SFTP rename).
fn move_lines(session: Option<&Session>, pairs: &[(String, String)]) -> (PlanMethod, String, Vec<String>) {
    match session {
        None => (
            PlanMethod::LocalRename,
            "Rinomina sul disco: non sovrascrive, si ferma se la destinazione esiste.".to_string(),
            pairs.iter().map(|(a, b)| format!("mv -n {}", shell_line([a, b]))).collect(),
        ),
        Some(session) => {
            let has_mv = session.capabilities.commands.iter().any(|c| c == "mv");
            let explanation = if has_mv {
                "Via SFTP rename: non sovrascrive, si ferma se la destinazione esiste. Tra filesystem diversi il server rifiuta e si usa mv."
            } else {
                "Via SFTP rename: non sovrascrive, si ferma se la destinazione esiste."
            };
            let mut lines: Vec<String> = pairs
                .iter()
                .map(|(a, b)| format!("sftp> rename {} {}", sftp_quote(a), sftp_quote(b)))
                .collect();
            if has_mv {
                lines.extend(
                    pairs
                        .iter()
                        .map(|(a, b)| format!("(solo se rifiutato) {}", crate::session::sh_command(r#"mv -- "$1" "$2""#, &[a, b]))),
                );
            }
            (PlanMethod::SftpRename, explanation.to_string(), lines)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_plans_preview_and_run_once() {
        let dir = std::env::temp_dir().join(format!("gf-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        let d = dir.to_string_lossy().into_owned();
        let logger = Logger::new(None, None);
        let planner = Planner::new();

        let request = PlanRequest::Rename { dir: d.clone(), from: "a.txt".into(), to: "b.txt".into() };
        let preview = planner.plan(&logger, Target::Local, None, request).await.unwrap();
        assert_eq!(preview.method, PlanMethod::LocalRename);
        assert!(preview.lines[0].starts_with("mv -n "), "{:?}", preview.lines);
        planner.execute(&logger, preview.id, None).await.unwrap();
        assert!(dir.join("b.txt").exists());
        // Single use.
        assert!(planner.execute(&logger, preview.id, None).await.is_err());

        let bad = PlanRequest::Delete { dir: d.clone(), names: vec!["../x".into()] };
        assert!(planner.plan(&logger, Target::Local, None, bad).await.is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parses_requests() {
        let r: PlanRequest =
            serde_json::from_str(r#"{"kind":"move","srcDir":"/a","names":["x"],"dstDir":"/b"}"#).unwrap();
        assert!(matches!(r, PlanRequest::Move { .. }));
    }
}
