//! Property-based tests for the sliding-window I-frame comparison engine.
//!
//! Uses proptest's TestRunner API directly (no proptest! or prop_assert! macros)
//! because the crate is named `core`, which shadows Rust's stdlib `core` crate
//! and causes any macro emitting `core::` paths to fail name resolution.

use core::comparison::{arrays_match, sliding_window_compare, MatchResult};
use proptest::test_runner::{Config, TestCaseError, TestRunner};

const PIXELS: usize = 32 * 32;

fn pattern(seed: u8) -> [u8; PIXELS] {
    let mut g = [0u8; PIXELS];
    for (i, b) in g.iter_mut().enumerate() {
        *b = ((i as u64 * 13 + seed as u64 * 97) % 256) as u8;
    }
    g
}

fn phash(seed: u8) -> u64 {
    core::phash::compute_phash(&pattern(seed))
}

fn fail(msg: impl std::fmt::Display) -> std::result::Result<(), TestCaseError> {
    Err(TestCaseError::fail(msg.to_string()))
}

fn missing() -> MatchResult {
    MatchResult { similarity: 0.0, offset: 0, consecutive_run: 0 }
}

// ── Invariant 1: exact subclip always detected ────────────────────────────────

#[test]
fn exact_subclip_always_detected() {
    let mut runner = TestRunner::new(Config::with_cases(512));
    runner
        .run(
            &(5usize..=50usize, 0usize..=40usize, 1usize..=20usize, 0u8..=200u8),
            |(longer_len, raw_offset, raw_clip_len, seed_base)| {
                let offset = raw_offset % longer_len;
                let max_clip = (longer_len - offset).max(1);
                let clip_len = (raw_clip_len % max_clip).max(1);

                let longer: Vec<u64> = (0..longer_len)
                    .map(|i| phash(seed_base.wrapping_add(i as u8)))
                    .collect();
                let shorter = longer[offset..offset + clip_len].to_vec();

                let result = sliding_window_compare(&shorter, &longer, 0.99, 0);
                if result.is_none() {
                    return fail(format!(
                        "exact subclip len={clip_len} offset={offset} in longer={longer_len} must be found"
                    ));
                }
                let r = result.unwrap();
                if r.similarity < 0.99 {
                    return fail(format!(
                        "exact match similarity must be ≥ 0.99, got {}",
                        r.similarity
                    ));
                }
                Ok(())
            },
        )
        .unwrap();
}

// ── Invariant 2: identical sequences → offset 0, similarity 1.0 ──────────────

#[test]
fn identical_sequences_match_at_offset_zero() {
    let mut runner = TestRunner::new(Config::with_cases(256));
    runner
        .run(&(1usize..=30usize, 0u8..=200u8), |(len, seed)| {
            let seq: Vec<u64> = (0..len)
                .map(|i| phash(seed.wrapping_add(i as u8)))
                .collect();

            let r = match sliding_window_compare(&seq, &seq, 0.99, 0) {
                Some(r) => r,
                None => return fail("identical sequences must produce Some"),
            };

            if r.similarity < 0.99 {
                return fail(format!("expected sim ≥ 0.99, got {}", r.similarity));
            }
            if r.offset != 0 {
                return fail(format!("expected offset 0, got {}", r.offset));
            }
            if r.consecutive_run != len {
                return fail(format!(
                    "expected consecutive_run={len}, got {}",
                    r.consecutive_run
                ));
            }
            Ok(())
        })
        .unwrap();
}

// ── Invariant 3: single gap within max_gap=1 does not break detection ─────────

