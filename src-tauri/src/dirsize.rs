//! Size of folders, on demand.
//!
//! Remote with a shell: `du` on the server, `du -sb` on Linux (GNU: the
//! apparent size in bytes, like a walk would give), else or as a fallback
//! `du -sk` (disk usage in KiB, an approximation). Without a shell, and locally: the
//! sizes of the files are added up walking the tree, without following
//! symlinks. Unreadable folders make the result partial, not an error.

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;

use crate::entry::EntryKind;
use crate::error::{AppError, Result};
use crate::fileops::validate_name;
use crate::log::{Channel, Logger};
use crate::session::{sh_command, Session};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirSize {
    pub name: String,
    pub bytes: u64,
    /// Disk usage (`du -k`) rather than the sum of file sizes.
    pub approx: bool,
    /// Some content could not be read.
    pub partial: bool,
}

fn paths(dir: &str, names: &[String]) -> Result<Vec<String>> {
    names
        .iter()
        .map(|name| {
            validate_name(name)?;
            Ok(if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") })
        })
        .collect()
}

/// `du -s` output: one `<size>\t<path>` line per argument, in order.
fn parse_du(stdout: &str, count: usize, unit: u64) -> Option<Vec<u64>> {
    let sizes: Vec<u64> = stdout
        .lines()
        .filter_map(|line| line.split('\t').next()?.trim().parse::<u64>().ok())
        .map(|n| n * unit)
        .collect();
    (sizes.len() == count).then_some(sizes)
}

pub mod remote {
    use super::*;

    pub async fn sizes(session: &Session, dir: &str, names: &[String]) -> Result<Vec<DirSize>> {
        let paths = paths(dir, names)?;
        let result = if session.capabilities.commands.iter().any(|c| c == "du") {
            du(session, &paths).await?
        } else {
            None
        };
        let measured = match result {
            Some(measured) => measured,
            None => walk_all(session, &paths).await?,
        };
        Ok(names
            .iter()
            .zip(measured)
            .map(|(name, (bytes, approx, partial))| DirSize { name: name.clone(), bytes, approx, partial })
            .collect())
    }

    /// `None` when neither form of du answered for every path.
    async fn du(session: &Session, paths: &[String]) -> Result<Option<Vec<(u64, bool, bool)>>> {
        let args: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut forms = vec![(r#"du -sk -- "$@""#, 1024, true)];
        // `-b` is GNU: try it only on Linux, so other systems do not log a failure.
        if session.capabilities.os.as_deref() == Some("Linux") {
            forms.insert(0, (r#"du -sb -- "$@""#, 1, false));
        }
        for (script, unit, approx) in forms {
            let output = session.launcher.exec("du", &sh_command(script, &args)).await?;
            // Exit 1 with every total printed: some subfolder was unreadable.
            if let Some(sizes) = parse_du(&String::from_utf8_lossy(&output.stdout), paths.len(), unit) {
                let partial = !output.status.success();
                return Ok(Some(sizes.into_iter().map(|b| (b, approx, partial)).collect()));
            }
        }
        Ok(None)
    }

    async fn walk_all(session: &Session, paths: &[String]) -> Result<Vec<(u64, bool, bool)>> {
        let mut op = session.launcher.log(Channel::Sftp, "du", paths.join(", "));
        op.equivalent(session.launcher.sftp_equivalent("ls -lR", &[&paths[0]]));
        let mut out = Vec::new();
        for path in paths {
            out.push(walk(session, path).await);
        }
        let total: u64 = out.iter().map(|(b, _, _)| b).sum();
        op.ok(Some(crate::transfer::format_bytes(total)));
        Ok(out)
    }

    async fn walk(session: &Session, root: &str) -> (u64, bool, bool) {
        let (mut bytes, mut partial) = (0, false);
        let mut stack = vec![root.to_string()];
        while let Some(dir) = stack.pop() {
            match session.read_dir(Path::new(&dir)).await {
                Ok(entries) => {
                    for entry in entries {
                        match entry.kind {
                            EntryKind::Dir => stack.push(format!("{}/{}", dir.trim_end_matches('/'), entry.name)),
                            EntryKind::File => bytes += entry.size.unwrap_or(0),
                            _ => {}
                        }
                    }
                }
                Err(_) => partial = true,
            }
        }
        (bytes, false, partial)
    }
}

pub mod local {
    use super::*;

    pub async fn sizes(logger: &Arc<Logger>, dir: &str, names: &[String]) -> Result<Vec<DirSize>> {
        let paths = paths(dir, names)?;
        let op = logger.start(None, Channel::Local, "du", paths.join(", "));
        let names = names.to_vec();
        let result = tokio::task::spawn_blocking(move || {
            names
                .into_iter()
                .zip(paths.iter())
                .map(|(name, path)| {
                    let (bytes, partial) = walk(Path::new(path));
                    DirSize { name, bytes, approx: false, partial }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| AppError::Other(e.to_string()));
        op.result(result, |sizes| Some(crate::transfer::format_bytes(sizes.iter().map(|s| s.bytes).sum())))
    }

    pub(super) fn walk(root: &Path) -> (u64, bool) {
        let (mut bytes, mut partial) = (0, false);
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                partial = true;
                continue;
            };
            for entry in entries.flatten() {
                match entry.metadata() {
                    // DirEntry::metadata does not follow symlinks.
                    Ok(meta) if meta.is_dir() => stack.push(entry.path()),
                    Ok(meta) if meta.is_file() => bytes += meta.len(),
                    Ok(_) => {}
                    Err(_) => partial = true,
                }
            }
        }
        (bytes, partial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_du_output() {
        assert_eq!(parse_du("12\t/a\n3\t/b c\n", 2, 1024), Some(vec![12 * 1024, 3 * 1024]));
        assert_eq!(parse_du("12\t/a\n", 2, 1), None);
        assert_eq!(parse_du("", 1, 1), None);
    }

    #[test]
    fn local_walk_adds_files_and_skips_links() {
        let dir = std::env::temp_dir().join(format!("gf-dirsize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tree/sub")).unwrap();
        std::fs::write(dir.join("tree/a"), vec![0u8; 100]).unwrap();
        std::fs::write(dir.join("tree/sub/b"), vec![0u8; 23]).unwrap();
        std::fs::write(dir.join("big"), vec![0u8; 5000]).unwrap();
        std::os::unix::fs::symlink(dir.join("big"), dir.join("tree/link")).unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("tree/sub/loop")).unwrap();
        assert_eq!(local::walk(&dir.join("tree")), (123, false));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
