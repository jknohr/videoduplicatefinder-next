//! Perceptual hashing via separated DCT (pHash).
//!
//! Faithful port of VDF.Core/pHash/PerceptualHash.cs.
//! Pipeline: 32×32 grayscale bytes → 2-pass separated DCT keeping only the
//! K×K=8×8 low-frequency AC coefficients → median threshold → 64-bit hash.
//!
//! Hash values are bit-identical to the C# implementation so cached hashes
//! produced by the original VDF remain valid.

use std::sync::OnceLock;

const N: usize = 32; // working size
const K: usize = 8;  // low-frequency block size (frequencies 1..=K)
const PIXELS: usize = N * N;

// Cos[k * N + i] = cos(((2i+1) * k * π) / (2N))  for k, i in 0..N
static COS_TABLE: OnceLock<Box<[f32; N * N]>> = OnceLock::new();

// Alpha[0] = sqrt(1/N),  Alpha[k>0] = sqrt(2/N)
static ALPHA: OnceLock<Box<[f32; N]>> = OnceLock::new();

fn cos_table() -> &'static [f32; N * N] {
    COS_TABLE.get_or_init(|| {
        let mut t = Box::new([0f32; N * N]);
        for k in 0..N {
            for i in 0..N {
                let angle = ((2 * i + 1) * k) as f64 * std::f64::consts::PI / (2.0 * N as f64);
                t[k * N + i] = angle.cos() as f32;
            }
        }
        t
    })
}

fn alpha_table() -> &'static [f32; N] {
    ALPHA.get_or_init(|| {
        let inv_n = 1.0f64 / N as f64;
        let mut a = Box::new([0f32; N]);
        a[0] = inv_n.sqrt() as f32;
        for k in 1..N {
            a[k] = (2.0 * inv_n).sqrt() as f32;
        }
        a
    })
}

/// Compute a 64-bit perceptual hash from a 32×32 grayscale image.
///
/// `gray` must be exactly 1024 bytes (32×32), row-major.
/// Output is bit-identical to `PerceptualHash.ComputePHashFromGray32x32` in C#.
pub fn compute_phash(gray: &[u8; PIXELS]) -> u64 {
    let cos = cos_table();
    let alpha = alpha_table();

    // Row DCT pass: for each row y, compute K outputs (u in 1..=K).
    // Compact layout temp[y * K + (u - 1)] omits the unused columns 0 and K+1..N.
    let mut temp = [0f32; N * K];
    for y in 0..N {
        let y_base = y * N;
        let t_base = y * K;
        for u in 1..=K {
            let cos_base = u * N;
            let mut sum = 0f32;
            for x in 0..N {
                sum += gray[y_base + x] as f32 * cos[cos_base + x];
            }
            temp[t_base + (u - 1)] = alpha[u] * sum;
        }
    }

    // Column DCT pass: K×K outputs (v outer, u inner — same sweep order as C#
    // so bit positions in `hash` are identical to the reference implementation).
    let mut ac = [0f32; K * K];
    let mut k_idx = 0usize;
    for v in 1..=K {
        let cos_base = v * N;
        let alpha_v = alpha[v];
        for u in 1..=K {
            let tu = u - 1;
            let mut sum = 0f32;
            for y in 0..N {
                sum += temp[y * K + tu] * cos[cos_base + y];
            }
            ac[k_idx] = alpha_v * sum;
            k_idx += 1;
        }
    }

    // Median of 64 AC values: sort a copy, take average of middle two elements.
    let median = median64(&ac);

    let mut hash = 0u64;
    for i in 0..64 {
        if ac[i] > median {
            hash |= 1u64 << i;
        }
    }
    hash
}

fn median64(values: &[f32; K * K]) -> f32 {
    let mut buf = *values;
    buf.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (buf[31] + buf[32]) * 0.5
}

/// Hamming distance between two pHash values.
#[inline(always)]
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Similarity as a fraction [0.0, 1.0] where 1.0 = identical.
#[inline(always)]
pub fn similarity(a: u64, b: u64) -> f32 {
    1.0 - hamming(a, b) as f32 / 64.0
}

/// Returns true if `a` and `b` are duplicates at the given similarity threshold.
///
/// Matches `IsDuplicateByPercent` in C#: duplicate if `hamming(a,b) ≤ floor((1−percent)×64)`.
pub fn is_duplicate(a: u64, b: u64, min_similarity: f32) -> bool {
    let max_bits = ((1.0 - min_similarity) * 64.0).floor() as u32;
    hamming(a, b) <= max_bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_produce_identical_hash() {
        let gray = [128u8; PIXELS];
        assert_eq!(compute_phash(&gray), compute_phash(&gray));
    }

    #[test]
    fn identical_hashes_have_zero_hamming() {
        let h = compute_phash(&[64u8; PIXELS]);
        assert_eq!(hamming(h, h), 0);
    }

    #[test]
    fn different_images_differ() {
        let mut a = [0u8; PIXELS];
        let mut b = [0u8; PIXELS];
        for i in 0..PIXELS {
            a[i] = (i % 256) as u8;
            b[i] = ((i * 3 + 100) % 256) as u8;
        }
        let ha = compute_phash(&a);
        let hb = compute_phash(&b);
        assert!(hamming(ha, hb) > 4, "expected significant hamming distance");
    }

    #[test]
    fn similar_images_have_high_similarity() {
        let mut a = [0u8; PIXELS];
        let mut b = [0u8; PIXELS];
        for i in 0..PIXELS {
            a[i] = (i % 256) as u8;
            // Very slight brightness shift — structure is identical
            b[i] = ((i % 256) as u8).saturating_add(5);
        }
        let ha = compute_phash(&a);
        let hb = compute_phash(&b);
        assert!(similarity(ha, hb) > 0.7, "similar images should have high similarity");
    }

    #[test]
    fn uniform_image_yields_zero_hash() {
        // Uniform image: all AC coefficients = 0, median = 0, no bit set.
        let h = compute_phash(&[128u8; PIXELS]);
        assert_eq!(h, 0);
    }

    #[test]
    fn is_duplicate_identical() {
        let h = compute_phash(&[42u8; PIXELS]);
        assert!(is_duplicate(h, h, 0.95));
    }

    #[test]
    fn is_duplicate_rejects_max_distance() {
        assert!(!is_duplicate(0u64, !0u64, 0.95));
    }
}