#[test]
fn single_gap_within_budget_does_not_break_run() {
    let mut runner = TestRunner::new(Config::with_cases(256));
    runner
        .run(&(6usize..=30usize, 1usize..=28usize, 0u8..=200u8), |(run_len, raw_pos, seed)| {
            let gap_pos = (raw_pos % (run_len - 1)).max(1);

            let noise: u64 = !0u64;
            let mut longer: Vec<u64> = (0..run_len)
                .map(|i| phash(seed.wrapping_add(i as u8)))
                .collect();
            longer[gap_pos] = noise;

            let shorter: Vec<u64> = (0..run_len)
                .map(|i| phash(seed.wrapping_add(i as u8)))
                .collect();

            let r = match sliding_window_compare(&shorter, &longer, 0.95, 1) {
                Some(r) => r,
                None => return fail(format!(
                    "single gap pos={gap_pos} run_len={run_len} max_gap=1 must produce Some"
                )),
            };

            let expected_min = (run_len - 1) as f32 / run_len as f32 - 0.02;
            if r.similarity < expected_min {
                return fail(format!(
                    "sim {} should be ≥ {} for single-gap run_len={run_len}",
                    r.similarity, expected_min
                ));
            }
            Ok(())
        })
        .unwrap();
}

// ── Invariant 4: strict mode limits consecutive run at noise frame ─────────────

#[test]
fn strict_mode_limits_consecutive_run() {
    let mut runner = TestRunner::new(Config::with_cases(256));
    runner
        .run(&(4usize..=20usize, 1usize..=18usize, 0u8..=200u8), |(run_len, raw_pos, seed)| {
            let gap_pos = (raw_pos % (run_len - 1)).max(1);

            let noise: u64 = !0u64;
            let mut longer: Vec<u64> = (0..run_len)
                .map(|i| phash(seed.wrapping_add(i as u8)))
                .collect();
            longer[gap_pos] = noise;

            let shorter: Vec<u64> = (0..run_len)
                .map(|i| phash(seed.wrapping_add(i as u8)))
                .collect();

            if let Some(r) = sliding_window_compare(&shorter, &longer, 0.95, 0) {
                let max_clean = gap_pos.max(run_len - gap_pos - 1);
                if r.consecutive_run > max_clean {
                    return fail(format!(
                        "strict mode consecutive_run {} > max_clean {} (gap={}, len={})",
                        r.consecutive_run, max_clean, gap_pos, run_len
                    ));
                }
            }
            Ok(())
        })
        .unwrap();
}

// ── Invariant 5: lower hash_threshold → similarity never decreases ────────────

#[test]
fn lowering_threshold_never_decreases_similarity() {
    let mut runner = TestRunner::new(Config::with_cases(256));
    runner
        .run(&(5usize..=40usize, 2usize..=15usize, 0u8..=200u8), |(longer_len, raw_clip, seed)| {
            let clip_len = raw_clip.min(longer_len);
            let longer: Vec<u64> = (0..longer_len)
                .map(|i| phash(seed.wrapping_add(i as u8)))
                .collect();
            let shorter = longer[0..clip_len].to_vec();

            let strict = sliding_window_compare(&shorter, &longer, 0.99, 0).unwrap_or_else(missing);
            let loose  = sliding_window_compare(&shorter, &longer, 0.50, 0).unwrap_or_else(missing);

            if loose.similarity < strict.similarity - 1e-6 {
                return fail(format!(
                    "loose sim {} < strict sim {} (threshold monotonicity violated)",
                    loose.similarity, strict.similarity
                ));
            }
            Ok(())
        })
        .unwrap();
}

// ── Invariant 6: arrays_match monotone in hash_threshold ─────────────────────

#[test]
fn arrays_match_monotone_in_hash_threshold() {
    let mut runner = TestRunner::new(Config::with_cases(256));
    runner
        .run(&(5usize..=30usize, 1usize..=10usize, 0u8..=200u8), |(longer_len, raw_clip, seed)| {
            let clip_len = raw_clip.min(longer_len);
            let longer: Vec<u64> = (0..longer_len)
                .map(|i| phash(seed.wrapping_add(i as u8)))
                .collect();
            let shorter = longer[0..clip_len].to_vec();

            let strict = arrays_match(&shorter, &longer, 0.9, 1, 0.99, 0);
            let loose  = arrays_match(&shorter, &longer, 0.9, 1, 0.50, 0);

            if strict.is_some() && loose.is_none() {
                return fail("looser hash_threshold must not reject what strict accepted");
            }
            Ok(())
        })
        .unwrap();
}
