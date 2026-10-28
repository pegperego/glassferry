//! Reads host aliases from the user's OpenSSH config.
//!
//! We never interpret the config ourselves beyond collecting `Host` aliases:
//! the effective settings of a host always come from `ssh -G <alias>`
//! (`connections::resolve`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{AppError, Result};

const MAX_INCLUDE_DEPTH: usize = 16;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostEntry {
    pub alias: String,
    /// Config file where the alias was first declared.
    pub source: PathBuf,
}

/// Lists concrete host aliases (no wildcards or negations), following `Include`.
pub fn list_hosts() -> Result<Vec<HostEntry>> {
    let ssh_dir = ssh_dir()?;
    let mut hosts = Vec::new();
    let mut seen_aliases = HashSet::new();
    let mut visited_files = HashSet::new();
    collect(
        &ssh_dir.join("config"),
        &ssh_dir,
        0,
        &mut hosts,
        &mut seen_aliases,
        &mut visited_files,
    );
    Ok(hosts)
}

fn ssh_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".ssh"))
        .ok_or_else(|| AppError::Other("home directory not found".into()))
}

fn collect(
    file: &Path,
    ssh_dir: &Path,
    depth: usize,
    hosts: &mut Vec<HostEntry>,
    seen_aliases: &mut HashSet<String>,
    visited_files: &mut HashSet<PathBuf>,
) {
    if depth > MAX_INCLUDE_DEPTH || !visited_files.insert(file.to_path_buf()) {
        return;
    }
    let Ok(content) = std::fs::read_to_string(file) else {
        return;
    };
    for line in content.lines() {
        let Some((keyword, args)) = split_directive(line) else {
            continue;
        };
        match keyword.to_ascii_lowercase().as_str() {
            "host" => {
                for alias in tokenize(args) {
                    if alias.contains(['*', '?', '!']) {
                        continue;
                    }
                    if seen_aliases.insert(alias.clone()) {
                        hosts.push(HostEntry {
                            alias,
                            source: file.to_path_buf(),
                        });
                    }
                }
            }
            "include" => {
                for pattern in tokenize(args) {
                    for path in expand_include(&pattern, ssh_dir) {
                        collect(&path, ssh_dir, depth + 1, hosts, seen_aliases, visited_files);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Splits `Keyword args` / `Keyword=args`, skipping comments and blank lines.
fn split_directive(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start().strip_prefix('=').unwrap_or(rest).trim();
    Some((keyword, rest))
}

/// Splits arguments on whitespace, honouring double quotes like OpenSSH does.
fn tokenize(args: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in args.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Relative `Include` paths are relative to ~/.ssh; `~` expands to home.
fn expand_include(pattern: &str, ssh_dir: &Path) -> Vec<PathBuf> {
    let expanded = if let Some(rest) = pattern.strip_prefix("~/") {
        match dirs::home_dir() {
            Some(home) => home.join(rest),
            None => return Vec::new(),
        }
    } else if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        ssh_dir.join(pattern)
    };
    let Ok(paths) = glob::glob(&expanded.to_string_lossy()) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = paths.filter_map(|p| p.ok()).filter(|p| p.is_file()).collect();
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_directive_handles_equals_and_comments() {
        assert_eq!(split_directive("Host foo bar"), Some(("Host", "foo bar")));
        assert_eq!(split_directive("  Include = conf.d/*"), Some(("Include", "conf.d/*")));
        assert_eq!(split_directive("# Host x"), None);
        assert_eq!(split_directive(""), None);
    }

    #[test]
    fn tokenize_honours_quotes() {
        assert_eq!(tokenize(r#"a "b c" d"#), vec!["a", "b c", "d"]);
    }

    #[test]
    fn collects_hosts_through_includes() {
        let dir = std::env::temp_dir().join(format!("gf-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("conf.d")).unwrap();
        std::fs::write(
            dir.join("config"),
            "Include conf.d/*\nHost alpha *.wild !neg\n  HostName a\nMatch host beta\n",
        )
        .unwrap();
        std::fs::write(dir.join("conf.d/one"), "Host beta gamma\nHost alpha\n").unwrap();

        let mut hosts = Vec::new();
        collect(
            &dir.join("config"),
            &dir,
            0,
            &mut hosts,
            &mut HashSet::new(),
            &mut HashSet::new(),
        );
        let aliases: Vec<_> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, vec!["beta", "gamma", "alpha"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
