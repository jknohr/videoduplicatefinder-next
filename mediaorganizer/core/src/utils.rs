//! Platform-aware utility functions.
//!
//! Faithful port of:
//! - VDF.Core/Utils/CoreUtils.cs  — state/settings folder resolution, writability check
//! - VDF.Core/Utils/FileUtils.cs  — trash (Linux freedesktop, macOS, Windows), same-filesystem check
//! - VDF.Core/Utils/Extensions.cs — duration/bytes formatting helpers

use std::{
    fs,
    path::{Path, PathBuf},
};

// ─── Platform detection ───────────────────────────────────────────────────────

/// Returns `true` when running inside a container (Docker/podman).
/// Checks `DOTNET_RUNNING_IN_CONTAINER` for C# compat; also checks `/.dockerenv`.
pub fn is_running_in_container() -> bool {
    std::env::var("DOTNET_RUNNING_IN_CONTAINER")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        || Path::new("/.dockerenv").exists()
}

/// Returns `true` if the directory exists and a test file can be created in it.
pub fn can_write_to_directory(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let test = path.join(format!(".vdf_write_test_{}", uuid_v4_hex()));
    match fs::File::create(&test) {
        Ok(_) => {
            let _ = fs::remove_file(&test);
            true
        }
        Err(_) => false,
    }
}

// ─── State and settings folder resolution ────────────────────────────────────

/// Resolves the directory where VDF stores its runtime state (database, MPEG-7 sigs).
///
/// Priority:
/// 1. If not in a container and the executable's folder is writable → use that.
/// 2. Otherwise → platform XDG/AppData/Library path.
pub fn state_folder() -> PathBuf {
    if !is_running_in_container() {
        if let Some(exe_dir) = current_exe_dir() {
            if can_write_to_directory(&exe_dir) {
                return exe_dir;
            }
        }
    }
    default_state_folder()
}

/// Resolves the directory where VDF stores its settings file.
pub fn settings_folder() -> PathBuf {
    if !is_running_in_container() {
        if let Some(exe_dir) = current_exe_dir() {
            if can_write_to_directory(&exe_dir) {
                return exe_dir;
            }
        }
    }
    default_settings_folder()
}

/// Returns `custom` if it exists; otherwise falls back to `state_folder()`.
pub fn resolve_database_folder(custom: Option<&Path>) -> PathBuf {
    if let Some(p) = custom {
        if p.is_dir() {
            return p.to_path_buf();
        }
    }
    state_folder()
}

fn current_exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

fn default_state_folder() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        dirs::data_local_dir()
    } else if cfg!(target_os = "macos") {
        dirs::home_dir().map(|h| h.join("Library").join("Application Support"))
    } else {
        // XDG_STATE_HOME / ~/.local/state
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))
    };
    let folder = base.unwrap_or_else(|| PathBuf::from("/tmp")).join("VDF");
    let _ = fs::create_dir_all(&folder);
    folder
}

fn default_settings_folder() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        dirs::config_dir()
    } else if cfg!(target_os = "macos") {
        dirs::home_dir().map(|h| h.join("Library").join("Preferences"))
    } else {
        // XDG_CONFIG_HOME / ~/.config
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::config_dir())
    };
    let folder = base.unwrap_or_else(|| PathBuf::from("/tmp")).join("VDF");
    let _ = fs::create_dir_all(&folder);
    folder
}

// ─── Trash / recycle bin ─────────────────────────────────────────────────────

/// Move `file_path` to the system trash.  Returns `true` on success.
/// Falls back to permanent deletion if trash is unavailable on this platform.
///
/// Mirrors `FileUtils.MoveToTrash` from C#.
pub fn move_to_trash(file_path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    return move_to_trash_linux(file_path);

    #[cfg(target_os = "macos")]
    return move_to_trash_macos(file_path);

    #[cfg(target_os = "windows")]
    return move_to_trash_windows(file_path);

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    false
}

