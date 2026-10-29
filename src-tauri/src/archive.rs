//! Compressing a selection into a `.tar.gz` and extracting archives, with
//! `tar` (and `unzip` for `.zip`) on the server, or on this machine for the
//! local pane. Nothing is downloaded. Nothing is overwritten: the archive
//! must not exist, and an archive is extracted into a new folder named
//! after it (members with absolute paths or `..` are refused by tar).

use std::sync::Arc;

use serde::Serialize;

use crate::error::{AppError, Result};
use crate::fileops::validate_name;
use crate::log::{shell_line, Channel, Logger};
use crate::session::{sh_command, Session};

/// Names travel as `./name`, so a name starting with `-` is not an option.
/// A failed archive is removed.
const COMPRESS: &str = r#"out=$1; dir=$2; shift 2; tar -czf "$out" -C "$dir" "$@" || { rm -f -- "$out"; exit 1; }"#;
/// The folder is created first (it must not exist), then filled.
const EXTRACT_TAR: &str = r#"mkdir -- "$2" && tar -xf "$1" -C "$2""#;
const EXTRACT_ZIP: &str = r#"mkdir -- "$2" && unzip -q "$1" -d "$2""#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveKind {
    Tar,
    Zip,
}

const SUFFIXES: &[(&str, ArchiveKind)] = &[
    (".tar.gz", ArchiveKind::Tar),
    (".tgz", ArchiveKind::Tar),
    (".tar.bz2", ArchiveKind::Tar),
    (".tbz2", ArchiveKind::Tar),
    (".tar.xz", ArchiveKind::Tar),
    (".txz", ArchiveKind::Tar),
    (".tar.zst", ArchiveKind::Tar),
    (".tar", ArchiveKind::Tar),
    (".zip", ArchiveKind::Zip),
];

/// The archive type and the name without its suffix (the extraction folder).
pub fn split(name: &str) -> Option<(ArchiveKind, &str)> {
    let lower = name.to_lowercase();
    SUFFIXES.iter().find_map(|(suffix, kind)| {
        // ASCII suffix: lowercasing does not change the byte length.
        let stem = name.get(..name.len().checked_sub(suffix.len())?)?;
        (lower.ends_with(suffix) && !stem.is_empty()).then_some((*kind, stem))
    })
}

fn join(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

/// `[archive, dir, ./name...]` for `COMPRESS`.
fn compress_args(dir: &str, names: &[String], archive: &str) -> Result<Vec<String>> {
    validate_name(archive)?;
    if !archive.to_lowercase().ends_with(".tar.gz") && !archive.to_lowercase().ends_with(".tgz") {
        return Err(AppError::Other("il nome dell'archivio deve finire con .tar.gz".into()));
    }
    if names.is_empty() {
        return Err(AppError::Other("niente da comprimere".into()));
    }
    let mut args = vec![join(dir, archive), dir.to_string()];
    for name in names {
        validate_name(name)?;
        if name == archive {
            return Err(AppError::Other("l'archivio non può avere il nome di un elemento selezionato".into()));
        }
        args.push(format!("./{name}"));
    }
    Ok(args)
}

/// Archive type, script, `[archive, folder]` and the folder's name.
fn extract_plan(dir: &str, name: &str) -> Result<(ArchiveKind, &'static str, Vec<String>, String)> {
    validate_name(name)?;
    let (kind, stem) = split(name).ok_or_else(|| AppError::Other(format!("{name} non è un archivio riconosciuto")))?;
    let script = match kind {
        ArchiveKind::Tar => EXTRACT_TAR,
        ArchiveKind::Zip => EXTRACT_ZIP,
    };
    Ok((kind, script, vec![join(dir, name), join(dir, stem)], stem.to_string()))
}

fn tool(kind: ArchiveKind) -> &'static str {
    match kind {
        ArchiveKind::Tar => "tar",
        ArchiveKind::Zip => "unzip",
    }
}

pub mod remote {
    use super::*;

    fn require(session: &Session, command: &str) -> Result<()> {
        if session.capabilities.commands.iter().any(|c| c == command) {
            Ok(())
        } else {
            Err(AppError::Other(format!("serve `{command}` sul server, che qui non è disponibile (account solo SFTP?)")))
        }
    }

