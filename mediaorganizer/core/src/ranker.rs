//! Quality ranker — picks the "keeper" from a group of duplicate files.
//!
//! Faithful port of VDF.Core/Utils/QualityRanker.cs.
//! Each criterion is "higher value wins".  On a tie the next criterion runs only
//! against the tied subset — never the full list again.  Walking stops as soon
//! as one criterion produces a unique winner; the first remaining item wins when
//! all criteria are exhausted.

use crate::db::FileRecord;

// ─── Criterion ────────────────────────────────────────────────────────────────

/// A single ranked quality signal.
pub struct Criterion {
    pub name: &'static str,
    /// Returns a comparable score for a FileRecord.  Higher = better.
    pub accessor: Box<dyn Fn(&FileRecord) -> f64 + Send + Sync>,
    /// If true, skip this criterion when all remaining candidates are images.
    pub video_only: bool,
}

impl Criterion {
    pub fn new(
        name: &'static str,
        accessor: impl Fn(&FileRecord) -> f64 + Send + Sync + 'static,
        video_only: bool,
    ) -> Self {
        Self { name, accessor: Box::new(accessor), video_only }
    }
}

// ─── pick_keeper ──────────────────────────────────────────────────────────────

/// Returns a reference to the file that should be kept from `items`.
///
/// Mirrors `QualityRanker.PickKeeper<T>` exactly:
/// - Walk criteria in priority order.
/// - On the first criterion, find the best-scoring item across *all* candidates.
/// - On subsequent criteria, first narrow to the subset that tied on the
///   previous criterion, then find the best among those.
/// - Stop as soon as the subset has been reduced to 1; return items[0] if empty.
pub fn pick_keeper<'a>(items: &'a [FileRecord], criteria: &[Criterion]) -> Option<&'a FileRecord> {
    if items.is_empty() {
        return None;
    }

    // Indices into `items` still in the running.
    let mut candidates: Vec<usize> = (0..items.len()).collect();
    let mut keep_idx: usize = 0;
    let mut any_applied = false;
    let mut last_applied: Option<usize> = None; // index into `criteria`

    for (crit_idx, criterion) in criteria.iter().enumerate() {
        // Skip video-only criteria when all remaining candidates are images.
        if criterion.video_only && candidates.iter().all(|&i| items[i].is_image()) {
            continue;
        }

        if any_applied {
            // Narrow to the subset that tied on the previous criterion.
            let last = &criteria[last_applied.unwrap()];
            let keep_val = (last.accessor)(&items[keep_idx]);
            candidates.retain(|&i| {
                let v = (last.accessor)(&items[i]);
                // Float comparison: treat equal within 1e-9 as tied.
                (v - keep_val).abs() < 1e-9
            });
            if candidates.len() <= 1 {
                break;
            }
        }

        // Best scorer on this criterion.
        let best_idx = *candidates
            .iter()
            .max_by(|&&a, &&b| {
                let va = (criterion.accessor)(&items[a]);
                let vb = (criterion.accessor)(&items[b]);
                va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        keep_idx = best_idx;
        any_applied = true;
        last_applied = Some(crit_idx);
    }

    Some(&items[keep_idx])
}

// ─── Default criteria (mirrors C# HighlightBestMatches ordering) ──────────────

/// Build the default ordered criterion list used in `HighlightBestMatches`.
///
/// Priority (descending):
/// 1. Longest duration           (video only)
/// 2. Smallest file size         (inverted: negative size → higher = smaller)
/// 3. Highest frame rate         (video only)
/// 4. Highest video bit-rate     (video only)
/// 5. Highest audio sample rate  (video only)
/// 6. Highest audio bit-rate     (video only)
/// 7. Best HDR format rank       (video only)
/// 8. Largest frame area (w×h)   (all)
pub fn default_criteria() -> Vec<Criterion> {
    vec![
        Criterion::new("duration",          |r| r.duration_secs(),                                        true),
        Criterion::new("size_smallest",     |r| -(r.size_bytes as f64),                                   false),
        Criterion::new("fps",               |r| r.frame_rate().unwrap_or(0.0) as f64,                     true),
        Criterion::new("video_bitrate",     |r| r.video_bitrate_kbps().unwrap_or(0) as f64,               true),
        Criterion::new("audio_sample_rate", |r| r.audio_sample_rate().unwrap_or(0) as f64,                true),
        Criterion::new("audio_bitrate",     |r| r.audio_bitrate_kbps().unwrap_or(0) as f64,               true),
        Criterion::new("hdr_rank",          |r| r.hdr_format_rank() as f64,                               true),
        Criterion::new("frame_area",        |r| {
            let w = r.width().unwrap_or(0) as f64;
            let h = r.height().unwrap_or(0) as f64;
            w * h
        }, false),
    ]
}

// ─── Group-level "highlight" flags ────────────────────────────────────────────

/// Quality flags set on a duplicate after `highlight_best_matches`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BestFlags {
    pub is_best_duration:         bool,
    pub is_best_size:             bool,
    pub is_best_fps:              bool,
    pub is_best_bitrate:          bool,
    pub is_best_audio_sample_rate: bool,
    pub is_best_audio_bitrate:    bool,
    pub is_best_hdr_format:       bool,
    pub is_best_frame_size:       bool,
}

