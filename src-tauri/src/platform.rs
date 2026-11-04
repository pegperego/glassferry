//! Differences between Unix (macOS, Linux) and Windows, in one place, so the
//! rest of the code stays the same on every system (see docs/windows.md).

use std::fs::Metadata;
use std::path::Path;

/// Permission bits of a local file (`0o7777`); Windows has none.
pub fn mode(meta: &Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode() & 0o7777)
    }
    #[cfg(windows)]
    {
        let _ = meta;
        None
    }
}

/// Owner and group ids of a local file; Windows has none.
pub fn owner(meta: &Metadata) -> (Option<u32>, Option<u32>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (Some(meta.uid()), Some(meta.gid()))
    }
    #[cfg(windows)]
    {
        let _ = meta;
        (None, None)
    }
}

/// The filesystem a file lives on, to stay on one filesystem like
/// `find -xdev`; unknown on Windows (walks cross volume mount points).
pub fn device(meta: &Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(meta.dev())
    }
    #[cfg(windows)]
    {
        let _ = meta;
        None
    }
}

/// Both paths name the same file (e.g. a case-only rename on a
/// case-insensitive volume).
pub fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        // Canonical paths resolve to the file's real name and case.
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

/// Sets the permission bits of a local file; nothing to do on Windows.
pub fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(windows)]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// A rename failed because source and destination are on different volumes.
pub fn is_cross_device(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::EXDEV)
    }
    #[cfg(windows)]
    {
        // ERROR_NOT_SAME_DEVICE
        error.raw_os_error() == Some(17)
    }
}

/// Options for a new file readable only by the current user (0600 on Unix;
/// on Windows the profile folders already allow only the user).
pub fn private_file_options() -> std::fs::OpenOptions {
    #[allow(unused_mut)]
    let mut options = std::fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Console programs started by the app must not open a console window on
/// Windows (the app has none); no effect elsewhere.
pub fn no_window(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// `no_window` for `std::process::Command`.
pub fn no_window_std(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Whether a process with this id is still running.
pub fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        alive || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code) != 0;
            CloseHandle(handle);
            ok && code == STILL_ACTIVE as u32
        }
    }
}

/// A pipe for a child's stdin (`inbound` false) or stdout (`inbound` true),
/// overlapped on both ends: Microsoft's ssh hangs after authentication when
/// its stdout is a pipe from `Stdio::piped()` (or `CreatePipe`), and works
/// with handles like those `sftp.exe` creates (docs/windows.md, phase 0).
/// Returns the app's end (async) and the child's end (inheritable). Must run
/// inside the Tokio runtime.
#[cfg(windows)]
pub fn overlapped_pipe(inbound: bool) -> std::io::Result<(tokio::net::windows::named_pipe::NamedPipeServer, std::os::windows::io::OwnedHandle)> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND};
    use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};

    static NEXT: AtomicU32 = AtomicU32::new(0);
    let token = crate::runtime_dir::random_token().map_err(|e| std::io::Error::other(e.to_string()))?;
    let name = format!(r"\\.\pipe\glassferry-io-{}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed), &token[..8]);
    let wide: Vec<u16> = std::ffi::OsStr::new(&name).encode_wide().chain(Some(0)).collect();
    // SAFETY: plain Win32 calls; each handle is checked and owned exactly once.
    unsafe {
        let access = if inbound { PIPE_ACCESS_INBOUND } else { PIPE_ACCESS_OUTBOUND };
        let server = CreateNamedPipeW(
            wide.as_ptr(),
            access | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            64 * 1024,
            64 * 1024,
            0,
            std::ptr::null(),
        );
        if server == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        let client_access = if inbound { GENERIC_WRITE } else { GENERIC_READ };
        let client = CreateFileW(wide.as_ptr(), client_access, 0, &attributes, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, std::ptr::null_mut());
        if client == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        // Registered with Tokio once both ends exist (connected).
        let server = tokio::net::windows::named_pipe::NamedPipeServer::from_raw_handle(server as RawHandle)?;
        Ok((server, OwnedHandle::from_raw_handle(client as RawHandle)))
    }
}

/// A local path as the panes show it (`/` separators) turned into this
/// system's form, for APIs that want it (the Recycle Bin, Explorer).
pub fn native_path(path: &str) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        std::path::PathBuf::from(path.replace('/', "\\"))
    }
    #[cfg(unix)]
    {
        std::path::PathBuf::from(path)
    }
}

/// A local path from the system in the panes' form (`C:/Users/x` on Windows).
pub fn pane_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) { text.replace('\\', "/") } else { text.into_owned() }
}

/// Windows: the drive letters present ("C:", "D:"…), for the local pane's
/// virtual root. Empty elsewhere.
pub fn drives() -> Vec<String> {
    #[cfg(windows)]
    {
        // SAFETY: no arguments, returns a bitmask.
        let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
        (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| format!("{}:", (b'A' + i) as char)).collect()
    }
    #[cfg(unix)]
    {
        Vec::new()
    }
}
