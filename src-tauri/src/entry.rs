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
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    /// Names of `uid` and `gid` when known; otherwise the UI shows the numbers.
    pub owner: Option<String>,
    pub group: Option<String>,
}
