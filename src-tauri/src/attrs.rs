//! Permissions and ownership of a selection (chmod / chown / chgrp).
//!
//! Remote: through the shell (`chmod`, `chown`, `chgrp`, `-R` when
//! recursive) when the probe found the command; otherwise the mode is set
//! with SFTP setstat, walking directories without following symlinks.
//! Owner and group need the shell: SFTP v3 only knows numeric ids. Local:
//! the mode only, with std (ownership changes need root).
//!
//! `commands` describes exactly what `apply` runs, for the preview.

use std::path::Path;
use std::sync::Arc;

use openssh_sftp_client::metadata::{MetaDataBuilder, Permissions, RawPermissions};
use serde::Deserialize;

use crate::entry::EntryKind;
use crate::error::{AppError, Result};
use crate::fileops::validate_name;
use crate::log::{shell_line, Channel, Logger};
use crate::session::{sh_command, Session};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttrChange {
    /// Permission bits (up to 0o7777); `None`: unchanged.
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub group: Option<String>,
    /// Also everything inside the selected folders.
    pub recursive: bool,
}

impl AttrChange {
    fn validate(&self) -> Result<()> {
        if self.mode.is_none() && self.owner.is_none() && self.group.is_none() {
            return Err(AppError::Other("nessuna modifica".into()));
        }
        if self.mode.is_some_and(|m| m > 0o7777) {
            return Err(AppError::Other("permessi non validi".into()));
        }
        for (label, name) in [("proprietario", &self.owner), ("gruppo", &self.group)] {
            if let Some(name) = name {
                let valid = !name.is_empty()
                    && !name.starts_with('-')
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
                if !valid {
                    return Err(AppError::Other(format!("{label} non valido: {name}")));
                }
            }
        }
        Ok(())
    }

    fn flag(&self) -> &'static str {
        if self.recursive { "-R " } else { "" }
    }

    /// `owner[:group]` for chown, or `None` when only the group changes.
    fn chown_spec(&self) -> Option<String> {
        let owner = self.owner.as_ref()?;
        Some(match &self.group {
            Some(group) => format!("{owner}:{group}"),
            None => owner.clone(),
        })
    }
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

fn has(session: &Session, command: &str) -> bool {
    session.capabilities.commands.iter().any(|c| c == command)
}

