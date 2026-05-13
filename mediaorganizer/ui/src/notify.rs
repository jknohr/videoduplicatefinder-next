//! Desktop notification helpers.
//!
//! Faithful port of `VDF.GUI/Utils/DesktopNotificationHelper.cs`.
//! Sends a system notification after scan completion using the platform's
//! native notification mechanism — no dependencies, silent on failure.

const APP_NAME: &str = "MediaOrganizer";

/// Send a desktop notification. Silently fails if the platform tool is absent.
pub fn notify_desktop(title: &str, message: &str) {
    #[cfg(target_os = "windows")]
    notify_windows(title, message);

    #[cfg(target_os = "macos")]
    notify_macos(title, message);

    #[cfg(target_os = "linux")]
    notify_linux(title, message);

    // All other platforms: no-op
    let _ = (title, message);
}

#[cfg(target_os = "linux")]
fn notify_linux(title: &str, message: &str) {
    let _ = std::process::Command::new("notify-send")
        .arg("--app-name").arg(APP_NAME)
        .arg(title)
        .arg(message)
        .spawn();
}

#[cfg(target_os = "macos")]
fn notify_macos(title: &str, message: &str) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape_applescript(message),
        escape_applescript(title),
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .spawn();
}

#[cfg(target_os = "windows")]
fn notify_windows(title: &str, message: &str) {
    // Use PowerShell's BalloonTip via the Shell.Application COM object.
    // All arguments are passed as separate argv entries — no shell escaping needed for
    // single-line alphanumeric title/message strings produced by the scan engine.
    // Falls back silently if PowerShell is unavailable.
    let script = format!(
        r#"Add-Type -AssemblyName System.Windows.Forms; $n=New-Object System.Windows.Forms.NotifyIcon; $n.Icon=[System.Drawing.SystemIcons]::Information; $n.Visible=$true; $n.ShowBalloonTip(4000,'{title}','{msg}',[System.Windows.Forms.ToolTipIcon]::Info); Start-Sleep -Milliseconds 4500; $n.Dispose()"#,
        title = title.replace('\'', "''"),
        msg = message.replace('\'', "''"),
    );
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
        .spawn();
}

#[cfg(target_os = "macos")]
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
