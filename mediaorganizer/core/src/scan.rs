//! Scan engine: file discovery → hash extraction → comparison → duplicate detection.

use crate::{
    audio,
    comparison::arrays_match,
    config::Settings,
    db::{Database, DuplicatePair, FileRecord, MatchMethod},
    error::VdfResult,
    ffmpeg,
    phash::{compute_phash, similarity as phash_similarity},
};
use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;
use rayon::prelude::*;
use std::sync::Arc;
use tracing::{info, warn};

/// Progress event emitted during scanning.
#[derive(Debug, Clone)]
pub enum ScanProgress {
    FileDiscovered { path: Utf8PathBuf },
    FileHashed { path: Utf8PathBuf, phash: u64 },
    ComparisonStarted { total_pairs: usize },
    DuplicateFound { file_a: String, file_b: String, similarity: f32 },
    ScanComplete { files: usize, duplicates: usize },
    Error { path: Utf8PathBuf, msg: String },
}

pub type ProgressCallback = Arc<dyn Fn(ScanProgress) + Send + Sync>;

/// Supported video/image extensions.
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg",
    "ts", "m2ts", "mts", "vob", "3gp", "ogv", "rm", "rmvb",
];
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "bmp", "gif", "webp", "tiff", "tif"];

pub struct ScanEngine<D: Database> {
    pub settings: Settings,
    pub db: D,
    progress: Option<ProgressCallback>,
}

impl<D: Database> ScanEngine<D> {
    pub fn new(settings: Settings, db: D) -> Self {
        Self { settings, db, progress: None }
    }

    pub fn with_progress(mut self, cb: ProgressCallback) -> Self {
        self.progress = Some(cb);
        self
    }

    fn emit(&self, event: ScanProgress) {
        if let Some(cb) = &self.progress {
            cb(event);
        }
    }

    /// Discover files, hash them, store in DB, then compare.
    pub fn run(&mut self) -> VdfResult<()> {
        let paths = self.discover_files();
        info!("discovered {} files", paths.len());

        self.hash_files(&paths)?;
        self.compare_all()?;

        let dupes = self.db.all_duplicates()?.len();
        self.db.flush()?;
        self.emit(ScanProgress::ScanComplete { files: paths.len(), duplicates: dupes });
        Ok(())
    }

    // ------------------------------------------------------------------
    // Phase 1: file discovery
    // ------------------------------------------------------------------

    fn discover_files(&self) -> Vec<Utf8PathBuf> {
        let mut paths = Vec::new();
        for dir in &self.settings.include_dirs {
            let walker = WalkBuilder::new(dir.as_std_path())
                .standard_filters(true)
                .build();
            for entry in walker.flatten() {
                if entry.file_type().map(|t| !t.is_file()).unwrap_or(true) {
                    continue;
                }
                let p = entry.path();
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                let is_video = VIDEO_EXTENSIONS.contains(&ext.as_str());
                let is_image = self.settings.include_images && IMAGE_EXTENSIONS.contains(&ext.as_str());
                if !is_video && !is_image {
                    continue;
                }
                let excluded = self.settings.exclude_dirs.iter().any(|ex| {
                    p.starts_with(ex.as_std_path())
                });
                if excluded { continue; }

                match Utf8PathBuf::from_path_buf(p.to_path_buf()) {
                    Ok(utf8) => {
                        self.emit(ScanProgress::FileDiscovered { path: utf8.clone() });
                        paths.push(utf8);
                    }
                    Err(bad) => warn!("skipping non-UTF-8 path: {}", bad.display()),
                }
            }
        }
        paths
    }

    // ------------------------------------------------------------------
    // Phase 2: hashing (parallel via rayon)
    // ------------------------------------------------------------------