/// Shell scripts for the change; mode and names are validated, paths are positional.
fn shell_scripts(change: &AttrChange) -> Vec<(&'static str, String)> {
    let flag = change.flag();
    let mut scripts = Vec::new();
    if let Some(mode) = change.mode {
        // `--` before the operand: BSD chmod takes anything after the mode as a file.
        scripts.push(("chmod", format!(r#"chmod {flag}-- {mode:04o} "$@""#)));
    }
    match (change.chown_spec(), &change.group) {
        (Some(spec), _) => scripts.push(("chown", format!(r#"chown {flag}-- {spec} "$@""#))),
        (None, Some(group)) => scripts.push(("chgrp", format!(r#"chgrp {flag}-- {group} "$@""#))),
        (None, None) => {}
    }
    scripts
}

pub mod remote {
    use super::*;

    /// What `apply` will run, one line per command or SFTP request.
    pub fn commands(session: &Session, dir: &str, names: &[String], change: &AttrChange) -> Result<Vec<String>> {
        change.validate()?;
        let paths = paths(dir, names)?;
        let args: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut lines = Vec::new();
        for (command, script) in shell_scripts(change) {
            if has(session, command) {
                lines.push(sh_command(&script, &args));
            } else if command == "chmod" {
                let mode = change.mode.unwrap_or_default();
                for path in &paths {
                    lines.push(format!("sftp> chmod {mode:o} {}", crate::log::sftp_quote(path)));
                }
                if change.recursive {
                    lines.push("# e lo stesso per ogni file e cartella contenuti (i link non vengono seguiti)".into());
                }
            } else {
                return Err(AppError::Other(format!(
                    "proprietario e gruppo si cambiano con `{command}`, che su questo server non è disponibile (account solo SFTP?)"
                )));
            }
        }
        Ok(lines)
    }

    pub async fn apply(session: &Session, dir: &str, names: &[String], change: &AttrChange) -> Result<Vec<String>> {
        let lines = commands(session, dir, names, change)?;
        let paths = paths(dir, names)?;
        let args: Vec<&str> = paths.iter().map(String::as_str).collect();
        for (command, script) in shell_scripts(change) {
            if has(session, command) {
                let line = sh_command(&script, &args);
                let output = session.launcher.exec(command, &line).await?;
                if !output.status.success() {
                    return Err(AppError::Other(String::from_utf8_lossy(&output.stderr).trim().to_string()));
                }
            } else {
                set_mode_sftp(session, &paths, change.mode.unwrap_or_default(), change.recursive).await?;
            }
        }
        Ok(lines)
    }

    /// setstat on each path; with `recursive`, children are listed before
    /// their folder changes (a mode without x would stop the listing).
    async fn set_mode_sftp(session: &Session, paths: &[String], mode: u32, recursive: bool) -> Result<()> {
        let mut stack: Vec<String> = paths.iter().rev().cloned().collect();
        let mut fs = session.sftp.fs();
        let mut count = 0usize;
        let mut op = session.launcher.log(Channel::Sftp, "chmod", format!("{mode:o} {}", paths.join(", ")));
        op.equivalent(session.launcher.sftp_equivalent("chmod", &[&format!("{mode:o}"), &paths[0]]));
        let result = async {
            while let Some(path) = stack.pop() {
                let meta = fs.symlink_metadata(&path).await?;
                let kind = meta.file_type();
                if kind.is_some_and(|t| t.is_symlink()) {
                    continue;
                }
                if recursive && kind.is_some_and(|t| t.is_dir()) {
                    for child in session.read_dir(Path::new(&path)).await? {
                        if child.kind != EntryKind::Symlink {
                            stack.push(format!("{}/{}", path.trim_end_matches('/'), child.name));
                        }
                    }
                }
                let permissions = Permissions::from_raw(RawPermissions::from_bits_truncate(mode));
                fs.set_metadata(&path, MetaDataBuilder::new().permissions(permissions).create()).await?;
                count += 1;
            }
            Ok::<_, AppError>(())
        }
        .await;
        op.result(result, |_| Some(format!("{count} elementi")))
    }
}

pub mod local {
    use super::*;

    pub fn commands(dir: &str, names: &[String], change: &AttrChange) -> Result<Vec<String>> {
        change.validate()?;
        if change.owner.is_some() || change.group.is_some() {
            return Err(AppError::Other("proprietario e gruppo dei file locali non si cambiano da qui".into()));
        }
        let paths = paths(dir, names)?;
        let mode = change.mode.unwrap_or_default();
        let mut args = vec!["chmod".to_string()];
        if change.recursive {
            args.push("-R".into());
        }
        args.push("--".into());
        args.push(format!("{mode:04o}"));
        args.extend(paths);
        Ok(vec![shell_line(args)])
    }

    pub async fn apply(logger: &Arc<Logger>, dir: &str, names: &[String], change: &AttrChange) -> Result<Vec<String>> {
        let lines = commands(dir, names, change)?;
        let paths = paths(dir, names)?;
        let (mode, recursive) = (change.mode.unwrap_or_default(), change.recursive);
        let mut op = logger.start(None, Channel::Local, "chmod", format!("{mode:o} {}", paths.join(", ")));
        op.equivalent(lines[0].clone());
        let result = tokio::task::spawn_blocking(move || set_mode(&paths, mode, recursive))
            .await
            .map_err(|e| AppError::Other(e.to_string()))
            .and_then(|r| r);
        op.result(result, |count| Some(format!("{count} elementi")))?;
        Ok(lines)
    }

    pub(super) fn set_mode(paths: &[String], mode: u32, recursive: bool) -> Result<usize> {
        use std::os::unix::fs::PermissionsExt;
        let mut stack: Vec<std::path::PathBuf> = paths.iter().rev().map(Into::into).collect();
        let mut count = 0;
        while let Some(path) = stack.pop() {
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if recursive && meta.is_dir() {
                for child in std::fs::read_dir(&path)? {
                    stack.push(child?.path());
                }
            }
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
            count += 1;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(mode: Option<u32>, owner: Option<&str>, group: Option<&str>, recursive: bool) -> AttrChange {
        AttrChange {
            mode,
            owner: owner.map(String::from),
            group: group.map(String::from),
            recursive,
        }
    }

    #[test]
    fn validates_changes() {
        assert!(change(None, None, None, false).validate().is_err());
        assert!(change(Some(0o10000), None, None, false).validate().is_err());
        assert!(change(None, Some("-rf"), None, false).validate().is_err());
        assert!(change(None, Some("www data"), None, false).validate().is_err());
        assert!(change(Some(0o2755), Some("www-data"), Some("www-data"), true).validate().is_ok());
    }

    #[test]
    fn builds_shell_scripts() {
        let scripts = shell_scripts(&change(Some(0o755), Some("deploy"), Some("www"), true));
        assert_eq!(scripts[0], ("chmod", r#"chmod -R -- 0755 "$@""#.to_string()));
        assert_eq!(scripts[1], ("chown", r#"chown -R -- deploy:www "$@""#.to_string()));
        let scripts = shell_scripts(&change(None, None, Some("www"), false));
        assert_eq!(scripts, vec![("chgrp", r#"chgrp -- www "$@""#.to_string())]);
    }

    #[test]
    fn local_chmod_is_recursive_and_skips_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("gf-attrs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tree/sub")).unwrap();
        std::fs::write(dir.join("tree/sub/a.txt"), "a").unwrap();
        std::fs::write(dir.join("outside.txt"), "o").unwrap();
        std::fs::set_permissions(dir.join("outside.txt"), std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(dir.join("outside.txt"), dir.join("tree/link")).unwrap();
        let mode = |p: &str| std::fs::metadata(dir.join(p)).unwrap().permissions().mode() & 0o7777;
        let count = local::set_mode(&[dir.join("tree").to_string_lossy().into_owned()], 0o750, true).unwrap();
        assert_eq!(count, 3);
        assert_eq!((mode("tree"), mode("tree/sub"), mode("tree/sub/a.txt")), (0o750, 0o750, 0o750));
        assert_eq!(mode("outside.txt"), 0o600);
        let lines = local::commands("/x", &["a b".into()], &change(Some(0o644), None, None, false)).unwrap();
        assert_eq!(lines, vec!["chmod -- 0644 '/x/a b'"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