#[cfg(target_os = "linux")]
fn move_to_trash_linux(file_path: &Path) -> bool {
    // Freedesktop.org Trash specification:
    // ~/.local/share/Trash/files/ and ~/.local/share/Trash/info/
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    let files_dir = data_home.join("Trash").join("files");
    let info_dir = data_home.join("Trash").join("info");

    if fs::create_dir_all(&files_dir).is_err() || fs::create_dir_all(&info_dir).is_err() {
        return false;
    }

    // Skip if file is on a different filesystem — avoids cross-mount copy.
    if !is_on_same_filesystem(file_path, &files_dir) {
        return false;
    }

    let file_name = file_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let (dest, info) = resolve_trash_name(&files_dir, &info_dir, &file_name);

    let now = chrono_or_iso8601();
    let trash_info = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", file_path.display(), now);

    if fs::write(&info, &trash_info).is_err() {
        return false;
    }
    match fs::rename(file_path, &dest) {
        Ok(_) => true,
        Err(_) => {
            let _ = fs::remove_file(&info);
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn move_to_trash_macos(file_path: &Path) -> bool {
    let trash_dir = match dirs::home_dir() {
        Some(h) => h.join(".Trash"),
        None => return false,
    };

    if fs::create_dir_all(&trash_dir).is_err() {
        return false;
    }
    if !is_on_same_filesystem(file_path, &trash_dir) {
        return false;
    }

    let file_name = file_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let dest = resolve_trash_name_simple(&trash_dir, &file_name);
    fs::rename(file_path, dest).is_ok()
}

#[cfg(target_os = "windows")]
fn move_to_trash_windows(file_path: &Path) -> bool {
    // Use SHFileOperation via winapi raw extern — same as C# SHFileOperation call.
    // Statically link shell32 on Windows.
    #[repr(C)]
    struct ShFileOpStruct {
        hwnd: *mut std::ffi::c_void,
        w_func: u32,
        p_from: *const u16,
        p_to: *const u16,
        f_flags: u16,
        f_any_operations_aborted: i32,
        h_name_mappings: *mut std::ffi::c_void,
        lpsz_progress_title: *const u16,
    }

    extern "system" {
        fn SHFileOperationW(lpFileOp: *mut ShFileOpStruct) -> i32;
    }

    const FO_DELETE: u32 = 0x0003;
    const FOF_ALLOWUNDO: u16 = 0x0040;
    const FOF_SILENT: u16 = 0x0004;
    const FOF_NOCONFIRMATION: u16 = 0x0010;
    const FOF_NOERRORUI: u16 = 0x0400;

    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = file_path.as_os_str().encode_wide().collect();
    wide.push(0); // pFrom is double-null terminated
    wide.push(0);

    let mut op = ShFileOpStruct {
        hwnd: std::ptr::null_mut(),
        w_func: FO_DELETE,
        p_from: wide.as_ptr(),
        p_to: std::ptr::null(),
        f_flags: FOF_ALLOWUNDO | FOF_SILENT | FOF_NOCONFIRMATION | FOF_NOERRORUI,
        f_any_operations_aborted: 0,
        h_name_mappings: std::ptr::null_mut(),
        lpsz_progress_title: std::ptr::null(),
    };

    unsafe { SHFileOperationW(&mut op) == 0 }
}

/// Returns true when `path1` and `path2` reside on the same mounted filesystem.
/// Compares mount points by finding the longest matching drive/mount prefix.
pub fn is_on_same_filesystem(path1: &Path, path2: &Path) -> bool {
    // On Linux/macOS compare device IDs via metadata.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(path1), fs::metadata(path2)) {
            (Ok(m1), Ok(m2)) => m1.dev() == m2.dev(),
            _ => true, // assume same on error — let rename decide
        }
    }
    #[cfg(windows)]
    {
        // Compare volume root of canonical paths.
        let root = |p: &Path| -> Option<PathBuf> {
            let c = fs::canonicalize(p).ok()?;
            c.components().next().map(|c| PathBuf::from(c.as_os_str()))
        };
        match (root(path1), root(path2)) {
            (Some(r1), Some(r2)) => r1 == r2,
            _ => true,
        }
    }
    #[cfg(not(any(unix, windows)))]
    true
}

// ─── Formatting helpers ───────────────────────────────────────────────────────

/// Format a duration in seconds as a human-readable string.
/// Mirrors `Extensions.Format(TimeSpan)` from C#.
pub fn format_duration(secs: f64) -> String {
    let total = secs as u64;
    let d = total / 86400;
    let h = (total % 86400) / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;

    if d > 0 {
        if h > 0 { format!("{}d, {}h", d, h) } else { format!("{}d", d) }
    } else if h > 0 {
        if m > 0 { format!("{}h, {}m", h, m) } else { format!("{}h", h) }
    } else if m > 0 {
        format!("{}m, {}s", m, s)
    } else {
        format!("{}s", s)
    }
}

/// Format a byte count as a human-readable size string.
/// Mirrors `Extensions.BytesToString(long)` from C#.
pub fn bytes_to_string(bytes: u64) -> String {
    const SUFFIXES: &[&str] = &[" B", " KB", " MB", " GB", " TB", " PB", " EB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut place = 0usize;
    let mut value = bytes as f64;
    while value >= 1024.0 && place < SUFFIXES.len() - 1 {
        value /= 1024.0;
        place += 1;
    }
    format!("{:.1}{}", value, SUFFIXES[place])
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Find an unused filename in `dir` for a trash entry named `name`.
fn resolve_trash_name(files_dir: &Path, info_dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    let dest = files_dir.join(name);
    let info = info_dir.join(format!("{}.trashinfo", name));
    if !dest.exists() && !info.exists() {
        return (dest, info);
    }
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("");
    let ext_dot = if ext.is_empty() { String::new() } else { format!(".{}", ext) };
    for i in 1.. {
        let new_name = format!("{}_{}{}", stem, i, ext_dot);
        let d = files_dir.join(&new_name);
        let inf = info_dir.join(format!("{}.trashinfo", new_name));
        if !d.exists() && !inf.exists() {
            return (d, inf);
        }
    }
    unreachable!()
}

#[cfg(target_os = "macos")]
fn resolve_trash_name_simple(dir: &Path, name: &str) -> PathBuf {
    let dest = dir.join(name);
    if !dest.exists() {
        return dest;
    }
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("");
    let ext_dot = if ext.is_empty() { String::new() } else { format!(".{}", ext) };
    for i in 1.. {
        let d = dir.join(format!("{}_{}{}", stem, i, ext_dot));
        if !d.exists() {
            return d;
        }
    }
    unreachable!()
}

fn chrono_or_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Format as ISO-8601 local time approximation (UTC, no TZ suffix for freedesktop compat).
    let s = secs;
    let sec = s % 60;
    let min = (s / 60) % 60;
    let hour = (s / 3600) % 24;
    let days = s / 86400;
    // Simplified date calculation (days since epoch → year/month/day).
    let (y, mo, d) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", y, mo, d, hour, min, sec)
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Gregorian calendar approximation (sufficient for trash timestamps).
    let mut year = 1970u64;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let dy = if leap { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days = [31u64, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u64;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }
    (year, month, days + 1)
}

fn uuid_v4_hex() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:032x}", t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_to_string_zero() {
        assert_eq!(bytes_to_string(0), "0 B");
    }

    #[test]
    fn bytes_to_string_mb() {
        let s = bytes_to_string(1024 * 1024);
        assert!(s.contains("MB"), "expected MB in '{}'", s);
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(45.0), "45s");
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(format_duration(90.0), "1m, 30s");
    }

    #[test]
    fn format_duration_hours() {
        assert_eq!(format_duration(3661.0), "1h, 1m");
    }

    #[test]
    fn state_folder_exists() {
        let p = state_folder();
        assert!(p.is_dir() || std::fs::create_dir_all(&p).is_ok());
    }
}
