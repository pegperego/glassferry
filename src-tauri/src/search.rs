//! Searching files by name below a folder.
//!
//! Remote with a shell: `find` on the server, staying on one filesystem
//! (`-xdev`: no /proc, no network mounts), case-insensitive name match,
//! output cut at the limit. Without a shell (SFTP walk) and locally (std
//! walk): the same rules, symlinks not followed. A search can be cancelled.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::entry::EntryKind;
use crate::error::{AppError, Result};
use crate::log::{Channel, Logger};
use crate::session::{sh_command, Session};

pub const MAX_RESULTS: usize = 1000;
/// Folders a walk visits at most (find on the server has no such limit).
const MAX_WALK_DIRS: usize = 20_000;
const FIND_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SearchKind {
    Any,
    File,
    Dir,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    /// Part of the name, or a pattern with `*` and `?`.
    pub text: String,
    pub kind: SearchKind,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    /// Absolute path.
    pub path: String,
    pub dir: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    /// More matches than `MAX_RESULTS`, or the walk stopped at its folder limit.
    pub truncated: bool,
    /// Some folders could not be read.
    pub partial: bool,
    /// "find", "sftp" or "local".
    pub method: &'static str,
}

/// Cancellation of a running search.
#[derive(Default)]
pub struct Cancel {
    flag: AtomicBool,
    notify: Notify,
}

impl Cancel {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    async fn cancelled(&self) {
        loop {
            // Created before the check: a cancel in between still wakes it.
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// `*text*` unless the text already has wildcards.
pub fn pattern(text: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AppError::Other("scrivi cosa cercare".into()));
    }
    if text.contains('/') {
        return Err(AppError::Other("si cerca per nome: niente / nel testo".into()));
    }
    Ok(if text.contains(['*', '?']) { text.to_string() } else { format!("*{text}*") })
}

/// Case-insensitive match of `*` and `?` (everything else literal).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

fn wanted(kind: SearchKind, dir: bool) -> bool {
    match kind {
        SearchKind::Any => true,
        SearchKind::File => !dir,
        SearchKind::Dir => dir,
    }
}

fn child(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

pub mod remote {
    use super::*;

    pub async fn search(session: &Session, dir: &str, query: &SearchQuery, cancel: &Cancel) -> Result<SearchResult> {
        let pattern = pattern(&query.text)?;
        if session.capabilities.commands.iter().any(|c| c == "find") {
            find(session, dir, &pattern, query.kind, cancel).await
        } else {
            walk(session, dir, &pattern, query.kind, cancel).await
        }
    }

    /// Folders are printed with a trailing `/` (portable: no `-printf`).
    pub(super) fn find_script(kind: SearchKind) -> String {
        let dirs = r#"\( -type d -iname "$2" -exec printf '%s/\n' {} + \)"#;
        let others = r#"\( ! -type d -iname "$2" -print \)"#;
        let tests = match kind {
            SearchKind::Any => format!("{dirs} -o {others}"),
            SearchKind::File => others.to_string(),
            SearchKind::Dir => dirs.to_string(),
        };
        format!(r#"find "$1" -xdev {tests} 2>/dev/null | head -n "$3""#)
    }

    async fn find(session: &Session, dir: &str, pattern: &str, kind: SearchKind, cancel: &Cancel) -> Result<SearchResult> {
        let limit = (MAX_RESULTS + 1).to_string();
        let command = sh_command(&find_script(kind), &[dir, pattern, &limit]);
        let output = tokio::select! {
            output = tokio::time::timeout(FIND_TIMEOUT, session.launcher.exec("find", &command)) => match output {
                Ok(output) => output?,
                Err(_) => return Err(AppError::Other("ricerca interrotta dopo 2 minuti: restringi la cartella o il nome".into())),
            },
            // Dropping the exec future kills ssh (kill_on_drop).
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
        };
        let root = dir.trim_end_matches('/');
        let mut matches: Vec<SearchMatch> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let (path, dir) = match line.strip_suffix('/') {
                    Some(path) if !path.is_empty() => (path, true),
                    _ => (line, false),
                };
                (path != root && !path.is_empty()).then(|| SearchMatch { path: path.to_string(), dir })
            })
            .collect();
        let truncated = matches.len() > MAX_RESULTS;
        matches.truncate(MAX_RESULTS);
        Ok(SearchResult { matches, truncated, partial: false, method: "find" })
    }

    async fn walk(session: &Session, dir: &str, pattern: &str, kind: SearchKind, cancel: &Cancel) -> Result<SearchResult> {
        let mut op = session.launcher.log(Channel::Sftp, "search", format!("{pattern} in {dir}"));
        op.equivalent(session.launcher.sftp_equivalent("ls -R", &[dir]));
        let mut result = SearchResult { matches: Vec::new(), truncated: false, partial: false, method: "sftp" };
        let mut queue = VecDeque::from([dir.to_string()]);
        let mut visited = 0;
        while let Some(current) = queue.pop_front() {
            if cancel.is_cancelled() {
                op.fail("annullata");
                return Err(AppError::Cancelled);
            }
            visited += 1;
            if visited > MAX_WALK_DIRS {
                result.truncated = true;
                break;
            }
            let Ok(entries) = session.read_dir(Path::new(&current)).await else {
                result.partial = true;
                continue;
            };
            for entry in entries {
                let is_dir = entry.kind == EntryKind::Dir;
                let path = child(&current, &entry.name);
                if wanted(kind, is_dir) && glob_match(pattern, &entry.name) {
                    if result.matches.len() == MAX_RESULTS {
                        result.truncated = true;
                        break;
                    }
                    result.matches.push(SearchMatch { path: path.clone(), dir: is_dir });
                }
                if is_dir {
                    queue.push_back(path);
                }
            }
            if result.truncated {
                break;
            }
        }
        op.ok(Some(format!("{} risultati", result.matches.len())));
        Ok(result)
    }
}

pub mod local {
    use super::*;

