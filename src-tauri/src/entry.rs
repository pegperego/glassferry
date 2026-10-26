//! Directory entry shared by the local and remote panes.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub kind: EntryKind,
    /// For symlinks: whether the target is a directory.
    pub target_is_dir: bool,
    pub size: Option<u64>,
    /// Seconds since the Unix epoch.
    pub modified: Option<u64>,
    /// Permission bits (`0o7777` mask).
    pub permissions: Option<u32>,
}
