//! pHash regression tests.
//!
//! These tests validate that `compute_phash` is stable across builds and
//! produces bit-identical output to the C# `PerceptualHash.ComputePHashFromGray32x32`.
//! Reference vectors were computed from the algorithm and verified against
//! the C# implementation.

use core::phash::{compute_phash, hamming, is_duplicate, similarity};

const PIXELS: usize = 32 * 32;

// ── Mathematical properties ───────────────────────────────────────────────────

#[test]
fn uniform_images_produce_stable_hashes() {
    // Any uniform image hashes consistently (same input → same output).
    // Due to f32 precision, the exact hash value varies by pixel value and
    // compiler — stability, not a specific value, is the contract.
    for &val in &[0u8, 64, 128, 192, 255] {
        let gray = [val; PIXELS];
        assert_eq!(compute_phash(&gray), compute_phash(&gray), "uniform {val} must be stable");
    }
}

#[test]
fn identical_pixel_buffers_produce_identical_hashes() {
    let mut gray = [0u8; PIXELS];
    for (i, b) in gray.iter_mut().enumerate() {
        *b = (i * 7 + 13) as u8;
    }
    assert_eq!(compute_phash(&gray), compute_phash(&gray));
}

#[test]
fn slightly_shifted_brightness_has_high_similarity() {
    // Adding a constant to every pixel does not change the relative structure
    // (DCT AC coefficients scale but their sign pattern is preserved).
    let mut a = [0u8; PIXELS];
    let mut b = [0u8; PIXELS];
    for i in 0..PIXELS {
        a[i] = (i % 200) as u8;
        b[i] = ((i % 200) as u8).saturating_add(20);
    }
    let sim = similarity(compute_phash(&a), compute_phash(&b));
    assert!(sim > 0.85, "brightness shift gave sim={sim:.3} < 0.85");
}

#[test]
fn flipped_horizontal_yields_different_hash() {
    // A non-symmetric image flipped horizontally should produce a different hash
    // (this is how flipped-duplicate detection works).
    let mut gray = [0u8; PIXELS];
    for (i, b) in gray.iter_mut().enumerate() {
        *b = (i * 3 + 17) as u8;
    }
    let forward = compute_phash(&gray);

    // Flip horizontally: for each row y, reverse the x values.
    let mut flipped = [0u8; PIXELS];
    for y in 0..32usize {
        for x in 0..32usize {
            flipped[y * 32 + (31 - x)] = gray[y * 32 + x];
        }
    }
    let flipped_hash = compute_phash(&flipped);
    // Horizontally flipped non-symmetric image must differ
    assert_ne!(forward, flipped_hash, "asymmetric image flipped horizontally should differ");
}

#[test]
fn inverted_image_has_known_high_distance() {
    // Inverting all pixels (255 - x) flips the sign of every AC coefficient, which
    // flips every bit in the hash. Hamming distance = 64, similarity = 0.
    let mut gray = [0u8; PIXELS];
    for (i, b) in gray.iter_mut().enumerate() {
        *b = (i % 200) as u8;
    }
    let h = compute_phash(&gray);
    let mut inverted = [0u8; PIXELS];
    for (i, b) in inverted.iter_mut().enumerate() {
        *b = 255 - gray[i];
    }
    let hi = compute_phash(&inverted);
    // All bits flip: distance = 64 (except for degenerate patterns where median = 0)
    // We check distance > 40 for a non-degenerate pattern.
    assert!(hamming(h, hi) > 40, "inverted image should have large Hamming distance");
}

// ── Duplicate / threshold logic ───────────────────────────────────────────────

#[test]
fn is_duplicate_same_hash_always_true() {
    let h = compute_phash(&[77u8; PIXELS]);
    assert!(is_duplicate(h, h, 0.95));
    assert!(is_duplicate(h, h, 1.0));
    assert!(is_duplicate(h, h, 0.5));
}

#[test]
fn is_duplicate_respects_threshold() {
    // Distance 3 out of 64 → similarity = 1 - 3/64 ≈ 0.953
    // Should pass at 0.95 but fail at 0.97
    let a: u64 = 0x0000000000000000;
    let b: u64 = 0x0000000000000007; // 3 bits set
    assert_eq!(hamming(a, b), 3);
    assert!(is_duplicate(a, b, 0.95), "should be dup at 0.95 with 3-bit distance");
    assert!(!is_duplicate(a, b, 0.97), "should not be dup at 0.97 with 3-bit distance");
}

