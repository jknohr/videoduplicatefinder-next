//! MPEG-7 Video Signature engine.
//!
//! Faithful port of VDF.Core/FFTools/Mpeg7SignatureEngine.cs.
//!
//! Extracts ISO/IEC 15938 binary signatures via the FFmpeg `signature` filter.
//! Compares two signatures using `detectmode=full` (sub-segment / clip-in-movie).
//!
//! Signature files are cached by path hash in `dirs::data_local_dir()/vdf/mpeg7/`.
//! The cache name is the first 16 hex characters of SHA-256(UTF-8(path)).

use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use tracing::warn;

// ─── Cache directory ─────────────────────────────────────────────────────────

/// Returns the directory where MPEG-7 signature files are cached.
pub fn sig_folder() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("vdf")
        .join("mpeg7")
}

fn path_hash(video_path: &str) -> String {
    let hash = Sha256::digest(video_path.as_bytes());
    // Take first 16 hex characters = 8 bytes (matches C# `[..16]`)
    format!("{:x}", hash)[..16].to_string()
}

fn escape_path(p: &str) -> String {
    p.replace('\\', "/").replace('\'', "'\\''")
}

// ─── Extraction ──────────────────────────────────────────────────────────────

/// Extract the MPEG-7 signature for `video_path` and write it to the cache.
///
/// Returns the path to the `.mpeg7sig` file on success, `None` on failure.
/// If the signature file already exists and is non-empty, returns it immediately
/// without re-running FFmpeg (matches C# caching behaviour).
pub fn extract_signature(video_path: &Path, ffmpeg_path: &Path, extended_logging: bool) -> Option<PathBuf> {
    let video_str = video_path.to_str()?;
    let folder = sig_folder();
    std::fs::create_dir_all(&folder).ok()?;

    let hash = path_hash(video_str);
    let sig_path = folder.join(format!("{hash}.mpeg7sig"));

    // Already cached?
    if sig_path.exists() && sig_path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Some(sig_path);
    }

    let loglevel = if extended_logging { "info" } else { "quiet" };
    let sig_escaped = escape_path(sig_path.to_str().unwrap_or(""));
    let video_escaped = escape_path(video_str);

    let args = vec![
        "-hide_banner".to_string(),
        format!("-loglevel {loglevel}"),
        "-nostdin".to_string(),
        format!("-i \"{video_escaped}\""),
        format!("-vf \"signature=format=binary:filename={sig_escaped}\""),
        "-f null -".to_string(),
    ];

    // Flatten to a single shell-style argument string for cross-platform compat.
    // We use Command with shell splitting on Unix; on Windows a similar approach works.
    let status = run_ffmpeg_with_timeout(ffmpeg_path, &args, Duration::from_secs(120));

    match status {
        Some(true) => {
            if sig_path.exists() && sig_path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                Some(sig_path)
            } else {
                warn!("MPEG-7 extract produced no output for {video_str}");
                None
            }
        }
        _ => {
            warn!("MPEG-7 extraction timed out or failed for {video_str}");
            None
        }
    }
}

// ─── Comparison ──────────────────────────────────────────────────────────────

/// Result of comparing two MPEG-7 signatures.
#[derive(Debug, Clone)]
pub struct Mpeg7Match {
    /// True when FFmpeg reported "match at offset".
    pub is_match: bool,
    /// Time offset in seconds (File B appears at this offset in File A).
    pub offset_secs: f64,
    /// Always 1.0 when matched (confidence is binary for MPEG-7).
    pub confidence: f64,
}

/// Compare two cached `.mpeg7sig` files using `detectmode=full`.
///
/// Returns `Mpeg7Match::is_match == false` if either file is missing or
/// FFmpeg does not report a match.
pub fn compare_signatures(
    sig_a: &Path,
    sig_b: &Path,
    ffmpeg_path: &Path,
    _extended_logging: bool,
) -> Mpeg7Match {
    if !sig_a.exists() || !sig_b.exists() {
        return Mpeg7Match { is_match: false, offset_secs: 0.0, confidence: 0.0 };
    }

    let sig_a_esc = escape_path(sig_a.to_str().unwrap_or(""));
    let sig_b_esc = escape_path(sig_b.to_str().unwrap_or(""));

    // Dummy lavfi source — the signature filter reads the sig files directly.
    let args = vec![
        "-hide_banner".to_string(),
        "-loglevel info".to_string(),
        "-nostdin".to_string(),
        "-f lavfi -i nullsrc=size=1x1:duration=1".to_string(),
        "-f lavfi -i nullsrc=size=1x1:duration=1".to_string(),
        format!(
            "-lavfi \"[0][1]signature=detectmode=full:nb_inputs=2:filename={sig_a_esc}|{sig_b_esc}\""
        ),
        "-f null -".to_string(),
    ];

    let output = run_ffmpeg_capture_stderr(ffmpeg_path, &args, Duration::from_secs(30));

    let Some(stderr) = output else {
        warn!("MPEG-7 comparison timed out");
        return Mpeg7Match { is_match: false, offset_secs: 0.0, confidence: 0.0 };
    };

    // Parse "match at offset NNN.NNN" from stderr.
    let mut is_match = false;
    let mut offset_secs = 0.0f64;

    for line in stderr.lines() {
        let lower = line.to_lowercase();
        if lower.contains("match at offset") {
            is_match = true;
            // Find the float after "offset "
            if let Some(idx) = lower.find("offset") {
                let after = &line[idx + 7..].trim_start();
                let end = after.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
                    .unwrap_or(after.len());
                if let Ok(v) = after[..end].parse::<f64>() {
                    offset_secs = v;
                }
            }
        }
    }

    Mpeg7Match {
        is_match,
        offset_secs,
        confidence: if is_match { 1.0 } else { 0.0 },
    }
}

// ─── FFmpeg process helpers ───────────────────────────────────────────────────

/// Run FFmpeg with a timeout; returns `Some(true)` if exited successfully within
/// the timeout, `Some(false)` if non-zero exit, `None` if timed out or failed to start.
fn run_ffmpeg_with_timeout(ffmpeg: &Path, args: &[String], timeout: Duration) -> Option<bool> {
    let joined = args.join(" ");
    // Use sh -c on Unix to handle the shell quoting in the args string.
    #[cfg(unix)]
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("{} {}", ffmpeg.display(), joined))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    #[cfg(not(unix))]
    let mut child = Command::new(ffmpeg)
        .raw_arg(&joined)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // Poll until timeout
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
}

/// Run FFmpeg and capture its stderr output within `timeout`.
/// Returns `None` if the process failed to start or timed out.
fn run_ffmpeg_capture_stderr(ffmpeg: &Path, args: &[String], timeout: Duration) -> Option<String> {
    let joined = args.join(" ");
    #[cfg(unix)]
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("{} {}", ffmpeg.display(), joined))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    #[cfg(not(unix))]
    let mut child = Command::new(ffmpeg)
        .raw_arg(&joined)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child.wait_with_output().ok()?;
                return Some(String::from_utf8_lossy(&output.stderr).into_owned());
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_hash_length() {
        let h = path_hash("/some/path/to/video.mp4");
        assert_eq!(h.len(), 16);
    }

    #[test]
    fn escape_path_backslash() {
        assert_eq!(escape_path("C:\\foo\\bar"), "C:/foo/bar");
    }
}
