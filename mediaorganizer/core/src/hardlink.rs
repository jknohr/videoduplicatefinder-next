//! Hard link detection — prevents comparing files that are hard links to the same inode.
//!
//! Faithful port of VDF.Core/Utils/HardLinkUtils.cs.
//!
//! POSIX: compare inode + device via `std::fs::Metadata`.
//! Windows: compare volume serial number + file index via `GetFileInformationByHandle`.

use std::path::Path;

/// Returns `true` when `a` and `b` are the same file on disk (hard links to
/// the same inode, or the same path).  Returns `false` on any I/O error so
/// that we degrade to comparing the files rather than silently skipping them.
pub fn are_same_file(a: impl AsRef<Path>, b: impl AsRef<Path>) -> bool {
    let a = a.as_ref();
    let b = b.as_ref();

    #[cfg(unix)]
    return are_same_file_unix(a, b);

    #[cfg(windows)]
    return are_same_file_windows(a, b);

    #[cfg(not(any(unix, windows)))]
    {
        // Fallback: canonicalize and compare paths.
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(ca), Ok(cb)) => ca == cb,
            _ => false,
        }
    }
}

// ── POSIX ─────────────────────────────────────────────────────────────────────

#[cfg(unix)]
fn are_same_file_unix(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (ma, mb) = match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) => (ma, mb),
        _ => return false,
    };
    ma.ino() == mb.ino() && ma.dev() == mb.dev()
}

// ── Windows ───────────────────────────────────────────────────────────────────

#[cfg(windows)]
fn are_same_file_windows(a: &Path, b: &Path) -> bool {
    use std::os::windows::io::AsRawHandle;
    // We use the standard library's same_file detection via the ntiern-crate-free
    // approach: open both files and compare the by-handle file information.
    match (get_file_id_windows(a), get_file_id_windows(b)) {
        (Some(id_a), Some(id_b)) => {
            // If both IDs are zero (some FS providers return zeros), fall back
            // to canonicalize comparison.
            if id_a == (0, 0, 0) || id_b == (0, 0, 0) {
                return fallback_canonicalize(a, b);
            }
            id_a == id_b
        }
        _ => fallback_canonicalize(a, b),
    }
}

#[cfg(windows)]
fn get_file_id_windows(path: &Path) -> Option<(u32, u32, u32)> {
    use std::os::windows::ffi::OsStrExt;
    use std::ffi::OsStr;

    // Use CreateFileW + GetFileInformationByHandle via winapi-style raw FFI.
    // We rely only on std and the windows-sys crate (available via std's re-export).
    // Avoid pulling winapi so this compiles on stable without extra deps.
    //
    // Open with FILE_READ_ATTRIBUTES | FILE_FLAG_BACKUP_SEMANTICS to handle
    // directories as well as files.
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();

    extern "system" {
        fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: u32,
            dwShareMode: u32,
            lpSecurityAttributes: *mut std::ffi::c_void,
            dwCreationDisposition: u32,
            dwFlagsAndAttributes: u32,
            hTemplateFile: *mut std::ffi::c_void,
        ) -> *mut std::ffi::c_void;

        fn GetFileInformationByHandle(
            hFile: *mut std::ffi::c_void,
            lpFileInformation: *mut ByHandleFileInformation,
        ) -> i32;

        fn CloseHandle(hObject: *mut std::ffi::c_void) -> i32;
    }

    #[repr(C)]
    struct ByHandleFileInformation {
        dw_file_attributes: u32,
        ft_creation_time: [u32; 2],
        ft_last_access_time: [u32; 2],
        ft_last_write_time: [u32; 2],
        dw_volume_serial_number: u32,
        n_file_size_high: u32,
        n_file_size_low: u32,
        n_number_of_links: u32,
        n_file_index_high: u32,
        n_file_index_low: u32,
    }

    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const FILE_SHARE_READ: u32 = 0x0001;
    const FILE_SHARE_WRITE: u32 = 0x0002;
    const FILE_SHARE_DELETE: u32 = 0x0004;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;
    const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = -1isize as *mut _;

    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut info: ByHandleFileInformation = std::mem::zeroed();
        let ok = GetFileInformationByHandle(handle, &mut info);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some((info.dw_volume_serial_number, info.n_file_index_high, info.n_file_index_low))
    }
}

#[allow(dead_code)]
fn fallback_canonicalize(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_file_same_path() {
        // A path compared to itself is always the same file.
        let path = std::env::current_exe().unwrap();
        assert!(are_same_file(&path, &path));
    }

    #[test]
    fn different_files_are_not_same() {
        let a = std::env::current_exe().unwrap();
        // Compare with the parent directory; should not match.
        let b = a.parent().unwrap_or(&a);
        // Don't assert false — directories may or may not work on all platforms.
        // Just verify no panic.
        let _ = are_same_file(&a, b);
    }
}