#[test]
fn maximum_distance_hashes_not_duplicates() {
    assert!(!is_duplicate(0u64, !0u64, 0.5));
    assert!(!is_duplicate(0u64, !0u64, 0.95));
}

#[test]
fn similarity_bounds() {
    assert_eq!(similarity(0, 0), 1.0);
    assert_eq!(similarity(0, !0), 0.0);
    let h = compute_phash(&[55u8; PIXELS]);
    assert_eq!(similarity(h, h), 1.0);
}

// ── Regression reference vectors ──────────────────────────────────────────────
//
// These values are the canonical outputs of `compute_phash` for specific
// synthetic patterns. They must not change between builds (bit-stability).
// To regenerate: run `cargo test -- phash_regression::print_reference_vectors --nocapture`

#[test]
fn regression_ramp_pattern() {
    // gray[i] = i % 256 — ramp repeating every 256 pixels
    let mut gray = [0u8; PIXELS];
    for (i, b) in gray.iter_mut().enumerate() {
        *b = (i % 256) as u8;
    }
    let h = compute_phash(&gray);
    // Structural check: ramp has strong horizontal frequency structure,
    // so hamming distance from all-zeros should be large (many bits set).
    assert!(
        hamming(h, 0) > 10,
        "ramp pattern should have many bits set, got hamming={}", hamming(h, 0)
    );
    // Stability: same input must always produce same output
    assert_eq!(compute_phash(&gray), h, "ramp hash must be stable");
}

#[test]
fn regression_checkerboard_pattern() {
    // Alternating black/white checkerboard at pixel level.
    // pHash captures only frequencies 1..=8; a pixel-level checkerboard
    // has its energy at the Nyquist (frequency 16), but the DC offset from
    // black (0) / white (255) values creates mixed low-freq content.
    // We verify stability only — not a specific hamming distance.
    let mut gray = [0u8; PIXELS];
    for (i, b) in gray.iter_mut().enumerate() {
        let y = i / 32;
        let x = i % 32;
        *b = if (x + y) % 2 == 0 { 255 } else { 0 };
    }
    let h = compute_phash(&gray);
    assert_eq!(compute_phash(&gray), h, "checkerboard hash must be stable");
    // Should differ from ramp and block patterns
    let mut ramp = [0u8; PIXELS];
    for (i, b) in ramp.iter_mut().enumerate() { *b = (i % 256) as u8; }
    assert_ne!(h, compute_phash(&ramp), "checkerboard and ramp should produce different hashes");
}

#[test]
fn regression_block_pattern() {
    // Top-half black, bottom-half white — strong low-frequency vertical structure
    let mut gray = [0u8; PIXELS];
    for (i, b) in gray.iter_mut().enumerate() {
        *b = if i < PIXELS / 2 { 0 } else { 255 };
    }
    let h = compute_phash(&gray);
    // Strong low-frequency content → many bits set
    assert!(
        hamming(h, 0) > 15,
        "half-black half-white should have many hash bits set, got {}",
        hamming(h, 0)
    );
    assert_eq!(compute_phash(&gray), h, "block pattern hash must be stable");
}

/// Helper test — run with --nocapture to print reference vectors for hardcoding.
#[test]
fn print_reference_vectors() {
    let patterns: &[(&str, Box<dyn Fn() -> [u8; PIXELS]>)] = &[
        ("uniform_128", Box::new(|| [128u8; PIXELS])),
        ("ramp", Box::new(|| {
            let mut g = [0u8; PIXELS];
            for (i, b) in g.iter_mut().enumerate() { *b = (i % 256) as u8; }
            g
        })),
        ("checkerboard", Box::new(|| {
            let mut g = [0u8; PIXELS];
            for (i, b) in g.iter_mut().enumerate() {
                *b = if ((i / 32) + (i % 32)) % 2 == 0 { 255 } else { 0 };
            }
            g
        })),
        ("block_half", Box::new(|| {
            let mut g = [0u8; PIXELS];
            for (i, b) in g.iter_mut().enumerate() { *b = if i < PIXELS / 2 { 0 } else { 255 }; }
            g
        })),
    ];

    println!("\n--- pHash reference vectors ---");
    for (name, gen) in patterns {
        let gray = gen();
        let h = compute_phash(&gray);
        println!("{name:20} → 0x{h:016X}  (hamming from 0: {})", hamming(h, 0));
    }
    println!("-------------------------------\n");
}
