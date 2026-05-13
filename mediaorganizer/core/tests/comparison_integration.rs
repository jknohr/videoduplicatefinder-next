//! Integration tests for the sliding-window I-frame comparison engine.
//!
//! These validate the `arrays_match` / `sliding_window_compare` contract that
//! the scan engine relies on — specifically the "partial clip inside longer
//! video" detection use case.

use core::comparison::{arrays_match, sliding_window_compare};
use core::phash::{compute_phash, hamming};

const PIXELS: usize = 32 * 32;

// ── Exact-subsequence detection ────────────────────────────────────────────────

#[test]
fn exact_clip_inside_longer_video_detected() {
    // Simulate 50-frame long video; the short clip occupies frames 20–29.
    let longer: Vec<u64> = (0u64..50)
        .map(|i| compute_phash(&pattern(i as u8)))
        .collect();
    let shorter = longer[20..30].to_vec();

    let result = sliding_window_compare(&shorter, &longer, 0.95, 0)
        .expect("exact subsequence must be found");

    assert_eq!(result.offset, 20, "clip starts at frame 20");
    assert_eq!(result.consecutive_run, 10, "10 consecutive frames");
    assert!(result.similarity > 0.99, "exact match → similarity near 1.0");
}

#[test]
fn clip_at_start_of_longer_video_detected() {
    let longer: Vec<u64> = (0u64..30)
        .map(|i| compute_phash(&pattern(i as u8)))
        .collect();
    let shorter = longer[0..10].to_vec();

    let result = sliding_window_compare(&shorter, &longer, 0.95, 0)
        .expect("prefix clip must be found");

    assert_eq!(result.offset, 0);
    assert_eq!(result.consecutive_run, 10);
}

#[test]
fn clip_at_end_of_longer_video_detected() {
    let longer: Vec<u64> = (0u64..30)
        .map(|i| compute_phash(&pattern(i as u8)))
        .collect();
    let shorter = longer[20..30].to_vec();

    let result = sliding_window_compare(&shorter, &longer, 0.95, 0)
        .expect("suffix clip must be found");

    assert_eq!(result.offset, 20);
    assert_eq!(result.consecutive_run, 10);
}

// ── Noise tolerance ────────────────────────────────────────────────────────────

#[test]
fn noisy_clip_detected_above_threshold() {
    // Shorter is the same content with 1 bit of noise per hash.
    let longer: Vec<u64> = (0u64..30)
        .map(|i| compute_phash(&pattern(i as u8)))
        .collect();
    // Add 1 bit of noise to each frame of the clip (frames 10..20).
    let shorter: Vec<u64> = longer[10..20]
        .iter()
        .map(|&h| h ^ 1u64) // flip LSB
        .collect();

    // 1-bit flip: similarity = 1 - 1/64 ≈ 0.984 → must pass at 0.95
    let result = sliding_window_compare(&shorter, &longer, 0.95, 0)
        .expect("noisy clip should still match");

    assert_eq!(result.offset, 10);
    assert!(result.similarity > 0.95);
}

#[test]
fn heavily_different_content_not_matched() {
    let longer: Vec<u64> = (0u64..30)
        .map(|i| compute_phash(&pattern(i as u8)))
        .collect();
    // Random-looking hashes (bit inversions of each frame)
    let shorter: Vec<u64> = longer[0..10]
        .iter()
        .map(|&h| !h) // invert all bits — maximum distance
        .collect();

    // With threshold 0.85 and maximum distance, should not match
    let result = sliding_window_compare(&shorter, &longer, 0.85, 0);
    match result {
        None => { /* no match — OK */ }
        Some(r) => {
            assert!(
                r.similarity < 0.5,
                "inverted content should have very low similarity, got {}", r.similarity
            );
        }
    }
}

// ── arrays_match — the full public API used by the scan engine ─────────────────

#[test]
fn arrays_match_requires_minimum_consecutive_frames() {
    let longer: Vec<u64> = (0u64..30)
        .map(|i| compute_phash(&pattern(i as u8)))
        .collect();
    let shorter = longer[10..20].to_vec();

    // min_consecutive = 5 → should match (we have 10 consecutive)
    assert!(
        arrays_match(&shorter, &longer, 0.95, 5, 0.8, 0).is_some(),
        "min_consecutive=5 should match 10-frame clip"
    );

    // min_consecutive = 15 → should not match (only 10 consecutive)
    assert!(
        arrays_match(&shorter, &longer, 0.95, 15, 0.8, 0).is_none(),
        "min_consecutive=15 should not match 10-frame clip"
    );
}

#[test]
fn arrays_match_min_similarity_threshold() {
    let longer: Vec<u64> = (0u64..20)
        .map(|i| compute_phash(&pattern(i as u8)))
        .collect();
    let shorter = longer[5..10].to_vec();

    // Very high required match percent (0.99) with only 5 consecutive frames
    // meeting the threshold — should still match since all 5 are exact.
    let result = arrays_match(&shorter, &longer, 0.95, 3, 0.99, 0);
    assert!(result.is_some(), "exact frames should pass very high percent threshold");
}

#[test]
fn arrays_match_with_gap_tolerance() {
    let a: u64 = 0x0000000000000000;
    let x: u64 = !0; // max distance from 'a'
    // longer: [a a a a a X a a a a a] — one bad frame at position 5
    let longer = vec![a, a, a, a, a, x, a, a, a, a, a];
    // shorter: [a a a a a a a a a a] — 10 clean frames
    let shorter = vec![a; 10];

    // Without gap tolerance — the X breaks the run
    let strict = arrays_match(&shorter, &longer, 0.9, 8, 0.8, 0);
    assert!(strict.is_none(), "strict match should fail (run broken by X)");

    // With gap=1 — X is bridged
    let lenient = arrays_match(&shorter, &longer, 0.9, 8, 0.8, 1);
    assert!(lenient.is_some(), "gap=1 should bridge the single bad frame");
}

// ── Edge cases ────────────────────────────────────────────────────────────────

#[test]
fn empty_shorter_returns_none_from_sliding_window() {
    let longer = vec![1u64, 2, 3, 4, 5];
    assert!(sliding_window_compare(&[], &longer, 0.9, 0).is_none());
}

#[test]
fn shorter_longer_than_longer_returns_none() {
    let a = vec![1u64; 10];
    let b = vec![1u64; 5];
    assert!(sliding_window_compare(&a, &b, 0.9, 0).is_none());
}

#[test]
fn single_frame_clip_detection() {
    let longer: Vec<u64> = (0u64..20)
        .map(|i| compute_phash(&pattern(i as u8)))
        .collect();
    let shorter = vec![longer[10]];

    let result = sliding_window_compare(&shorter, &longer, 0.99, 0)
        .expect("single-frame exact match must be found");
    assert_eq!(result.offset, 10);
    assert_eq!(result.consecutive_run, 1);
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Generate a synthetic 32×32 grayscale pattern distinguishable by `seed`.
fn pattern(seed: u8) -> [u8; PIXELS] {
    let mut g = [0u8; PIXELS];
    for (i, b) in g.iter_mut().enumerate() {
        *b = ((i as u64 * 13 + seed as u64 * 97) % 256) as u8;
    }
    g
}
