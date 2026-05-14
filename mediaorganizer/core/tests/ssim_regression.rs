//! SSIM verification regression tests.
//!
//! All test videos are generated synthetically via `ffmpeg -f lavfi`, so no
//! real media files are required. Tests skip when FFmpeg is absent.
//!
//! Key invariants:
//!   1. Identical videos → SSIM ≈ 1.0
//!   2. Completely different videos → SSIM near 0.0
//!   3. Slightly noisy copy → SSIM high but < 1.0
//!   4. SSIM is deterministic on repeated calls
//!   5. Very short window returns a valid value (no crash on edge inputs)

use core::ffmpeg::{compute_ssim_at_offset, which_ffmpeg};
use core::config::HardwareAccel;
use camino::Utf8Path;
use std::process::Command;
use tempfile::TempDir;

// ── helpers ────────────────────────────────────────────────────────────────────

macro_rules! require_ffmpeg {
    () => {
        if which_ffmpeg().is_none() {
            eprintln!("SKIP: ffmpeg not found");
            return;
        }
    };
}

/// Generate a solid-colour test video (no audio).
/// Returns the path to the `.mp4` file.
fn make_colour_video(dir: &TempDir, colour: &str, duration_secs: f32, name: &str) -> std::path::PathBuf {
    let ffmpeg = which_ffmpeg().unwrap();
    let out = dir.path().join(format!("{name}.mp4"));
    let status = Command::new(&ffmpeg)
        .args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi",
            "-i", &format!("color=c={colour}:size=320x240:rate=25:duration={duration_secs}"),
            "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
            out.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg failed for colour={colour} video");
    out
}

/// Generate a testsrc video (moving test-card pattern — lots of detail).
fn make_testsrc_video(dir: &TempDir, duration_secs: f32, name: &str) -> std::path::PathBuf {
    let ffmpeg = which_ffmpeg().unwrap();
    let out = dir.path().join(format!("{name}.mp4"));
    let status = Command::new(&ffmpeg)
        .args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi",
            "-i", &format!("testsrc=size=320x240:rate=25:duration={duration_secs}"),
            "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
            out.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg testsrc generation failed");
    out
}

/// Generate a noisy version of the testsrc video.
fn make_noisy_testsrc(dir: &TempDir, duration_secs: f32, noise_strength: u32, name: &str) -> std::path::PathBuf {
    let ffmpeg = which_ffmpeg().unwrap();
    let out = dir.path().join(format!("{name}.mp4"));
    let status = Command::new(&ffmpeg)
        .args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi",
            "-i", &format!(
                "testsrc=size=320x240:rate=25:duration={duration_secs},noise=alls={noise_strength}:allf=t"
            ),
            "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
            out.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg noisy testsrc generation failed");
    out
}

fn ssim(a: &std::path::Path, b: &std::path::Path, window: f64) -> f32 {
    let ua = Utf8Path::from_path(a).unwrap();
    let ub = Utf8Path::from_path(b).unwrap();
    compute_ssim_at_offset(ua, 0.0, ub, 0.0, window, HardwareAccel::None)
}

// ── tests ──────────────────────────────────────────────────────────────────────

#[test]
fn identical_videos_have_ssim_near_one() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let video = make_testsrc_video(&dir, 3.0, "testsrc");

    let result = ssim(&video, &video, 2.0);
    assert!(
        result >= 0.98,
        "identical video must have SSIM ≥ 0.98, got {result}"
    );
}

#[test]
fn completely_different_videos_have_low_ssim() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let white = make_colour_video(&dir, "white", 3.0, "white");
    let black = make_colour_video(&dir, "black", 3.0, "black");

    let result = ssim(&white, &black, 2.0);
    // Solid white vs solid black: luma DC components are max distance → very low SSIM.
    assert!(
        result < 0.5,
        "white vs black must have SSIM < 0.5, got {result}"
    );
}

#[test]
fn noisy_copy_has_high_but_imperfect_ssim() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let clean = make_testsrc_video(&dir, 3.0, "clean");
    // strength=5: subtle noise, should give SSIM > 0.85
    let noisy = make_noisy_testsrc(&dir, 3.0, 5, "noisy_subtle");

    let result = ssim(&clean, &noisy, 2.0);
    assert!(
        result > 0.70 && result < 0.999,
        "subtle noise should give 0.70 < SSIM < 1.0, got {result}"
    );
}

#[test]
fn heavy_noise_has_lower_ssim_than_subtle_noise() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let clean  = make_testsrc_video(&dir, 3.0, "clean2");
    let subtle = make_noisy_testsrc(&dir, 3.0, 5,  "noisy_subtle2");
    let heavy  = make_noisy_testsrc(&dir, 3.0, 50, "noisy_heavy");

    let ssim_subtle = ssim(&clean, &subtle, 2.0);
    let ssim_heavy  = ssim(&clean, &heavy,  2.0);

    assert!(
        ssim_heavy < ssim_subtle,
        "heavy noise must produce lower SSIM ({ssim_heavy}) than subtle ({ssim_subtle})"
    );
}

#[test]
fn ssim_is_deterministic() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let a = make_testsrc_video(&dir, 3.0, "det_a");
    let b = make_noisy_testsrc(&dir, 3.0, 10, "det_b");

    let r1 = ssim(&a, &b, 2.0);
    let r2 = ssim(&a, &b, 2.0);
    assert_eq!(r1, r2, "SSIM must be deterministic");
}

#[test]
fn ssim_at_nonzero_offset() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    // 5-second test video; sample at offset 1.0 s into both.
    let video = make_testsrc_video(&dir, 5.0, "offset_test");
    let uv = Utf8Path::from_path(&video).unwrap();

    let result = compute_ssim_at_offset(uv, 1.0, uv, 1.0, 1.0, HardwareAccel::None);
    assert!(
        result >= 0.98,
        "same video at same offset must have SSIM ≥ 0.98, got {result}"
    );
}

#[test]
fn ssim_returns_minus_one_when_ffmpeg_missing() {
    // This tests the error path without needing FFmpeg to be absent at
    // the binary level — instead pass a path that doesn't exist.
    // We rely on the fact that compute_ssim_at_offset returns -1.0 on failure.
    // We test this by using a non-existent file path.
    let dir = tempfile::tempdir().unwrap();
    let missing = Utf8Path::new("/nonexistent_path/no_such_file.mp4");
    let missing2 = Utf8Path::new("/nonexistent_path/no_such_file2.mp4");
    // If FFmpeg is present it will fail to open the file; if absent it returns -1 directly.
    let result = compute_ssim_at_offset(missing, 0.0, missing2, 0.0, 1.0, HardwareAccel::None);
    assert_eq!(
        result, -1.0,
        "missing input files must yield -1.0 (error sentinel), got {result}"
    );
}
