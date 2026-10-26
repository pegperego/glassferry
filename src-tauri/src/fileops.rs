//! File operations within one side: create folder, rename, move, delete.
//!
//! Rename and move never overwrite: the destination is checked first, since
//! posix-rename (remote) and rename(2) (local) would replace it silently.
//! Local deletes go to the Trash; remote deletes are permanent and use
//! `rm -rf` when the account has a shell, a recursive SFTP walk otherwise.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::entry::EntryKind;
use crate::error::{AppError, Result};
use crate::log::{sftp_equivalent, shell_line, Channel, Logger};
use crate::session::{sh_command, Session};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DeleteMethod {
    /// Moved to the local Trash.
    Trash,
    /// `rm -rf` through an exec channel.
    Shell,
    /// Recursive SFTP removal.
    Sftp,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Target {
    Local,
    Remote { session_id: u64 },
}

/// Rejects names that would escape the directory or address it.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(AppError::Other(format!("nome non valido: {name:?}")));
    }
    Ok(())
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn exists_error(path: &str) -> AppError {
    AppError::Other(format!("esiste già: {path}"))
}

// ---- local -------------------------------------------------------------------

pub mod local {
    use super::*;

    pub async fn mkdir(logger: &Arc<Logger>, dir: &str, name: &str) -> Result<()> {
        validate_name(name)?;
        let path = join(dir, name);
        let mut op = logger.start(None, Channel::Local, "mkdir", &path);
        op.equivalent(format!("mkdir {}", shell_line([&path])));
        op.result(tokio::fs::create_dir(&path).await.map_err(AppError::from), |_| None)
    }

    pub async fn rename(logger: &Arc<Logger>, dir: &str, from: &str, to: &str) -> Result<()> {
        validate_name(from)?;
        validate_name(to)?;
        move_one(logger, "rename", &join(dir, from), &join(dir, to)).await
    }

    pub async fn move_to(logger: &Arc<Logger>, src_dir: &str, names: &[String], dst_dir: &str) -> Result<()> {
        for name in names {
            validate_name(name)?;
            move_one(logger, "move", &join(src_dir, name), &join(dst_dir, name)).await?;
        }
        Ok(())
    }

    async fn move_one(logger: &Arc<Logger>, op_name: &str, from: &str, to: &str) -> Result<()> {
        let mut op = logger.start(None, Channel::Local, op_name, format!("{from} → {to}"));
        op.equivalent(format!("mv -n {}", shell_line([from, to])));
        let result = async {
            if to.starts_with(&format!("{from}/")) {
                return Err(AppError::Other("impossibile spostare una cartella dentro sé stessa".into()));
            }
            // Case-only renames on case-insensitive volumes: same inode.
            if tokio::fs::symlink_metadata(to).await.is_ok() && !same_file(from, to) {
                return Err(exists_error(to));
            }
            tokio::fs::rename(from, to).await.map_err(|e| {
                if e.raw_os_error() == Some(libc::EXDEV) {
                    AppError::Other("spostamento tra volumi diversi non supportato: usa copia ed elimina".into())
                } else {
                    e.into()
                }
            })
        }
        .await;
        op.result(result, |_| None)
    }

    fn same_file(a: &str, b: &str) -> bool {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }

    pub async fn delete(logger: &Arc<Logger>, dir: &str, names: &[String]) -> Result<DeleteMethod> {
        for name in names {
            validate_name(name)?;
        }
        let paths: Vec<PathBuf> = names.iter().map(|n| PathBuf::from(join(dir, n))).collect();
        let mut op = logger.start(
            None,
            Channel::Local,
            "trash",
            paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join(", "),
        );
        op.equivalent(format!("mv {} ~/.Trash/", shell_line(&paths)));
        let result = tokio::task::spawn_blocking(move || {
            let mut context = trash::TrashContext::new();
            #[cfg(target_os = "macos")]
            {
                use trash::macos::{DeleteMethod as MacMethod, TrashContextExtMacos};
                // No Finder automation permission prompt.
                context.set_delete_method(MacMethod::NsFileManager);
            }
            context.delete_all(&paths)
        })
        .await
        .map_err(|e| AppError::Other(e.to_string()))
        .and_then(|r| r.map_err(|e| AppError::Other(format!("cestino: {e}"))));
        op.result(result, |_| Some(format!("{} elementi nel Cestino", names.len())))?;
        Ok(DeleteMethod::Trash)
    }
}

// ---- remote ------------------------------------------------------------------

pub mod remote {
    use super::*;

    fn log_op(session: &Session, op: &str, detail: impl Into<String>) -> crate::log::LogOp {
        session.launcher.logger.start(Some(&session.launcher.alias), Channel::Sftp, op, detail)
    }

    async fn exists(session: &Session, path: &str) -> bool {
        session.sftp.fs().symlink_metadata(path).await.is_ok()
    }

    pub async fn mkdir(session: &Session, dir: &str, name: &str) -> Result<()> {
        validate_name(name)?;
        let path = join(dir, name);
        let mut op = log_op(session, "mkdir", &path);
        op.equivalent(sftp_equivalent(&session.launcher.alias, "mkdir", &[&path]));
        op.result(session.sftp.fs().create_dir(&path).await.map_err(AppError::from), |_| None)
    }

    pub async fn rename(session: &Session, dir: &str, from: &str, to: &str) -> Result<()> {
        validate_name(from)?;
        validate_name(to)?;
        move_one(session, "rename", &join(dir, from), &join(dir, to)).await
    }

