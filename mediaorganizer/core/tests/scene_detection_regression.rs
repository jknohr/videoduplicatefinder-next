//! Scene-aware skip regression tests.
//!
//! All test fixtures are generated synthetically via `ffmpeg -f lavfi`, so no
//! real media files are required. Each test skips silently when FFmpeg is absent.
//!
//! Synthetic scene-change video:
//!   Segment 0–2 s : solid red   (colour=c=red)
//!   Segment 2–5 s : solid blue  (colour=c=blue)
//!   Segment 5–7 s : solid green (colour=c=green)
//! → hard scene cuts at t≈2.0 s and t≈5.0 s.

use core::ffmpeg::{get_scene_change_timestamps, which_ffmpeg};
use core::config::HardwareAccel;
use camino::Utf8Path;
use std::process::Command;
use tempfile::TempDir;

// ── helpers ────────────────────────────────────────────────────────────────────

/// Generate a synthetic video with N hard scene cuts at known timestamps.
/// Returns (TempDir, path_to_video) — caller must keep TempDir alive.
fn make_scene_video(dir: &TempDir) -> std::path::PathBuf {
    let ffmpeg = which_ffmpeg().expect("ffmpeg must be present");
    let out = dir.path().join("scene_test.mp4");

    // Two colour segments concatenated via filter_complex:
    //   [0:v] red 0–2 s, [1:v] blue 2–5 s, [2:v] green 5–7 s
    let status = Command::new(&ffmpeg)
        .args([
            "-y",
            "-hide_banner",
            "-loglevel", "error",
            // Three colour sources
            "-f", "lavfi", "-i", "color=c=red:size=320x240:rate=25:duration=2",
            "-f", "lavfi", "-i", "color=c=blue:size=320x240:rate=25:duration=3",
            "-f", "lavfi", "-i", "color=c=green:size=320x240:rate=25:duration=2",
            // Concatenate them
            "-filter_complex", "[0:v][1:v][2:v]concat=n=3:v=1:a=0[out]",
            "-map", "[out]",
            "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("ffmpeg invocation failed");

    assert!(status.success(), "ffmpeg failed to generate scene test video");
    out
}

/// Skip the test if FFmpeg isn't installed.
macro_rules! require_ffmpeg {
    () => {
        if which_ffmpeg().is_none() {
            eprintln!("SKIP: ffmpeg not found");
            return;
        }
    };
}

// ── tests ──────────────────────────────────────────────────────────────────────

#[test]
fn scene_changes_detected_at_known_timestamps() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let video = make_scene_video(&dir);
    let utf8 = Utf8Path::from_path(&video).unwrap();

    // Low threshold to catch the hard colour cuts.
    let timestamps = get_scene_change_timestamps(utf8, 0.1, 10, HardwareAccel::None);

    assert!(
        !timestamps.is_empty(),
        "expected at least one scene change, got none"
    );
    // Scene cuts are at t≈2.0 and t≈5.0; allow ±0.5 s tolerance for frame
    // boundary quantisation at 25 fps.
    let near_2 = timestamps.iter().any(|&t| (t - 2.0).abs() < 0.5);
    let near_5 = timestamps.iter().any(|&t| (t - 5.0).abs() < 0.5);

    assert!(
        near_2,
        "expected a scene change near t=2.0 s; got {:?}",
        timestamps
    );
    assert!(
        near_5,
        "expected a scene change near t=5.0 s; got {:?}",
        timestamps
    );
}

#[test]
fn scene_detection_returns_empty_for_uniform_video() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let ffmpeg = which_ffmpeg().unwrap();
    let out = dir.path().join("uniform.mp4");

    // Uniform solid colour — no scene changes.
    let status = Command::new(&ffmpeg)
        .args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", "color=c=gray:size=320x240:rate=25:duration=5",
            "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
            out.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let utf8 = Utf8Path::from_path(&out).unwrap();
    let timestamps = get_scene_change_timestamps(utf8, 0.3, 10, HardwareAccel::None);

    assert!(
        timestamps.is_empty(),
        "uniform video should produce no scene changes; got {:?}",
        timestamps
    );
}

#[test]
fn scene_detection_respects_max_count() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let video = make_scene_video(&dir);
    let utf8 = Utf8Path::from_path(&video).unwrap();

    // We have 2 real cuts; ask for max 1 — must not return more than 1.
    let timestamps = get_scene_change_timestamps(utf8, 0.1, 1, HardwareAccel::None);
    assert!(
        timestamps.len() <= 1,
        "max_count=1 must return at most 1 timestamp, got {}",
        timestamps.len()
    );
}

#[test]
fn scene_detection_high_threshold_finds_fewer_cuts() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let video = make_scene_video(&dir);
    let utf8 = Utf8Path::from_path(&video).unwrap();

    // Very high threshold — may miss cuts or find none.
    // Low threshold — must find at least the two hard cuts.
    let low = get_scene_change_timestamps(utf8, 0.05, 20, HardwareAccel::None);
    let high = get_scene_change_timestamps(utf8, 0.95, 20, HardwareAccel::None);

    assert!(
        low.len() >= high.len(),
        "lower threshold must find at least as many cuts as higher: low={} high={}",
        low.len(), high.len()
    );
}

#[test]
fn scene_detection_is_stable_across_calls() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let video = make_scene_video(&dir);
    let utf8 = Utf8Path::from_path(&video).unwrap();

    let a = get_scene_change_timestamps(utf8, 0.1, 10, HardwareAccel::None);
    let b = get_scene_change_timestamps(utf8, 0.1, 10, HardwareAccel::None);

    assert_eq!(a, b, "scene detection must be deterministic");
}
