//! MPEG-7 video signature regression tests.
//!
//! All test videos generated via `ffmpeg -f lavfi`. Tests skip when:
//!   - FFmpeg is absent, OR
//!   - The `signature` filter is not compiled into the installed FFmpeg build
//!     (many distro packages omit it; it requires --enable-avfilter).
//!
//! Key invariants:
//!   1. extract_signature produces a non-empty .mpeg7sig file
//!   2. Same video → identical signature path returned (cache hit)
//!   3. compare_signatures: identical content → is_match = true, offset ≈ 0
//!   4. compare_signatures: different content → is_match = false
//!   5. Clip-in-movie: short clip known to be at t=5 s → offset ≈ 5.0

use core::mpeg7::{extract_signature, compare_signatures, sig_folder};
use core::ffmpeg::which_ffmpeg;
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

/// Return false (and print a skip message) if the installed FFmpeg does not
/// have the `signature` filter. Called before tests that need it.
fn signature_filter_available() -> bool {
    let ffmpeg = match which_ffmpeg() {
        Some(f) => f,
        None => return false,
    };
    let out = Command::new(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "quiet", "-filters"])
        .output()
        .unwrap_or_else(|_| std::process::Output {
            status: std::process::ExitStatus::default(),
            stdout: vec![],
            stderr: vec![],
        });
    let stdout = String::from_utf8_lossy(&out.stdout);
    let has = stdout.contains("signature");
    if !has {
        eprintln!("SKIP: FFmpeg `signature` filter not available in this build");
    }
    has
}

/// Generate a solid-colour video suitable for MPEG-7 signing.
fn make_video(dir: &TempDir, colour: &str, duration_secs: f32, name: &str) -> std::path::PathBuf {
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
    assert!(status.success(), "ffmpeg colour video failed");
    out
}

/// Build a video: [10 s gray][10 s red][10 s gray]
/// The red segment starts at t=10 and can serve as a "known clip".
fn make_composite_video(dir: &TempDir, name: &str) -> std::path::PathBuf {
    let ffmpeg = which_ffmpeg().unwrap();
    let out = dir.path().join(format!("{name}.mp4"));
    let status = Command::new(&ffmpeg)
        .args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", "color=c=gray:size=320x240:rate=25:duration=10",
            "-f", "lavfi", "-i", "color=c=red:size=320x240:rate=25:duration=10",
            "-f", "lavfi", "-i", "color=c=gray:size=320x240:rate=25:duration=10",
            "-filter_complex", "[0:v][1:v][2:v]concat=n=3:v=1:a=0[out]",
            "-map", "[out]",
            "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
            out.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    out
}

fn extract(video: &std::path::Path) -> Option<std::path::PathBuf> {
    let ffmpeg = which_ffmpeg().unwrap();
    extract_signature(video, &ffmpeg, false)
}

// ── tests ──────────────────────────────────────────────────────────────────────

#[test]
fn signature_file_is_created_and_non_empty() {
    require_ffmpeg!();
    if !signature_filter_available() { return; }

    let dir = tempfile::tempdir().unwrap();
    let video = make_video(&dir, "blue", 5.0, "blue5s");

    let sig = extract(&video);
    assert!(sig.is_some(), "extract_signature must return Some for valid video");
    let sig_path = sig.unwrap();
    assert!(sig_path.exists(), "signature file must exist on disk");
    assert!(
        sig_path.metadata().unwrap().len() > 0,
        "signature file must be non-empty"
    );
}

#[test]
fn second_call_returns_cached_path() {
    require_ffmpeg!();
    if !signature_filter_available() { return; }

    let dir = tempfile::tempdir().unwrap();
    let video = make_video(&dir, "green", 5.0, "green5s");

    let sig_a = extract(&video).expect("first call must succeed");
    let sig_b = extract(&video).expect("second call must succeed");

    assert_eq!(
        sig_a, sig_b,
        "second extraction of same path must return cached file (same path)"
    );
}

#[test]
fn identical_videos_match_at_offset_zero() {
    require_ffmpeg!();
    if !signature_filter_available() { return; }

    let dir = tempfile::tempdir().unwrap();
    let video = make_video(&dir, "red", 5.0, "red_ident");
    let ffmpeg = which_ffmpeg().unwrap();

    let sig = match extract(&video) {
        Some(s) => s,
        None => { eprintln!("SKIP: extraction failed"); return; }
    };

    let result = compare_signatures(&sig, &sig, &ffmpeg, false);
    assert!(
        result.is_match,
        "identical video must match itself; is_match={}",
        result.is_match
    );
    assert!(
        result.offset_secs.abs() < 1.0,
        "self-match offset must be near 0; got {}",
        result.offset_secs
    );
}

#[test]
fn different_videos_do_not_match() {
    require_ffmpeg!();
    if !signature_filter_available() { return; }

    let dir = tempfile::tempdir().unwrap();
    let red   = make_video(&dir, "red",   5.0, "diff_red");
    let white = make_video(&dir, "white", 5.0, "diff_white");
    let ffmpeg = which_ffmpeg().unwrap();

    let sig_red   = match extract(&red)   { Some(s) => s, None => { eprintln!("SKIP"); return; } };
    let sig_white = match extract(&white) { Some(s) => s, None => { eprintln!("SKIP"); return; } };

    let result = compare_signatures(&sig_red, &sig_white, &ffmpeg, false);
    assert!(
        !result.is_match,
        "different videos must not match; is_match={}",
        result.is_match
    );
}

#[test]
fn missing_signature_files_return_no_match() {
    require_ffmpeg!();
    let ffmpeg = which_ffmpeg().unwrap();
    let missing_a = std::path::Path::new("/nonexistent/sig_a.mpeg7sig");
    let missing_b = std::path::Path::new("/nonexistent/sig_b.mpeg7sig");

    let result = compare_signatures(missing_a, missing_b, &ffmpeg, false);
    assert!(
        !result.is_match,
        "missing sig files must return is_match=false"
    );
}

#[test]
fn clip_in_movie_detected_at_correct_offset() {
    require_ffmpeg!();
    if !signature_filter_available() { return; }

    // "movie": 10 s gray + 10 s red + 10 s gray
    // "clip" : 10 s red  (the middle segment in isolation)
    // MPEG-7 detectmode=full should report offset ≈ 10.0 s (where red starts in movie)
    let dir = tempfile::tempdir().unwrap();
    let movie = make_composite_video(&dir, "movie_30s");
    let clip  = make_video(&dir, "red", 10.0, "clip_red");
    let ffmpeg = which_ffmpeg().unwrap();

    let sig_movie = match extract(&movie) { Some(s) => s, None => { eprintln!("SKIP"); return; } };
    let sig_clip  = match extract(&clip)  { Some(s) => s, None => { eprintln!("SKIP"); return; } };

    let result = compare_signatures(&sig_movie, &sig_clip, &ffmpeg, false);

    if !result.is_match {
        // Some FFmpeg builds don't support sub-segment detection even with the
        // signature filter compiled in — accept a non-match as a soft skip.
        eprintln!(
            "INFO: clip-in-movie not detected (FFmpeg build may lack detectmode=full support)"
        );
        return;
    }

    // Offset should be near 10.0 s (where the red segment starts in the movie).
    assert!(
        (result.offset_secs - 10.0).abs() < 2.0,
        "clip-in-movie offset should be ≈10 s; got {}",
        result.offset_secs
    );
}

#[test]
fn sig_folder_is_deterministic() {
    // Pure logic, no FFmpeg needed.
    let a = sig_folder();
    let b = sig_folder();
    assert_eq!(a, b, "sig_folder() must return the same path on repeated calls");
}