    pub async fn move_to(session: &Session, src_dir: &str, names: &[String], dst_dir: &str) -> Result<()> {
        for name in names {
            validate_name(name)?;
            move_one(session, "move", &join(src_dir, name), &join(dst_dir, name)).await?;
        }
        Ok(())
    }

    async fn move_one(session: &Session, op_name: &str, from: &str, to: &str) -> Result<()> {
        if to.starts_with(&format!("{from}/")) {
            return Err(AppError::Other("impossibile spostare una cartella dentro sé stessa".into()));
        }
        // A case-only rename may hit the same file on case-insensitive servers;
        // anything else that exists is a conflict.
        if exists(session, to).await && !from.eq_ignore_ascii_case(to) {
            return Err(exists_error(to));
        }
        let alias = &session.launcher.alias;
        let mut op = log_op(session, op_name, format!("{from} → {to}"));
        op.equivalent(sftp_equivalent(alias, "rename", &[from, to]));
        let result = session.sftp.fs().rename(from, to).await.map_err(AppError::from);
        match result {
            Ok(()) => {
                op.ok(None);
                Ok(())
            }
            // SFTP rename cannot cross filesystems: `mv` can, when available.
            Err(e) if session.capabilities.commands.iter().any(|c| c == "mv") => {
                op.fail(&e);
                let output = session
                    .launcher
                    .exec("mv", &sh_command(r#"mv -- "$1" "$2""#, &[from, to]))
                    .await?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(AppError::Other(String::from_utf8_lossy(&output.stderr).trim().to_string()))
                }
            }
            Err(e) => {
                op.fail(&e);
                Err(e)
            }
        }
    }

    pub async fn delete(session: &Session, dir: &str, names: &[String]) -> Result<DeleteMethod> {
        let mut paths = Vec::with_capacity(names.len());
        for name in names {
            validate_name(name)?;
            let path = join(dir, name);
            // Belt and braces before a recursive delete.
            if path == "/" || Path::new(&path) == session.home {
                return Err(AppError::Other(format!("rifiuto di eliminare {path}")));
            }
            paths.push(path);
        }

        if session.capabilities.commands.iter().any(|c| c == "rm") {
            let args: Vec<&str> = paths.iter().map(String::as_str).collect();
            let output = session
                .launcher
                .exec("rm -rf", &sh_command(r#"rm -rf -- "$@""#, &args))
                .await?;
            if output.status.success() {
                return Ok(DeleteMethod::Shell);
            }
            return Err(AppError::Other(String::from_utf8_lossy(&output.stderr).trim().to_string()));
        }

        for path in &paths {
            let mut op = log_op(session, "rm -r", path);
            op.equivalent(sftp_equivalent(&session.launcher.alias, "rm", &[path]));
            let result = remove_tree(session, path).await;
            op.result(result, |count| Some(format!("{count} elementi rimossi")))?;
        }
        Ok(DeleteMethod::Sftp)
    }

    /// Depth-first removal over SFTP; symlinks are removed, never followed.
    async fn remove_tree(session: &Session, root: &str) -> Result<u64> {
        let mut fs = session.sftp.fs();
        let meta = fs.symlink_metadata(root).await?;
        if !meta.file_type().is_some_and(|t| t.is_dir()) {
            fs.remove_file(root).await?;
            return Ok(1);
        }
        // (path, children already listed)
        let mut stack: Vec<(String, bool)> = vec![(root.to_string(), false)];
        let mut removed = 0;
        while let Some((dir, expanded)) = stack.pop() {
            if expanded {
                fs.remove_dir(&dir).await?;
                removed += 1;
                continue;
            }
            stack.push((dir.clone(), true));
            for child in session.read_dir(Path::new(&dir)).await? {
                let path = join(&dir, &child.name);
                if child.kind == EntryKind::Dir {
                    stack.push((path, false));
                } else {
                    fs.remove_file(&path).await?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_dangerous_names() {
        for bad in ["", ".", "..", "a/b", "/", "x\0y"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_name("ok name.txt").is_ok());
        assert!(validate_name("..hidden").is_ok());
    }

    #[test]
    fn parses_targets() {
        let local: Target = serde_json::from_str(r#"{"kind":"local"}"#).unwrap();
        let remote: Target = serde_json::from_str(r#"{"kind":"remote","sessionId":3}"#).unwrap();
        assert_eq!(local, Target::Local);
        assert_eq!(remote, Target::Remote { session_id: 3 });
    }

    #[tokio::test]
    async fn local_rename_and_move_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("sfm-ops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        std::fs::write(dir.join("b.txt"), b"b").unwrap();
        std::fs::write(dir.join("sub/a.txt"), b"other").unwrap();
        let d = dir.to_string_lossy().into_owned();
        let logger = Logger::new(None, None);

        assert!(local::rename(&logger, &d, "a.txt", "b.txt").await.is_err());
        assert_eq!(std::fs::read(dir.join("b.txt")).unwrap(), b"b");
        local::rename(&logger, &d, "a.txt", "c.txt").await.unwrap();
        assert!(local::move_to(&logger, &d, &["c.txt".into()], &format!("{d}/sub")).await.is_ok());
        assert!(dir.join("sub/c.txt").exists());
        assert!(local::move_to(&logger, &d, &["sub".into()], &format!("{d}/sub")).await.is_err());
        local::mkdir(&logger, &d, "new").await.unwrap();
        assert!(dir.join("new").is_dir());
        assert!(local::mkdir(&logger, &d, "../escape").await.is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
