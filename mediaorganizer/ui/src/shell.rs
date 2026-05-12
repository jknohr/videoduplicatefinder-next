//! Cross-platform shell utilities.
//!
//! Ports VDF.GUI/Utils/ShellUtils.cs — opening files and revealing them in
//! the system file manager.

/// Open a file path with the default application registered for its type.
/// Silent on failure.
pub fn open_path(path: &str) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();

    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();

    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", path])
        .spawn();
}

/// Reveal a file in the system file manager, selecting it.
/// Port of `ShellUtils.ShowInExplorer` from C# VDF.GUI.
pub fn reveal_in_folder(path: &str) {
    #[cfg(target_os = "linux")]
    {
        // Try D-Bus file manager protocol (Nautilus, Dolphin, Nemo support it).
        // Fall back to xdg-open on the parent directory if dbus-send is absent.
        let parent = std::path::Path::new(path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or(path);
        let dbus_ok = std::process::Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("array:string:file://{path}"),
                "string:",
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !dbus_ok {
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        }
    }

    #[cfg(target_os = "macos")]
    {
        // `open -R file` reveals the file in Finder with it selected.
        let _ = std::process::Command::new("open").args(["-R", path]).spawn();
    }

    #[cfg(target_os = "windows")]
    {
        // `explorer /select,path` opens Explorer and selects the file.
        // Path must use backslashes.
        let win_path = path.replace('/', "\\");
        let _ = std::process::Command::new("explorer")
            .args(["/select,", &win_path])
            .spawn();
    }
}