    async fn run(session: &Session, op: &str, script: &str, args: &[String]) -> Result<()> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = session.launcher.exec(op, &sh_command(script, &args)).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(AppError::Other(String::from_utf8_lossy(&output.stderr).trim().to_string()))
        }
    }

    async fn exists(session: &Session, path: &str) -> bool {
        session.sftp.fs().symlink_metadata(path).await.is_ok()
    }

    pub async fn compress(session: &Session, dir: &str, names: &[String], archive: &str) -> Result<()> {
        require(session, "tar")?;
        let args = compress_args(dir, names, archive)?;
        if exists(session, &args[0]).await {
            return Err(AppError::Other(format!("{archive} esiste già")));
        }
        run(session, "tar", COMPRESS, &args).await
    }

    /// Returns the name of the folder created.
    pub async fn extract(session: &Session, dir: &str, name: &str) -> Result<String> {
        let (kind, script, args, folder) = extract_plan(dir, name)?;
        require(session, tool(kind))?;
        if exists(session, &args[1]).await {
            return Err(AppError::Other(format!("la cartella {folder} esiste già")));
        }
        run(session, tool(kind), script, &args).await?;
        Ok(folder)
    }
}

pub mod local {
    use super::*;

    async fn run(logger: &Arc<Logger>, op_name: &str, script: &str, args: &[String]) -> Result<()> {
        let mut op = logger.start(None, Channel::Local, op_name, args.join(" "));
        let mut line = vec!["sh".to_string(), "-c".into(), script.to_string(), "sh".into()];
        line.extend(args.iter().cloned());
        op.equivalent(shell_line(&line));
        let result = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .arg("sh")
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(AppError::from)
            .and_then(|output| {
                if output.status.success() {
                    Ok(())
                } else {
                    Err(AppError::Other(String::from_utf8_lossy(&output.stderr).trim().to_string()))
                }
            });
        op.result(result, |_| None)
    }

    pub async fn compress(logger: &Arc<Logger>, dir: &str, names: &[String], archive: &str) -> Result<()> {
        let args = compress_args(dir, names, archive)?;
        if std::fs::symlink_metadata(&args[0]).is_ok() {
            return Err(AppError::Other(format!("{archive} esiste già")));
        }
        run(logger, "tar", COMPRESS, &args).await
    }

    pub async fn extract(logger: &Arc<Logger>, dir: &str, name: &str) -> Result<String> {
        let (kind, script, args, folder) = extract_plan(dir, name)?;
        if std::fs::symlink_metadata(&args[1]).is_ok() {
            return Err(AppError::Other(format!("la cartella {folder} esiste già")));
        }
        run(logger, tool(kind), script, &args).await?;
        Ok(folder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_archives() {
        assert_eq!(split("site.tar.gz"), Some((ArchiveKind::Tar, "site")));
        assert_eq!(split("Backup.TGZ"), Some((ArchiveKind::Tar, "Backup")));
        assert_eq!(split("a.b.zip"), Some((ArchiveKind::Zip, "a.b")));
        assert_eq!(split(".tar.gz"), None);
        assert_eq!(split("notes.txt"), None);
        assert_eq!(split("a"), None);
        assert_eq!(split("città.zip"), Some((ArchiveKind::Zip, "città")));
    }

    #[test]
    fn validates_compress_arguments() {
        let names = vec!["-rf".to_string(), "b c".to_string()];
        let args = compress_args("/srv", &names, "out.tar.gz").unwrap();
        assert_eq!(args, vec!["/srv/out.tar.gz", "/srv", "./-rf", "./b c"]);
        assert!(compress_args("/srv", &names, "out.zip").is_err());
        assert!(compress_args("/srv", &names, "../out.tar.gz").is_err());
        assert!(compress_args("/srv", &["out.tar.gz".into()], "out.tar.gz").is_err());
    }

    #[tokio::test]
    async fn compresses_and_extracts_locally() {
        let dir = std::env::temp_dir().join(format!("gf-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("site/sub")).unwrap();
        std::fs::write(dir.join("site/sub/a.txt"), "alpha").unwrap();
        std::fs::write(dir.join("-dash"), "dash").unwrap();
        let root = dir.to_string_lossy().into_owned();
        let logger = Logger::new(None, None);
        let names = vec!["site".to_string(), "-dash".to_string()];
        local::compress(&logger, &root, &names, "pack.tar.gz").await.unwrap();
        assert!(local::compress(&logger, &root, &names, "pack.tar.gz").await.is_err(), "never overwrites");
        let folder = local::extract(&logger, &root, "pack.tar.gz").await.unwrap();
        assert_eq!(folder, "pack");
        assert_eq!(std::fs::read_to_string(dir.join("pack/site/sub/a.txt")).unwrap(), "alpha");
        assert_eq!(std::fs::read_to_string(dir.join("pack/-dash")).unwrap(), "dash");
        assert!(local::extract(&logger, &root, "pack.tar.gz").await.is_err(), "folder exists");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
