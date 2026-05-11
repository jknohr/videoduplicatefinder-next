//! Sliding-window timeline comparison for I-frame fingerprint matching.
//!
//! Ports VDF.Core/Utils/TemporalHashUtils.cs with the same algorithm.

use crate::phash::similarity;

/// Result of a sliding-window comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchResult {
    /// Best similarity score (fraction of `shorter` frames that matched).
    pub similarity: f32,
    /// Offset index into `longer` where the best match starts.
    pub offset: usize,
    /// Longest consecutive (gap-bridged) run of matching frames.
    pub consecutive_run: usize,
}

/// Slide `shorter` over `longer` and find the best matching offset.
///
/// - `hash_threshold`: per-frame similarity required to count as a match (0.0–1.0)
/// - `max_gap`: non-matching frames tolerated inside a run before the run resets
///   - 0 = strict consecutive (identical clip)
///   - 2 = re-edit tolerance (alternate shots)
///   - 5+ = shared source material
///
/// Returns `None` if `shorter` is empty or longer than `longer`.
pub fn sliding_window_compare(
    shorter: &[u64],
    longer: &[u64],
    hash_threshold: f32,
    max_gap: usize,
) -> Option<MatchResult> {
    if shorter.is_empty() || shorter.len() > longer.len() {
        return None;
    }

    let max_offsets = longer.len() - shorter.len() + 1;
    // Early-exit budget: allow at most this many misses per offset before skipping
    let miss_budget = shorter.len().max(1);

    let mut best = MatchResult { similarity: 0.0, offset: 0, consecutive_run: 0 };

    for offset in 0..max_offsets {
        let mut matches = 0usize;
        let mut consecutive = 0usize;
        let mut max_consecutive = 0usize;
        let mut gap = 0usize;
        let mut misses = 0usize;

        for (i, &sh) in shorter.iter().enumerate() {
            let sim = similarity(sh, longer[offset + i]);
            if sim >= hash_threshold {
                matches += 1;
                consecutive += 1;
                gap = 0;
                if consecutive > max_consecutive {
                    max_consecutive = consecutive;
                }
            } else {
                misses += 1;
                if max_gap > 0 && gap < max_gap {
                    // Bridge this non-match — run continues
                    gap += 1;
                } else {
                    consecutive = 0;
                    gap = 0;
                }
                // Early exit when too many misses accumulated
                if misses > miss_budget {
                    break;
                }
            }
        }

        let sim = matches as f32 / shorter.len() as f32;
        if sim > best.similarity {
            best = MatchResult {
                similarity: sim,
                offset,
                consecutive_run: max_consecutive,
            };
        }
    }

    Some(best)
}

/// Convenience: compare two pHash arrays and return whether they match given
/// the supplied thresholds.
pub fn arrays_match(
    shorter: &[u64],
    longer: &[u64],
    min_similarity: f32,
    min_consecutive: usize,
    hash_threshold: f32,
    max_gap: usize,
) -> Option<MatchResult> {
    let result = sliding_window_compare(shorter, longer, hash_threshold, max_gap)?;
    if result.similarity >= min_similarity && result.consecutive_run >= min_consecutive {
        Some(result)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_hashes(values: &[u64]) -> Vec<u64> {
        values.to_vec()
    }

    #[test]
    fn exact_subsequence_detected() {
        // shorter is a contiguous slice of longer — must find it
        let longer: Vec<u64> = (0u64..20).map(|i| i * 0x0101010101010101).collect();
        let shorter: Vec<u64> = longer[5..10].to_vec();
        let result = sliding_window_compare(&shorter, &longer, 0.9, 0).unwrap();
        assert_eq!(result.offset, 5);
        assert!(result.similarity > 0.99);
        assert_eq!(result.consecutive_run, 5);
    }

    #[test]
    fn no_match_returns_low_similarity() {
        let longer: Vec<u64> = (0u64..20).map(|i| i).collect();
        let shorter: Vec<u64> = (100u64..105).map(|i| i << 32).collect();
        let result = sliding_window_compare(&shorter, &longer, 0.85, 0).unwrap();
        assert!(result.similarity < 0.3);
    }

    #[test]
    fn empty_shorter_returns_none() {
        let longer = vec![1u64, 2, 3];
        assert!(sliding_window_compare(&[], &longer, 0.85, 0).is_none());
    }

    #[test]
    fn shorter_longer_than_longer_returns_none() {
        let a = vec![1u64, 2, 3, 4, 5];
        let b = vec![1u64, 2];
        assert!(sliding_window_compare(&a, &b, 0.85, 0).is_none());
    }

    #[test]
    fn gap_tolerance_bridges_single_alternate_frame() {
        // Build longer: [A A A X A A A] where X differs; shorter = [A A A A A A]
        let a: u64 = 0x0000000000000000;
        let x: u64 = 0xFFFFFFFFFFFFFFFF;
        let longer = vec![a, a, a, x, a, a, a];
        let shorter = vec![a; 6];
        // Without gap: consecutive run breaks at X → max consecutive = 3
        let strict = sliding_window_compare(&shorter, &longer, 0.9, 0).unwrap();
        assert!(strict.consecutive_run <= 3);
        // With gap=1: X is bridged → run of 6
        let tolerant = sliding_window_compare(&shorter, &longer, 0.9, 1).unwrap();
        assert_eq!(tolerant.consecutive_run, 6);
    }

    #[test]
    fn arrays_match_respects_min_consecutive() {
        let a: u64 = 0;
        let longer = vec![a; 20];
        let shorter = vec![a; 5];
        assert!(arrays_match(&shorter, &longer, 0.9, 3, 0.9, 0).is_some());
        assert!(arrays_match(&shorter, &longer, 0.9, 10, 0.9, 0).is_none());
    }
}