    pub async fn search(logger: &Arc<Logger>, dir: &str, query: &SearchQuery, cancel: Arc<Cancel>) -> Result<SearchResult> {
        let pattern = pattern(&query.text)?;
        let op = logger.start(None, Channel::Local, "search", format!("{pattern} in {dir}"));
        let (dir, kind) = (dir.to_string(), query.kind);
        let result = tokio::task::spawn_blocking(move || walk(&dir, &pattern, kind, &cancel))
            .await
            .map_err(|e| AppError::Other(e.to_string()))
            .and_then(|r| r);
        op.result(result, |r| Some(format!("{} risultati", r.matches.len())))
    }

    pub(super) fn walk(dir: &str, pattern: &str, kind: SearchKind, cancel: &Cancel) -> Result<SearchResult> {
        use std::os::unix::fs::MetadataExt;
        let device = std::fs::metadata(dir)?.dev();
        let mut result = SearchResult { matches: Vec::new(), truncated: false, partial: false, method: "local" };
        let mut queue = VecDeque::from([dir.to_string()]);
        let mut visited = 0;
        while let Some(current) = queue.pop_front() {
            if cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            visited += 1;
            if visited > MAX_WALK_DIRS {
                result.truncated = true;
                break;
            }
            let Ok(entries) = std::fs::read_dir(&current) else {
                result.partial = true;
                continue;
            };
            for entry in entries.flatten() {
                // DirEntry::metadata does not follow symlinks.
                let Ok(meta) = entry.metadata() else { continue };
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = child(&current, &name);
                if wanted(kind, meta.is_dir()) && glob_match(pattern, &name) {
                    if result.matches.len() == MAX_RESULTS {
                        result.truncated = true;
                        break;
                    }
                    result.matches.push(SearchMatch { path: path.clone(), dir: meta.is_dir() });
                }
                // Same filesystem only, like find -xdev.
                if meta.is_dir() && meta.dev() == device {
                    queue.push_back(path);
                }
            }
            if result.truncated {
                break;
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_patterns_and_matches_names() {
        assert_eq!(pattern(" nginx ").unwrap(), "*nginx*");
        assert_eq!(pattern("*.conf").unwrap(), "*.conf");
        assert!(pattern("  ").is_err());
        assert!(pattern("a/b").is_err());
        assert!(glob_match("*nginx*", "Nginx.conf"));
        assert!(glob_match("*.conf", "site.conf"));
        assert!(!glob_match("*.conf", "site.conf.bak"));
        assert!(glob_match("f?le", "FILE"));
        assert!(!glob_match("f?le", "fle"));
        assert!(glob_match("*a*b*", "xxaYYbzz"));
    }

    #[test]
    fn find_script_filters_by_kind() {
        assert!(remote::find_script(SearchKind::File).starts_with(r#"find "$1" -xdev \( ! -type d"#));
        assert!(remote::find_script(SearchKind::Dir).contains("-type d -iname"));
        assert!(!remote::find_script(SearchKind::Dir).contains("! -type d"));
    }

    #[test]
    fn find_script_runs_on_this_system() {
        let dir = std::env::temp_dir().join(format!("gf-search-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("conf.d/nginx")).unwrap();
        std::fs::write(dir.join("conf.d/Nginx.conf"), "").unwrap();
        std::fs::write(dir.join("other.txt"), "").unwrap();
        let root = dir.to_string_lossy().into_owned();
        let script = remote::find_script(SearchKind::Any);
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &script, "sh", &root, "*nginx*", "10"])
            .output()
            .unwrap();
        let mut lines: Vec<String> = String::from_utf8_lossy(&output.stdout).lines().map(|l| l.replace(&root, "")).collect();
        lines.sort();
        assert_eq!(lines, vec!["/conf.d/Nginx.conf", "/conf.d/nginx/"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn local_walk_finds_and_stops() {
        let dir = std::env::temp_dir().join(format!("gf-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/site.conf"), "").unwrap();
        std::fs::write(dir.join("a/readme"), "").unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("a/loop")).unwrap();
        let root = dir.to_string_lossy().into_owned();
        let found = local::walk(&root, "*.conf", SearchKind::Any, &Cancel::default()).unwrap();
        assert_eq!(found.matches, vec![SearchMatch { path: format!("{root}/a/b/site.conf"), dir: false }]);
        let dirs = local::walk(&root, "*", SearchKind::Dir, &Cancel::default()).unwrap();
        assert_eq!(dirs.matches.len(), 2);
        let cancel = Cancel::default();
        cancel.cancel();
        assert!(matches!(local::walk(&root, "*", SearchKind::Any, &cancel), Err(AppError::Cancelled)));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
