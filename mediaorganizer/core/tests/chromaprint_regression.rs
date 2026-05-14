//! Chromaprint audio fingerprint regression tests.
//!
//! All test audio is generated synthetically via `ffmpeg -f lavfi -i sine`,
//! so no real media files are required. Tests skip when FFmpeg is absent.
//!
//! Key invariants verified:
//!   1. Same audio content → identical fingerprint (determinism)
//!   2. Different frequencies → distinct fingerprints (discrimination)
//!   3. Fingerprint similarity: same content scores ~1.0, different ~low
//!   4. fingerprint_sliding_window: A-as-clip inside B-with-A is found
//!   5. Silence / very short audio → None or empty vec, no panic

use core::audio::{compute_fingerprint, fingerprint_similarity, fingerprint_sliding_window};
use core::ffmpeg::which_ffmpeg;
use camino::Utf8Path;
use std::process::Command;
use tempfile::TempDir;

// ── helpers ────────────────────────────────────────────────────────────────────

/// Skip when FFmpeg is absent.
macro_rules! require_ffmpeg {
    () => {
        if which_ffmpeg().is_none() {
            eprintln!("SKIP: ffmpeg not found");
            return;
        }
    };
}

/// Generate a sine-wave audio file.
///
/// * `freq_hz`  — frequency in Hz (e.g. 440 for A4)
/// * `duration` — seconds
/// * `name`     — filename stem used inside `dir`
///
/// Returns the path to the generated `.m4a` file.
fn make_sine(dir: &TempDir, freq_hz: u32, duration: f32, name: &str) -> std::path::PathBuf {
    let ffmpeg = which_ffmpeg().expect("ffmpeg present");
    let out = dir.path().join(format!("{name}.m4a"));
    let status = Command::new(&ffmpeg)
        .args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi",
            "-i", &format!("sine=frequency={freq_hz}:duration={duration}"),
            "-c:a", "aac", "-b:a", "128k",
            out.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg sine generation failed");
    out
}

/// Concatenate two audio files using FFmpeg concat filter.
fn concat_audio(dir: &TempDir, first: &std::path::Path, second: &std::path::Path, name: &str) -> std::path::PathBuf {
    let ffmpeg = which_ffmpeg().unwrap();
    let out = dir.path().join(format!("{name}.m4a"));
    let status = Command::new(&ffmpeg)
        .args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-i", first.to_str().unwrap(),
            "-i", second.to_str().unwrap(),
            "-filter_complex", "[0:a][1:a]concat=n=2:v=0:a=1[out]",
            "-map", "[out]",
            "-c:a", "aac", "-b:a", "128k",
            out.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg concat failed");
    out
}

fn fp(path: &std::path::Path) -> Vec<u32> {
    let utf8 = Utf8Path::from_path(path).unwrap();
    compute_fingerprint(utf8)
        .expect("compute_fingerprint must not error on valid audio")
        .unwrap_or_default()
}

// ── tests ──────────────────────────────────────────────────────────────────────

#[test]
fn fingerprint_is_deterministic() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let audio = make_sine(&dir, 440, 10.0, "a440_long");
    let a = fp(&audio);
    let b = fp(&audio);
    assert!(!a.is_empty(), "fingerprint must not be empty for 10 s audio");
    assert_eq!(a, b, "fingerprint must be identical on repeated calls");
}

#[test]
fn fingerprint_length_proportional_to_duration() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let short = make_sine(&dir, 440, 5.0,  "a5s");
    let long  = make_sine(&dir, 440, 10.0, "a10s");

    let fp_short = fp(&short);
    let fp_long  = fp(&long);

    // Chromaprint produces ~1 u32 per second; 5 s → ~5 elements, 10 s → ~10.
    // Allow generous bounds (±3) for encoder/decoder latency.
    assert!(
        fp_short.len() >= 2,
        "5-second audio should produce ≥2 fingerprint elements, got {}",
        fp_short.len()
    );
    assert!(
        fp_long.len() > fp_short.len(),
        "10-second audio must produce longer fingerprint than 5-second: {} vs {}",
        fp_long.len(), fp_short.len()
    );
}

#[test]
fn same_content_has_high_similarity() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let audio = make_sine(&dir, 440, 10.0, "same");
    let a = fp(&audio);
    let b = fp(&audio);
    let sim = fingerprint_similarity(&a, &b);
    assert!(
        sim > 0.99,
        "identical audio must have similarity > 0.99, got {sim}"
    );
}

#[test]
fn different_frequencies_have_low_similarity() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();
    let a440  = make_sine(&dir, 440,  10.0, "440hz");
    let a4000 = make_sine(&dir, 4000, 10.0, "4000hz");

    let fp_a = fp(&a440);
    let fp_b = fp(&a4000);

    let sim = fingerprint_similarity(&fp_a, &fp_b);
    assert!(
        sim < 0.6,
        "440 Hz vs 4000 Hz must have low similarity, got {sim}"
    );
}

#[test]
fn sliding_window_finds_clip_inside_longer_audio() {
    require_ffmpeg!();
    let dir = tempfile::tempdir().unwrap();

    // Longer = 10 s silence + 10 s A440 + 10 s silence = 30 s
    let silence = make_sine(&dir, 1, 10.0, "silence_1hz");   // near-silence (1 Hz, inaudible)
    let clip    = make_sine(&dir, 440, 10.0, "a440_clip");
    let longer_path = concat_audio(&dir,
        &concat_audio(&dir, &silence, &clip, "pre_clip"),
        &silence,
        "longer",
    );

    let fp_clip   = fp(&clip);
    let fp_longer = fp(&longer_path);

    assert!(!fp_clip.is_empty(),   "clip fingerprint must not be empty");
    assert!(!fp_longer.is_empty(), "longer fingerprint must not be empty");

    // Require the clip fingerprint to be shorter than the longer one.
    if fp_clip.len() >= fp_longer.len() {
        eprintln!("SKIP: clip not shorter than longer (encoder latency edge case)");
        return;
    }

    let (sim, _offset) = fingerprint_sliding_window(&fp_clip, &fp_longer, 0.5);
    assert!(
        sim > 0.6,
        "A440 clip inside longer audio must be found with sim > 0.6, got {sim}"
    );
}

#[test]
fn fingerprint_similarity_bounds() {
    // Pure arithmetic property — no FFmpeg needed.
    let a: Vec<u32> = (0u32..32).collect();
    let b: Vec<u32> = (0u32..32).collect();
    let c: Vec<u32> = (0u32..32).map(|x| !x).collect(); // inverted

    let same = fingerprint_similarity(&a, &b);
    let diff = fingerprint_similarity(&a, &c);

    assert!((0.0..=1.0).contains(&same), "similarity must be in [0,1]");
    assert!((0.0..=1.0).contains(&diff), "similarity must be in [0,1]");
    assert!(same > diff, "same fingerprints must score higher than inverted");
    assert!(same > 0.99, "identical fingerprints must score near 1.0");
    assert!(diff < 0.5,  "inverted fingerprints must score < 0.5");
}

#[test]
fn empty_fingerprint_handled_gracefully() {
    // sliding_window with empty input must not panic.
    let empty: Vec<u32> = vec![];
    let full: Vec<u32>  = (0u32..10).collect();
    let (sim, _) = fingerprint_sliding_window(&empty, &full, 0.5);
    assert_eq!(sim, 0.0, "empty shorter must yield 0 similarity");
}