    fn hash_files(&mut self, paths: &[Utf8PathBuf]) -> VdfResult<()> {
        let settings = &self.settings;

        // Process in parallel; collect results then write to DB serially
        let results: Vec<(Utf8PathBuf, VdfResult<FileRecord>)> = paths
            .par_iter()
            .map(|path| {
                let record = hash_one_file(path, settings);
                (path.clone(), record)
            })
            .collect();

        for (path, result) in results {
            match result {
                Ok(record) => {
                    let first_hash = record.phashes.values().next().copied();
                    self.db.upsert_file(record)?;
                    if let Some(h) = first_hash {
                        self.emit(ScanProgress::FileHashed { path, phash: h });
                    }
                }
                Err(e) => {
                    self.emit(ScanProgress::Error { path, msg: e.to_string() });
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Phase 3: pairwise comparison
    // ------------------------------------------------------------------

    fn compare_all(&mut self) -> VdfResult<()> {
        let all = self.db.all_files()?;
        let n = all.len();
        let total_pairs = n * (n.saturating_sub(1)) / 2;
        self.emit(ScanProgress::ComparisonStarted { total_pairs });

        self.db.clear_duplicates()?;

        let settings = &self.settings;
        let mut pairs: Vec<DuplicatePair> = Vec::new();

        for i in 0..n {
            for j in (i + 1)..n {
                let a = &all[i];
                let b = &all[j];

                // Duration pre-filter: use percentage-based tolerance matching C#
                if let (Some(ia), Some(ib)) = (&a.media_info, &b.media_info) {
                    let longer_dur = ia.duration_secs.max(ib.duration_secs);
                    let diff = (ia.duration_secs - ib.duration_secs).abs();
                    let tolerance = settings.duration_tolerance_secs(longer_dur);
                    if tolerance > 0.0 && diff > tolerance {
                        continue;
                    }
                }

                // --- Standard pHash comparison ---
                if let Some(pair) = compare_phash(a, b, settings.min_similarity) {
                    self.emit(ScanProgress::DuplicateFound {
                        file_a: a.path.to_string(),
                        file_b: b.path.to_string(),
                        similarity: pair.similarity,
                    });
                    pairs.push(pair);
                    continue; // already matched; skip heavier methods
                }

                // --- I-frame timeline comparison ---
                if settings.iframe_fingerprint
                    && !a.iframe_phashes.is_empty()
                    && !b.iframe_phashes.is_empty()
                {
                    if let Some(pair) = compare_iframe_timeline(a, b, settings) {
                        self.emit(ScanProgress::DuplicateFound {
                            file_a: a.path.to_string(),
                            file_b: b.path.to_string(),
                            similarity: pair.similarity,
                        });
                        pairs.push(pair);
                        continue;
                    }
                }

                // --- Audio fingerprint comparison ---
                if settings.partial_clip_detection
                    && !a.audio_fingerprint.is_empty()
                    && !b.audio_fingerprint.is_empty()
                {
                    if let Some(pair) = compare_audio(a, b, settings) {
                        self.emit(ScanProgress::DuplicateFound {
                            file_a: a.path.to_string(),
                            file_b: b.path.to_string(),
                            similarity: pair.similarity,
                        });
                        pairs.push(pair);
                    }
                }
            }
        }

        for pair in pairs {
            self.db.add_duplicate(pair)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Per-file hashing (called in parallel by rayon)
// ---------------------------------------------------------------------------

fn hash_one_file(path: &Utf8Path, settings: &Settings) -> VdfResult<FileRecord> {
    let size = std::fs::metadata(path)?.len();
    let mut record = FileRecord::new(path.to_owned(), size);

    // Probe media info
    let info = ffmpeg::probe_media(path)?;
    let duration = info.duration_secs;
    record.media_info = Some(info);

    // Standard pHash samples
    let timestamps = settings.sample_timestamps(duration, 5); // 5 sample positions
    let frames = ffmpeg::extract_gray_frames(
        path,
        &timestamps,
        settings.effective_skip_start(duration),
        settings.effective_skip_end(duration),
    )?;
    for (ts_ms, gray) in frames {
        record.phashes.insert(ts_ms, compute_phash(&gray));
    }

    // I-frame timeline
    if settings.iframe_fingerprint {
        let ts_list = ffmpeg::extract_iframe_timestamps(
            path,
            settings.iframe_sample_interval_secs,
            settings.effective_skip_start(duration),
            settings.effective_skip_end(duration),
            settings.max_iframe_samples,
        )?;
        let gray_frames = ffmpeg::extract_gray_frames(
            path,
            &ts_list,
            settings.effective_skip_start(duration),
            settings.effective_skip_end(duration),
        )?;
        record.iframe_timestamps = ts_list;
        record.iframe_phashes = gray_frames.values().map(|g| compute_phash(g)).collect();
    }

    // Audio fingerprint
    if settings.partial_clip_detection {
        if let Ok(Some(fp)) = audio::compute_fingerprint(path) {
            record.audio_fingerprint = fp;
        }
    }

    Ok(record)
}

// ---------------------------------------------------------------------------
// Comparison helpers
// ---------------------------------------------------------------------------

fn compare_phash(a: &FileRecord, b: &FileRecord, min_sim: f32) -> Option<DuplicatePair> {
    if a.phashes.is_empty() || b.phashes.is_empty() {
        return None;
    }
    // Compare all same-index positions and average
    let mut total = 0f32;
    let mut count = 0usize;
    for (&ts, &ha) in &a.phashes {
        if let Some(&hb) = b.phashes.get(&ts) {
            total += phash_similarity(ha, hb);
            count += 1;
        }
    }
    if count == 0 {
        // No matching timestamps: compare by position index
        let av: Vec<u64> = a.phashes.values().copied().collect();
        let bv: Vec<u64> = b.phashes.values().copied().collect();
        let n = av.len().min(bv.len());
        for i in 0..n {
            total += phash_similarity(av[i], bv[i]);
            count += 1;
        }
    }
    if count == 0 { return None; }
    let sim = total / count as f32;
    if sim >= min_sim {
        Some(DuplicatePair {
            file_a: a.id.clone(),
            file_b: b.id.clone(),
            similarity: sim,
            method: MatchMethod::FrameSimilarity,
            clip_offset_secs: None,
        })
    } else {
        None
    }
}

fn compare_iframe_timeline(
    a: &FileRecord,
    b: &FileRecord,
    settings: &Settings,
) -> Option<DuplicatePair> {
    let (shorter, longer, _shorter_rec, _longer_rec) =
        if a.iframe_phashes.len() <= b.iframe_phashes.len() {
            (&a.iframe_phashes, &b.iframe_phashes, a, b)
        } else {
            (&b.iframe_phashes, &a.iframe_phashes, b, a)
        };

    let result = arrays_match(
        shorter,
        longer,
        settings.iframe_match_percent,
        settings.iframe_min_consecutive,
        settings.iframe_hash_threshold,
        settings.iframe_max_gap,
    )?;

    // Compute clip offset in seconds from offset index + interval
    let offset_secs = result.offset as f64 * settings.iframe_sample_interval_secs;

    Some(DuplicatePair {
        file_a: a.id.clone(),
        file_b: b.id.clone(),
        similarity: result.similarity,
        method: MatchMethod::IframeTimeline,
        clip_offset_secs: Some(offset_secs),
    })
}

fn compare_audio(
    a: &FileRecord,
    b: &FileRecord,
    settings: &Settings,
) -> Option<DuplicatePair> {
    let sim = audio::fingerprint_similarity(&a.audio_fingerprint, &b.audio_fingerprint);
    if sim >= settings.partial_clip_min_similarity {
        Some(DuplicatePair {
            file_a: a.id.clone(),
            file_b: b.id.clone(),
            similarity: sim,
            method: MatchMethod::AudioFingerprint,
            clip_offset_secs: None,
        })
    } else {
        None
    }
}
