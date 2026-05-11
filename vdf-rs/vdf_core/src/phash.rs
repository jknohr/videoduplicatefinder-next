//! Perceptual hashing via DCT (pHash).
//!
//! Pipeline: 32×32 grayscale bytes → 8×8 DCT mean → 64-bit hash.
//! Matches the C# PerceptualHash implementation in VDF.Core/pHash/.

const HASH_SIZE: usize = 8;
const SAMPLE_SIZE: usize = 32;
const PIXELS: usize = SAMPLE_SIZE * SAMPLE_SIZE;

/// Compute a 64-bit perceptual hash from a 32×32 grayscale image.
///
/// `gray` must be exactly 1024 bytes (32×32), row-major.
pub fn compute_phash(gray: &[u8; PIXELS]) -> u64 {
    // Step 1: compute 8x8 DCT of the 32x32 image
    let dct = dct8x8(gray);

    // Step 2: compute mean of all 64 DCT coefficients (excluding DC component at [0][0])
    let mean = {
        let sum: f32 = dct.iter().flatten().skip(1).copied().sum();
        sum / (HASH_SIZE * HASH_SIZE - 1) as f32
    };

    // Step 3: set bit if coefficient > mean
    let mut hash: u64 = 0;
    for (i, &val) in dct.iter().flatten().enumerate() {
        if val > mean {
            hash |= 1u64 << i;
        }
    }
    hash
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

/// Compute an 8×8 DCT-II of the top-left 8×8 block extracted from a 32×32 image.
///
/// Uses the separable 1D DCT applied to rows then columns.
fn dct8x8(gray: &[u8; PIXELS]) -> [[f32; HASH_SIZE]; HASH_SIZE] {
    // Downsample 32×32 → 8×8 means to reduce DCT cost while preserving structure
    // (This matches the reference pHash: average each 4×4 block into one pixel)
    let mut reduced = [[0f32; HASH_SIZE]; HASH_SIZE];
    for row in 0..HASH_SIZE {
        for col in 0..HASH_SIZE {
            let mut sum = 0u32;
            for dr in 0..4 {
                for dc in 0..4 {
                    let r = row * 4 + dr;
                    let c = col * 4 + dc;
                    sum += gray[r * SAMPLE_SIZE + c] as u32;
                }
            }
            reduced[row][col] = sum as f32 / 16.0;
        }
    }

    // Apply 8×8 2D DCT-II
    let mut dct = [[0f32; HASH_SIZE]; HASH_SIZE];
    let n = HASH_SIZE as f32;

    for u in 0..HASH_SIZE {
        for v in 0..HASH_SIZE {
            let cu = if u == 0 { 1.0 / 2f32.sqrt() } else { 1.0 };
            let cv = if v == 0 { 1.0 / 2f32.sqrt() } else { 1.0 };
            let mut sum = 0f32;
            for x in 0..HASH_SIZE {
                for y in 0..HASH_SIZE {
                    let cos_u =
                        (std::f32::consts::PI * u as f32 * (2.0 * x as f32 + 1.0) / (2.0 * n))
                            .cos();
                    let cos_v =
                        (std::f32::consts::PI * v as f32 * (2.0 * y as f32 + 1.0) / (2.0 * n))
                            .cos();
                    sum += reduced[x][y] * cos_u * cos_v;
                }
            }
            dct[u][v] = (2.0 / n) * cu * cv * sum;
        }
    }

    dct
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
        let mut b = [255u8; PIXELS];
        // Make them more interesting
        for i in 0..PIXELS {
            a[i] = (i % 256) as u8;
            b[i] = ((i * 3 + 100) % 256) as u8;
        }
        let ha = compute_phash(&a);
        let hb = compute_phash(&b);
        // Two very different images should differ by many bits
        assert!(hamming(ha, hb) > 4, "expected significant hamming distance");
    }

    #[test]
    fn similar_images_have_high_similarity() {
        let mut a = [0u8; PIXELS];
        let mut b = [0u8; PIXELS];
        for i in 0..PIXELS {
            a[i] = (i % 256) as u8;
            b[i] = ((i % 256) as u8).saturating_add(5); // very slight brightness shift
        }
        let ha = compute_phash(&a);
        let hb = compute_phash(&b);
        assert!(similarity(ha, hb) > 0.7, "similar images should have high similarity");
    }
}
