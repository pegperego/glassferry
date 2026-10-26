//! Local filesystem access for the left pane.

use std::fs::Metadata;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::entry::{EntryKind, FileEntry};
use crate::error::{AppError, Result};
use crate::log::{shell_line, Channel, Logger};

pub fn home() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| AppError::Other("home directory not found".into()))
}

pub async fn list_dir(logger: &Arc<Logger>, path: PathBuf) -> Result<Vec<FileEntry>> {
    let mut op = logger.start(None, Channel::Local, "readdir", path.to_string_lossy());
    op.equivalent(shell_line(["ls", "-la"]) + " " + &shell_line([&path]));
    let result = tokio::task::spawn_blocking(move || list_dir_blocking(&path))
        .await
        .map_err(|e| AppError::Other(e.to_string()))
        .and_then(|r| r);
    op.result(result, |entries| Some(format!("{} voci", entries.len())))
}

fn list_dir_blocking(path: &Path) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for dirent in std::fs::read_dir(path)? {
        let dirent = dirent?;
        let name = dirent.file_name().to_string_lossy().into_owned();
        // An entry may vanish between readdir and lstat: skip it.
        let Ok(meta) = dirent.path().symlink_metadata() else {
            continue;
        };
        let file_type = meta.file_type();
        let kind = if file_type.is_dir() {
            EntryKind::Dir
        } else if file_type.is_file() {
            EntryKind::File
        } else if file_type.is_symlink() {
            EntryKind::Symlink
        } else {
            EntryKind::Other
        };
        let target_is_dir = kind == EntryKind::Symlink && dirent.path().is_dir();
        entries.push(to_entry(name, kind, target_is_dir, &meta));
    }
    Ok(entries)
}

fn to_entry(name: String, kind: EntryKind, target_is_dir: bool, meta: &Metadata) -> FileEntry {
    FileEntry {
        name,
        kind,
        target_is_dir,
        size: Some(meta.len()),
        modified: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs()),
        permissions: Some(meta.permissions().mode() & 0o7777),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_files_dirs_and_symlinks() {
        let dir = std::env::temp_dir().join(format!("sfm-local-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("file.txt"), b"hello").unwrap();
        std::os::unix::fs::symlink(dir.join("sub"), dir.join("link")).unwrap();

        let mut entries = list_dir_blocking(&dir).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let summary: Vec<_> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.kind, e.target_is_dir))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("file.txt", EntryKind::File, false),
                ("link", EntryKind::Symlink, true),
                ("sub", EntryKind::Dir, false),
            ]
        );
        assert_eq!(entries[0].size, Some(5));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
