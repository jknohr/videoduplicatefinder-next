//! FFmpeg availability check — port of VDF.Web/Services/FFmpegSetupService.cs.
//!
//! On startup, verifies that `ffmpeg` and `ffprobe` are available on PATH.
//! The result is stored globally and served to the UI via `get_ffmpeg_status()`.
//! If either binary is missing, the UI shows an inline warning banner with
//! install instructions rather than a hard error.

use std::sync::OnceLock;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq)]
pub enum FfmpegStatus {
    /// Both ffmpeg and ffprobe are on PATH.
    Ready,
    /// ffmpeg was found but ffprobe was not.
    MissingFfprobe { ffmpeg_path: std::path::PathBuf },
    /// ffprobe was found but ffmpeg was not.
    MissingFfmpeg { ffprobe_path: std::path::PathBuf },
    /// Neither binary was found.
    Missing,
}

struct FfmpegState {
    status: FfmpegStatus,
    ffmpeg_path: Option<std::path::PathBuf>,
    ffprobe_path: Option<std::path::PathBuf>,
}

static FFMPEG_STATE: OnceLock<FfmpegState> = OnceLock::new();

/// Run the FFmpeg availability check. Safe to call from multiple threads —
/// only the first call does work; subsequent calls are instant no-ops.
pub fn check_ffmpeg() {
    FFMPEG_STATE.get_or_init(|| {
        let ffmpeg  = app_core::ffmpeg::which_ffmpeg();
        let ffprobe = app_core::ffmpeg::which_ffprobe();

        let status = match (&ffmpeg, &ffprobe) {
            (Some(_), Some(_)) => {
                info!("FFmpeg: ✓  FFprobe: ✓");
                FfmpegStatus::Ready
            }
            (Some(f), None) => {
                warn!("FFprobe not found on PATH. Some features (metadata read, SSIM) may not work.");
                FfmpegStatus::MissingFfprobe { ffmpeg_path: f.clone() }
            }
            (None, Some(p)) => {
                warn!("FFmpeg not found on PATH. Video hashing, thumbnail generation, and scanning will not work.");
                FfmpegStatus::MissingFfmpeg { ffprobe_path: p.clone() }
            }
            (None, None) => {
                warn!("Neither ffmpeg nor ffprobe found on PATH.");
                warn!("Install FFmpeg: https://ffmpeg.org/download.html");
                warn!("  Linux:   sudo apt install ffmpeg");
                warn!("  macOS:   brew install ffmpeg");
                warn!("  Windows: https://github.com/BtbN/FFmpeg-Builds/releases");
                warn!("  Docker:  use the provided Dockerfile (FFmpeg is pre-installed)");
                FfmpegStatus::Missing
            }
        };

        FfmpegState { status, ffmpeg_path: ffmpeg, ffprobe_path: ffprobe }
    });
}

/// Returns the FFmpeg availability status determined at startup.
/// Returns `None` if `check_ffmpeg()` has not been called yet.
pub fn ffmpeg_status() -> Option<&'static FfmpegStatus> {
    FFMPEG_STATE.get().map(|s| &s.status)
}

/// Returns `true` if FFmpeg is available (both ffmpeg and ffprobe found).
pub fn ffmpeg_ready() -> bool {
    matches!(ffmpeg_status(), Some(FfmpegStatus::Ready))
}

/// Returns the path to the ffmpeg binary, if found.
pub fn ffmpeg_path() -> Option<&'static std::path::Path> {
    FFMPEG_STATE.get().and_then(|s| s.ffmpeg_path.as_deref())
}

/// Returns the path to the ffprobe binary, if found.
pub fn ffprobe_path() -> Option<&'static std::path::Path> {
    FFMPEG_STATE.get().and_then(|s| s.ffprobe_path.as_deref())
}

/// Human-readable install instructions for the current platform.
pub fn install_instructions() -> &'static str {
    if cfg!(target_os = "linux") {
        "sudo apt install ffmpeg   (Debian/Ubuntu)\nsudo dnf install ffmpeg   (Fedora/RHEL)\nOr download from https://ffmpeg.org/download.html"
    } else if cfg!(target_os = "macos") {
        "brew install ffmpeg\nOr download from https://ffmpeg.org/download.html"
    } else if cfg!(target_os = "windows") {
        "Download from https://github.com/BtbN/FFmpeg-Builds/releases\nExtract and add the bin/ folder to your PATH."
    } else {
        "https://ffmpeg.org/download.html"
    }
}
