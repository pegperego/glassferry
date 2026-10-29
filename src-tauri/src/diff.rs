//! Line diff for the edit conflict dialog: the server's current version
//! against the local copy, in hunks with a few lines of context. Binary or
//! very large files are reported instead of diffed.

use std::time::Duration;

use serde::Serialize;
use similar::{ChangeTag, TextDiff};

/// Larger files are not diffed (the editor's own tools are better there).
pub const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINES: usize = 4000;
const CONTEXT: usize = 3;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LineKind {
    Same,
    /// Only on the server: lost if the local copy overwrites it.
    Server,
    /// Only in the local copy.
    Mine,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: LineKind,
    /// 1-based line numbers on each side.
    pub server_line: Option<usize>,
    pub mine_line: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDiff {
    pub hunks: Vec<Vec<DiffLine>>,
    /// Lines only on the server / only in the local copy.
    pub server_only: usize,
    pub mine_only: usize,
    pub identical: bool,
    pub binary: bool,
    pub too_large: bool,
    /// Cut at `MAX_LINES` lines shown.
    pub truncated: bool,
}

pub fn diff(server: &[u8], mine: &[u8]) -> ConflictDiff {
    if server.len() > MAX_DIFF_BYTES || mine.len() > MAX_DIFF_BYTES {
        return ConflictDiff { too_large: true, ..Default::default() };
    }
    let (Some(server), Some(mine)) = (text(server), text(mine)) else {
        return ConflictDiff { binary: true, ..Default::default() };
    };
    let lines = TextDiff::configure().timeout(Duration::from_secs(2)).diff_lines(server, mine);
    let mut result = ConflictDiff::default();
    let mut shown = 0;
    for group in lines.grouped_ops(CONTEXT) {
        let mut hunk = Vec::new();
        for op in &group {
            for change in lines.iter_changes(op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => LineKind::Same,
                    ChangeTag::Delete => LineKind::Server,
                    ChangeTag::Insert => LineKind::Mine,
                };
                match kind {
                    LineKind::Server => result.server_only += 1,
                    LineKind::Mine => result.mine_only += 1,
                    LineKind::Same => {}
                }
                if shown == MAX_LINES {
                    result.truncated = true;
                    continue;
                }
                shown += 1;
                hunk.push(DiffLine {
                    kind,
                    server_line: change.old_index().map(|i| i + 1),
                    mine_line: change.new_index().map(|i| i + 1),
                    text: change.value().trim_end_matches(['\n', '\r']).to_string(),
                });
            }
        }
        if !hunk.is_empty() {
            result.hunks.push(hunk);
        }
    }
    result.identical = result.server_only == 0 && result.mine_only == 0;
    result
}

/// UTF-8 without NUL bytes; anything else is treated as binary.
fn text(bytes: &[u8]) -> Option<&str> {
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_lines_with_context() {
        let server = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nserver only\n";
        let mine = "a\nb\nC changed\nd\ne\nf\ng\nh\ni\nj\n";
        let d = diff(server.as_bytes(), mine.as_bytes());
        assert_eq!((d.server_only, d.mine_only, d.identical), (2, 1, false));
        assert_eq!(d.hunks.len(), 2, "far apart changes make two hunks");
        let first = &d.hunks[0];
        assert!(first.iter().any(|l| l.kind == LineKind::Server && l.text == "c" && l.server_line == Some(3)));
        assert!(first.iter().any(|l| l.kind == LineKind::Mine && l.text == "C changed" && l.mine_line == Some(3)));
        assert!(d.hunks[1].iter().any(|l| l.kind == LineKind::Server && l.text == "server only"));
    }

    #[test]
    fn reports_binary_large_and_identical() {
        assert!(diff(b"a\0b", b"a").binary);
        assert!(diff(&[0xff, 0xfe], b"a").binary);
        assert!(diff(&vec![b'a'; MAX_DIFF_BYTES + 1], b"a").too_large);
        let same = diff(b"x\ny\n", b"x\ny\n");
        assert!(same.identical && same.hunks.is_empty());
    }
}