/// Compute per-group best flags for a slice of FileRecords that all belong to
/// the same duplicate cluster.
///
/// Mirrors `ScanEngine.HighlightBestMatches()` exactly: for each quality axis,
/// marks *all* items that share the winning value (ties all flagged).
pub fn compute_best_flags(group: &[FileRecord]) -> Vec<BestFlags> {
    let n = group.len();
    let mut flags = vec![BestFlags::default(); n];
    if n == 0 {
        return flags;
    }

    let all_images = group.iter().all(|r| r.is_image());

    // Helper: mark every item matching the best value for a given accessor.
    let mark = |flags: &mut Vec<BestFlags>,
                accessor: &dyn Fn(&FileRecord) -> f64,
                setter: &mut dyn FnMut(&mut BestFlags)| {
        let best = group
            .iter()
            .map(|r| accessor(r))
            .fold(f64::NEG_INFINITY, f64::max);
        for (i, r) in group.iter().enumerate() {
            if (accessor(r) - best).abs() < 1e-9 {
                setter(&mut flags[i]);
            }
        }
    };

    // Duration (video only, highest wins)
    if !all_images {
        mark(&mut flags, &|r| r.duration_secs(), &mut |f| f.is_best_duration = true);
    }

    // Size (smallest wins → negate)
    mark(&mut flags, &|r| -(r.size_bytes as f64), &mut |f| f.is_best_size = true);

    // FPS (video only)
    if !all_images {
        mark(&mut flags, &|r| r.frame_rate().unwrap_or(0.0) as f64, &mut |f| f.is_best_fps = true);
    }

    // Video bit-rate (video only)
    if !all_images {
        mark(&mut flags, &|r| r.video_bitrate_kbps().unwrap_or(0) as f64, &mut |f| f.is_best_bitrate = true);
    }

    // Audio sample rate (video only)
    if !all_images {
        mark(&mut flags, &|r| r.audio_sample_rate().unwrap_or(0) as f64, &mut |f| f.is_best_audio_sample_rate = true);
    }

    // Audio bit-rate (video only)
    if !all_images {
        mark(&mut flags, &|r| r.audio_bitrate_kbps().unwrap_or(0) as f64, &mut |f| f.is_best_audio_bitrate = true);
    }

    // HDR format rank (video only)
    if !all_images {
        mark(&mut flags, &|r| r.hdr_format_rank() as f64, &mut |f| f.is_best_hdr_format = true);
    }

    // Frame area (all)
    mark(
        &mut flags,
        &|r| {
            let w = r.width().unwrap_or(0) as f64;
            let h = r.height().unwrap_or(0) as f64;
            w * h
        },
        &mut |f| f.is_best_frame_size = true,
    );

    flags
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    fn make_record(id: &str, size: u64, dur: f64) -> FileRecord {
        let path = Utf8PathBuf::from(format!("/fake/{id}.mp4"));
        let mut r = FileRecord::new(path, size);
        r.id = id.to_string();
        // Populate a minimal ContainerInfo so duration_secs() works.
        r.container = Some(crate::db::ContainerInfo {
            duration_secs: Some(dur),
            width: Some(1920),
            height: Some(1080),
            ..Default::default()
        });
        r
    }

    #[test]
    fn pick_keeper_longest_duration() {
        let items = vec![
            make_record("a", 1000, 60.0),
            make_record("b", 2000, 120.0),
            make_record("c", 500,  90.0),
        ];
        let criteria = default_criteria();
        let winner = pick_keeper(&items, &criteria).unwrap();
        assert_eq!(winner.id, "b");
    }

    #[test]
    fn pick_keeper_tiebreak_by_size() {
        // Two items with same duration; smaller size wins.
        let items = vec![
            make_record("big",   2000, 60.0),
            make_record("small", 1000, 60.0),
        ];
        let criteria = default_criteria();
        let winner = pick_keeper(&items, &criteria).unwrap();
        assert_eq!(winner.id, "small");
    }

    #[test]
    fn compute_best_flags_marks_longest() {
        let items = vec![
            make_record("a", 1000, 60.0),
            make_record("b", 2000, 120.0),
        ];
        let flags = compute_best_flags(&items);
        assert!(!flags[0].is_best_duration);
        assert!(flags[1].is_best_duration);
    }
}
