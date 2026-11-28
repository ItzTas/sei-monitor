use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(super) fn look_path(program: &str, path: Option<&str>) -> bool {
    if matches!(program, "" | "." | "..") {
        return false;
    }
    if program.contains('/') {
        return is_executable(Path::new(program));
    }
    let path = path.unwrap_or_default();
    if path.is_empty() {
        return false;
    }
    path.split(':')
        .map(|dir| Path::new(if dir.is_empty() { "." } else { dir }).join(program))
        .find(|candidate| is_executable(candidate))
        .is_some_and(|found| found.is_absolute())
}

pub(super) fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if metadata.is_dir() {
        return false;
    }
    match effective_access(path) {
        Ok(()) => true,
        Err(code) if code == libc::ENOSYS || code == libc::EPERM => {
            metadata.permissions().mode() & 0o111 != 0
        }
        Err(_) => false,
    }
}

fn effective_access(path: &Path) -> Result<(), i32> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| libc::EINVAL)?;
    let status =
        unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::X_OK, libc::AT_EACCESS) };
    if status == 0 {
        return Ok(());
    }
    Err(io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EINVAL))
}
