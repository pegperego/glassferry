//! Private per-user directory for control sockets, askpass socket and ssh logs.
//!
//! Unix socket paths are limited (~104 bytes on macOS) and ssh appends a
//! random suffix while creating the ControlPath, so the directory path must
//! stay short: `$XDG_RUNTIME_DIR/sfm` on Linux, `/tmp/sfm-<uid>` otherwise.

use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::PathBuf;

use crate::error::{AppError, Result};

pub fn create() -> Result<PathBuf> {
    let uid = unsafe { libc::getuid() };
    let dir = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(base) if cfg!(target_os = "linux") => PathBuf::from(base).join("sfm"),
        _ => PathBuf::from(format!("/tmp/sfm-{uid}")),
    };

    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }

    // Refuse a directory planted by someone else in a shared /tmp.
    let meta = std::fs::symlink_metadata(&dir)?;
    if !meta.is_dir() || meta.uid() != uid {
        return Err(AppError::Other(format!(
            "{} is not a directory owned by the current user",
            dir.display()
        )));
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// Removes sockets and logs left behind by app processes that no longer
/// run. Files are named `askpass-<pid>.sock`, `<pid>-<n>.sock`, `<pid>-<n>.log`.
pub fn remove_stale_files(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let pid = name
            .strip_prefix("askpass-")
            .unwrap_or(&name)
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|p| p.parse::<libc::pid_t>().ok());
        let Some(pid) = pid else {
            continue;
        };
        let alive = unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !alive {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Random hex token read from the OS CSPRNG.
pub fn random_token() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
