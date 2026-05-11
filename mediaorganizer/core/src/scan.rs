//! Scan engine: file discovery → hash extraction → comparison → duplicate detection.
//!
//! Faithful port of VDF.Core/ScanEngine.cs.
//! Three comparison phases run sequentially after hashing:
//!   1. Visual pHash (or grayscale) — ScanForDuplicates
//!   2. Partial clip audio fingerprint — ScanForPartialDuplicates
//!   3. I-frame timeline — ScanForTimelineDuplicates

use crate::{
    audio,
    comparison::arrays_match,
    config::{FolderMatchMode, Settings},
    db::{Database, DuplicatePair, FileRecord, MatchMethod},
    error::VdfResult,
    ffmpeg,
    phash::{compute_phash, is_duplicate as phash_is_duplicate, similarity as phash_similarity},
};
use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;
use rayon::prelude::*;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
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

/// Supported video extensions (matches VDF.Core/FFTools/FileUtils.cs).
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg",
    "ts", "m2ts", "mts", "vob", "3gp", "ogv", "rm", "rmvb", "divx", "xvid",
    "asf", "f4v", "hevc", "264", "265",
];
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "bmp", "gif", "webp", "tiff", "tif"];

/// Threshold for activating duration-bucket optimization (matching C# bucketActivationThreshold).
const BUCKET_ACTIVATION_THRESHOLD: usize = 5000;

/// Bucket granularity in seconds (matching C# bucketSizeSeconds = 1).
const BUCKET_SIZE_SECS: i64 = 1;

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

    /// Full scan: discover → hash → compare (three phases).
    pub fn run(&mut self) -> VdfResult<()> {
        let paths = self.discover_files();
        info!("discovered {} files", paths.len());

        self.hash_files(&paths)?;

        self.scan_for_duplicates()?;

        if self.settings.partial_clip_detection {
            self.scan_for_partial_duplicates()?;
        }

        if self.settings.iframe_fingerprint {
            self.scan_for_timeline_duplicates()?;
        }

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
                .standard_filters(false) // Don't skip hidden files — VDF includes them
                .follow_links(false)
                .build();

            for entry in walker.flatten() {
                let ft = match entry.file_type() {
                    Some(ft) => ft,
                    None => continue,
                };
                if !ft.is_file() {
                    continue;
                }

                let p = entry.path();
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                let is_video = VIDEO_EXTENSIONS.contains(&ext.as_str());
                let is_image =
                    self.settings.include_images && IMAGE_EXTENSIONS.contains(&ext.as_str());
                if !is_video && !is_image {
                    continue;
                }

                // Blacklist / exclude_dirs check
                let excluded =
                    self.settings.exclude_dirs.iter().any(|ex| p.starts_with(ex.as_std_path()));
                if excluded {
                    continue;
                }

                // File size filter
                if self.settings.filter_by_file_size {
                    if let Ok(meta) = std::fs::metadata(p) {
                        let size = meta.len();
                        if self.settings.min_file_size_bytes > 0
                            && size < self.settings.min_file_size_bytes
                        {
                            continue;
                        }
                        if self.settings.max_file_size_bytes > 0
                            && size > self.settings.max_file_size_bytes
                        {
                            continue;
                        }
                    }
                }

                // Path contains filter
                if self.settings.filter_by_path_contains && !self.settings.path_contains_texts.is_empty()
                {
                    let path_str = p.to_string_lossy();
                    let matches = self
                        .settings
                        .path_contains_texts
                        .iter()
                        .any(|pat| path_str.contains(pat.as_str()));
                    if !matches {
                        continue;
                    }
                }

                // Path not-contains filter
                if self.settings.filter_by_path_not_contains
                    && !self.settings.path_not_contains_texts.is_empty()
                {
                    let path_str = p.to_string_lossy();
                    let excluded_by_pattern = self
                        .settings
                        .path_not_contains_texts
                        .iter()
                        .any(|pat| path_str.contains(pat.as_str()));
                    if excluded_by_pattern {
                        continue;
                    }
                }

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
    // Phase 3a: visual pHash duplicate scan (ScanForDuplicates)
    // ------------------------------------------------------------------

    fn scan_for_duplicates(&mut self) -> VdfResult<()> {
        let all = self.db.all_files()?;

        let (mut images, mut videos): (Vec<_>, Vec<_>) =
            all.into_iter().partition(|f| f.is_image);

        // Filter out files with no hash data
        images.retain(|f| !f.phashes.is_empty());
        videos.retain(|f| !f.phashes.is_empty());

        let total = images.len() + videos.len();
        let total_pairs = total * total.saturating_sub(1) / 2;
        self.emit(ScanProgress::ComparisonStarted { total_pairs });

        self.db.clear_duplicates()?;

        let settings = &self.settings;
        let pairs: Arc<Mutex<Vec<DuplicatePair>>> = Arc::new(Mutex::new(Vec::new()));

        // Group representatives for preventing daisy-chain merges.
        // Maps group_id → index in the unified scan list.
        let group_reps: Arc<Mutex<HashMap<u64, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        // Map path → group_id for existing duplicate entries.
        let path_to_group: Arc<Mutex<HashMap<String, u64>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let next_group_id = AtomicU64::new(1);

        // Merge helper closure (mirrors C# MergeDuplicate)
        let try_merge = |rec_a: &FileRecord,
                         rec_b: &FileRecord,
                         sim: f32,
                         method: MatchMethod,
                         offset: Option<f64>,
                         all_recs: &[FileRecord],
                         pairs_lock: &Arc<Mutex<Vec<DuplicatePair>>>,
                         p2g: &Arc<Mutex<HashMap<String, u64>>>,
                         greps: &Arc<Mutex<HashMap<u64, usize>>>,
                         gid_counter: &AtomicU64,
                         settings: &Settings| {
            let mut p2g_map = p2g.lock().unwrap();
            let mut greps_map = greps.lock().unwrap();
            let path_a = rec_a.path.to_string();
            let path_b = rec_b.path.to_string();

            let found_a = p2g_map.get(&path_a).copied();
            let found_b = p2g_map.get(&path_b).copied();

            match (found_a, found_b) {
                (Some(ga), Some(gb)) => {
                    if ga != gb {
                        // Merging two groups: verify representatives are similar
                        let rep_a_idx = greps_map.get(&ga).copied();
                        let rep_b_idx = greps_map.get(&gb).copied();
                        if let (Some(ia), Some(ib)) = (rep_a_idx, rep_b_idx) {
                            if ia < all_recs.len() && ib < all_recs.len() {
                                let ra = &all_recs[ia];
                                let rb = &all_recs[ib];
                                if !phash_check_duplicate(ra, rb, settings) {
                                    return; // Representatives not similar — block merge
                                }
                            }
                        }
                        // Reassign all gb members to ga
                        for gid in p2g_map.values_mut() {
                            if *gid == gb {
                                *gid = ga;
                            }
                        }
                        greps_map.remove(&gb);
                    }
                }
                (Some(ga), None) => {
                    // Verify against existing group representative
                    if let Some(&rep_idx) = greps_map.get(&ga) {
                        if rep_idx < all_recs.len()
                            && !phash_check_duplicate(&all_recs[rep_idx], rec_b, settings)
                        {
                            return;
                        }
                    }
                    p2g_map.insert(path_b.clone(), ga);
                }
                (None, Some(gb)) => {
                    // Verify against existing group representative
                    if let Some(&rep_idx) = greps_map.get(&gb) {
                        if rep_idx < all_recs.len()
                            && !phash_check_duplicate(&all_recs[rep_idx], rec_a, settings)
                        {
                            return;
                        }
                    }
                    p2g_map.insert(path_a.clone(), gb);
                }
                (None, None) => {
                    let gid = gid_counter.fetch_add(1, Ordering::Relaxed);
                    p2g_map.insert(path_a.clone(), gid);
                    p2g_map.insert(path_b.clone(), gid);
                    // Representative is always rec_a (first of the pair, lower scan index)
                    let rep_idx = all_recs.iter().position(|r| r.path == rec_a.path).unwrap_or(0);
                    greps_map.insert(gid, rep_idx);
                }
            }

            pairs_lock.lock().unwrap().push(DuplicatePair {
                file_a: rec_a.id.clone(),
                file_b: rec_b.id.clone(),
                similarity: sim,
                method,
                clip_offset_secs: offset,
            });
        };

        // Compare images (always linear, no buckets)
        compare_images(
            &images,
            settings,
            &pairs,
            &path_to_group,
            &group_reps,
            &next_group_id,
            &try_merge,
        );

        // Compare videos (bucketed for large datasets)
        if videos.len() < BUCKET_ACTIVATION_THRESHOLD {
            compare_videos_linear(
                &videos,
                settings,
                &pairs,
                &path_to_group,
                &group_reps,
                &next_group_id,
                &try_merge,
            );
        } else {
            compare_videos_bucketed(
                &videos,
                settings,
                &pairs,
                &path_to_group,
                &group_reps,
                &next_group_id,
                &try_merge,
            );
        }

        let found_pairs = pairs.lock().unwrap().clone();
        for pair in found_pairs {
            self.emit(ScanProgress::DuplicateFound {
                file_a: pair.file_a.clone(),
                file_b: pair.file_b.clone(),
                similarity: pair.similarity,
            });
            self.db.add_duplicate(pair)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Phase 3b: partial clip audio fingerprint scan
    // ------------------------------------------------------------------

    fn scan_for_partial_duplicates(&mut self) -> VdfResult<()> {
        let all = self.db.all_files()?;

        // Eligible: has non-empty, non-silent audio fingerprint
        let mut candidates: Vec<FileRecord> = all
            .into_iter()
            .filter(|f| !f.is_image && !f.audio_fingerprint.is_empty()
                && !is_silent_fingerprint(&f.audio_fingerprint))
            .collect();

        if candidates.len() < 2 {
            return Ok(());
        }

        // Sort longest first (matching C# OrderByDescending duration)
        candidates.sort_by(|a, b| {
            let da = a.media_info.as_ref().map(|m| m.duration_secs).unwrap_or(0.0);
            let db_ = b.media_info.as_ref().map(|m| m.duration_secs).unwrap_or(0.0);
            db_.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });

        let sim_threshold = self.settings.partial_clip_min_similarity;

        let mut new_pairs = Vec::new();

        for i in 0..candidates.len().saturating_sub(1) {
            let source = &candidates[i];
            let source_sec =
                source.media_info.as_ref().map(|m| m.duration_secs).unwrap_or(0.0);
            if source_sec < 1.0 {
                continue;
            }

            for j in (i + 1)..candidates.len() {
                let clip = &candidates[j];
                let clip_sec =
                    clip.media_info.as_ref().map(|m| m.duration_secs).unwrap_or(0.0);
                if clip_sec < 1.0 {
                    continue;
                }

                // Pre-filter: clip must be shorter than source (sorted desc, so j > i is shorter)
                if clip.audio_fingerprint.len() >= source.audio_fingerprint.len() {
                    continue;
                }

                // Pre-filter: clip < 95% of source length (visual dup handles the rest)
                if clip_sec / source_sec >= 0.95 {
                    continue;
                }

                let (sim, offset_secs) = audio::fingerprint_sliding_window(
                    &clip.audio_fingerprint,
                    &source.audio_fingerprint,
                    sim_threshold,
                );

                if sim >= sim_threshold {
                    new_pairs.push(DuplicatePair {
                        file_a: source.id.clone(),
                        file_b: clip.id.clone(),
                        similarity: sim,
                        method: MatchMethod::AudioFingerprint,
                        clip_offset_secs: Some(offset_secs as f64),
                    });
                }
            }
        }

        for pair in new_pairs {
            self.emit(ScanProgress::DuplicateFound {
                file_a: pair.file_a.clone(),
                file_b: pair.file_b.clone(),
                similarity: pair.similarity,
            });
            self.db.add_duplicate(pair)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Phase 3c: I-frame timeline scan
    // ------------------------------------------------------------------

    fn scan_for_timeline_duplicates(&mut self) -> VdfResult<()> {
        let all = self.db.all_files()?;
        let candidates: Vec<FileRecord> = all
            .into_iter()
            .filter(|f| !f.is_image && f.iframe_phashes.len() >= 2)
            .collect();

        if candidates.len() < 2 {
            return Ok(());
        }

        info!("I-frame timeline scan: {} videos with fingerprints", candidates.len());

        let settings = &self.settings;
        let mut new_pairs = Vec::new();

        for i in 0..candidates.len().saturating_sub(1) {
            let a = &candidates[i];
            for j in (i + 1)..candidates.len() {
                let b = &candidates[j];

                let (shorter, longer, short_rec, long_rec) =
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
                );

                if let Some(r) = result {
                    // Convert offset index to seconds using the longer video's timestamps
                    let offset_secs = if !long_rec.iframe_timestamps.is_empty() {
                        let idx = r.offset.min(long_rec.iframe_timestamps.len() - 1);
                        long_rec.iframe_timestamps[idx]
                    } else {
                        r.offset as f64 * settings.iframe_sample_interval_secs
                    };

                    info!(
                        "[Timeline] {} in {}: sim={:.1}%, offset={:.1}s",
                        short_rec.path.file_name().unwrap_or("?"),
                        long_rec.path.file_name().unwrap_or("?"),
                        r.similarity * 100.0,
                        offset_secs,
                    );

                    new_pairs.push(DuplicatePair {
                        file_a: long_rec.id.clone(),
                        file_b: short_rec.id.clone(),
                        similarity: r.similarity,
                        method: MatchMethod::IframeTimeline,
                        clip_offset_secs: Some(offset_secs),
                    });
                }
            }
        }

        for pair in new_pairs {
            self.emit(ScanProgress::DuplicateFound {
                file_a: pair.file_a.clone(),
                file_b: pair.file_b.clone(),
                similarity: pair.similarity,
            });
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

    let is_image = {
        let ext = path.extension().unwrap_or("").to_lowercase();
        IMAGE_EXTENSIONS.contains(&ext.as_str())
    };
    record.is_image = is_image;

    if is_image {
        // Load image, resize to 32×32, extract grayscale bytes
        if let Ok(gray) = load_image_as_gray32(path) {
            let h = compute_phash(&gray);
            record.phashes.insert(0, h);
        } else {
            return Err(crate::error::VdfError::FfmpegGeneral {
                code: -1,
                msg: format!("failed to load image: {path}"),
            });
        }
        return Ok(record);
    }

    // Probe media info
    let info = ffmpeg::probe_media(path)?;
    let duration = info.duration_secs;
    record.media_info = Some(info);

    // Standard pHash samples using the configured thumbnail count
    let timestamps = settings.sample_timestamps(duration, settings.thumbnail_count);
    let frames = ffmpeg::extract_gray_frames(
        path,
        &timestamps,
        settings.effective_skip_start(duration),
        settings.effective_skip_end(duration),
    )?;
    for (ts_ms, gray) in frames {
        record.phashes.insert(ts_ms, compute_phash(&gray));
    }

    // I-frame timeline fingerprint
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
        match audio::compute_fingerprint(path) {
            Ok(Some(fp)) => {
                if is_silent_fingerprint(&fp) {
                    // Silent tracks produce all-zero fingerprints that match any other
                    // silent track at 100% — store empty to skip in comparison.
                    warn!("silent audio fingerprint detected, skipping: {path}");
                } else {
                    record.audio_fingerprint = fp;
                }
            }
            Ok(None) => {} // no audio stream
            Err(e) => warn!("audio fingerprint failed for {path}: {e}"),
        }
    }

    Ok(record)
}

/// Load any image file as a 32×32 grayscale array using the `image` crate.
fn load_image_as_gray32(path: &Utf8Path) -> VdfResult<Box<[u8; 1024]>> {
    use fast_image_resize::images::Image;
    use fast_image_resize::{PixelType, ResizeAlg, ResizeOptions, Resizer};

    let img = image::open(path.as_std_path())
        .map_err(|e| crate::error::VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let rgb_flat = rgb.into_raw();

    let src = Image::from_vec_u8(w, h, rgb_flat, PixelType::U8x3)
        .map_err(|e| crate::error::VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;
    let mut dst = Image::new(32, 32, PixelType::U8x3);
    let mut resizer = Resizer::new();
    resizer
        .resize(&src, &mut dst, &ResizeOptions::new().resize_alg(ResizeAlg::Nearest))
        .map_err(|e| crate::error::VdfError::FfmpegGeneral { code: -1, msg: e.to_string() })?;

    let buf = dst.buffer();
    let mut gray = Box::new([0u8; 1024]);
    for i in 0..1024usize {
        let r = buf[i * 3] as u32;
        let g = buf[i * 3 + 1] as u32;
        let b = buf[i * 3 + 2] as u32;
        gray[i] = ((r * 299 + g * 587 + b * 114) / 1000) as u8;
    }
    Ok(gray)
}

// ---------------------------------------------------------------------------
// Silent fingerprint detection (port of ScanEngine.IsSilentFingerprint)
// ---------------------------------------------------------------------------

/// Returns true when every block in the fingerprint is zero.
/// Silent tracks produce uniform-zero fingerprints that match any other silent
/// track at 100%, causing false-positive partial-clip groups.
pub fn is_silent_fingerprint(fp: &[u32]) -> bool {
    if fp.is_empty() {
        return false;
    }
    fp.iter().all(|&b| b == 0)
}

// ---------------------------------------------------------------------------
// Folder match mode helper (port of ScanEngine.SameFolderAtDepth)
// ---------------------------------------------------------------------------

/// Returns true when the last `depth` path segments of both folder paths are equal.
fn same_folder_at_depth(a: &str, b: &str, depth: usize) -> bool {
    fn segments(s: &str) -> Vec<&str> {
        s.split(std::path::MAIN_SEPARATOR).filter(|seg| !seg.is_empty()).collect()
    }
    let sa = segments(a);
    let sb = segments(b);
    for i in 0..depth {
        let ai = sa.len().checked_sub(i + 1);
        let bi = sb.len().checked_sub(i + 1);
        match (ai, bi) {
            (Some(ia), Some(ib)) => {
                if !sa[ia].eq_ignore_ascii_case(sb[ib]) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// Check whether two file records should be compared according to the folder match mode.
fn folder_filter_passes(a: &FileRecord, b: &FileRecord, settings: &Settings) -> bool {
    match settings.folder_match_mode {
        FolderMatchMode::None => true,
        FolderMatchMode::SameFolderOnly => {
            let fa = a.path.parent().map(|p| p.as_str()).unwrap_or("");
            let fb = b.path.parent().map(|p| p.as_str()).unwrap_or("");
            same_folder_at_depth(fa, fb, settings.same_folder_depth)
        }
        FolderMatchMode::DifferentFolderOnly => {
            let fa = a.path.parent().map(|p| p.as_str()).unwrap_or("");
            let fb = b.path.parent().map(|p| p.as_str()).unwrap_or("");
            !same_folder_at_depth(fa, fb, settings.same_folder_depth)
        }
    }
}

// ---------------------------------------------------------------------------
// pHash comparison (port of CheckIfDuplicate + IsDuplicateByPercent)
// ---------------------------------------------------------------------------

/// Check duplicate using pHash at the first thumbnail position only.
/// Mirrors C# CheckIfDuplicate with UsePHashing=true: only positionList[0] is checked.
fn phash_check_duplicate(a: &FileRecord, b: &FileRecord, settings: &Settings) -> bool {
    let ha = a.phashes.values().next().copied();
    let hb = b.phashes.values().next().copied();
    match (ha, hb) {
        (Some(ha), Some(hb)) => phash_is_duplicate(ha, hb, settings.min_similarity),
        _ => false,
    }
}

/// Compute similarity between the first pHashes of two records (for DuplicatePair.similarity).
fn phash_first_similarity(a: &FileRecord, b: &FileRecord) -> f32 {
    let ha = a.phashes.values().next().copied().unwrap_or(0);
    let hb = b.phashes.values().next().copied().unwrap_or(0);
    phash_similarity(ha, hb)
}

/// Duration tolerance check matching C# GetDurationToleranceSeconds.
fn duration_filter_passes(a: &FileRecord, b: &FileRecord, settings: &Settings) -> bool {
    let (Some(ia), Some(ib)) = (&a.media_info, &b.media_info) else {
        return true; // no duration info — don't filter
    };
    let longer = ia.duration_secs.max(ib.duration_secs);
    let diff = (ia.duration_secs - ib.duration_secs).abs();
    let tolerance = settings.duration_tolerance_secs(longer);
    tolerance <= 0.0 || diff <= tolerance
}

// ---------------------------------------------------------------------------
// Merge type alias for the closure
// ---------------------------------------------------------------------------

type MergeFn<'a> = dyn Fn(
        &FileRecord,
        &FileRecord,
        f32,
        MatchMethod,
        Option<f64>,
        &[FileRecord],
        &Arc<Mutex<Vec<DuplicatePair>>>,
        &Arc<Mutex<HashMap<String, u64>>>,
        &Arc<Mutex<HashMap<u64, usize>>>,
        &AtomicU64,
        &Settings,
    ) + 'a;

// ---------------------------------------------------------------------------
// Image comparison (linear, no buckets)
// ---------------------------------------------------------------------------

fn compare_images(
    images: &[FileRecord],
    settings: &Settings,
    pairs: &Arc<Mutex<Vec<DuplicatePair>>>,
    p2g: &Arc<Mutex<HashMap<String, u64>>>,
    greps: &Arc<Mutex<HashMap<u64, usize>>>,
    gid: &AtomicU64,
    merge: &MergeFn<'_>,
) {
    for i in 0..images.len().saturating_sub(1) {
        let a = &images[i];
        // Pre-compute flipped hash for image a if enabled
        let flipped_a: Option<u64> = if settings.compare_horizontally_flipped {
            a.phashes.values().next().map(|&h| {
                // For images: flip the raw 32×32 gray and recompute (we don't have raw bytes here,
                // so we use a placeholder — proper flip needs the gray bytes stored in DB)
                // TODO: store raw gray bytes in FileRecord for flip support
                h // placeholder: same hash (no flip effect without raw bytes)
            })
        } else {
            None
        };

        for j in (i + 1)..images.len() {
            let b = &images[j];

            if !folder_filter_passes(a, b, settings) {
                continue;
            }

            let is_dup = phash_check_duplicate(a, b, settings);
            let _ = flipped_a; // flip not yet implemented without raw gray bytes
            if is_dup {
                let sim = phash_first_similarity(a, b);
                merge(a, b, sim, MatchMethod::FrameSimilarity, None, images, pairs, p2g, greps, gid, settings);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Video comparison — linear path (< BUCKET_ACTIVATION_THRESHOLD files)
// ---------------------------------------------------------------------------

fn compare_videos_linear(
    videos: &[FileRecord],
    settings: &Settings,
    pairs: &Arc<Mutex<Vec<DuplicatePair>>>,
    p2g: &Arc<Mutex<HashMap<String, u64>>>,
    greps: &Arc<Mutex<HashMap<u64, usize>>>,
    gid: &AtomicU64,
    merge: &MergeFn<'_>,
) {
    for i in 0..videos.len().saturating_sub(1) {
        let a = &videos[i];
        for j in (i + 1)..videos.len() {
            let b = &videos[j];

            if !duration_filter_passes(a, b, settings) {
                continue;
            }
            if !folder_filter_passes(a, b, settings) {
                continue;
            }

            let is_dup = phash_check_duplicate(a, b, settings);
            if is_dup {
                let sim = phash_first_similarity(a, b);
                merge(a, b, sim, MatchMethod::FrameSimilarity, None, videos, pairs, p2g, greps, gid, settings);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Video comparison — bucketed path (≥ BUCKET_ACTIVATION_THRESHOLD files)
// ---------------------------------------------------------------------------

fn compare_videos_bucketed(
    videos: &[FileRecord],
    settings: &Settings,
    pairs: &Arc<Mutex<Vec<DuplicatePair>>>,
    p2g: &Arc<Mutex<HashMap<String, u64>>>,
    greps: &Arc<Mutex<HashMap<u64, usize>>>,
    gid: &AtomicU64,
    merge: &MergeFn<'_>,
) {
    // Build duration buckets
    let mut buckets: HashMap<i64, Vec<usize>> = HashMap::new();
    for (idx, video) in videos.iter().enumerate() {
        let dur = video.media_info.as_ref().map(|m| m.duration_secs).unwrap_or(0.0);
        let key = (dur / BUCKET_SIZE_SECS as f64).floor() as i64;
        buckets.entry(key).or_default().push(idx);
    }

    for (&bucket_key, bucket_indices) in &buckets {
        for &i_pos in bucket_indices {
            let a = &videos[i_pos];
            let dur_a =
                a.media_info.as_ref().map(|m| m.duration_secs).unwrap_or(0.0);
            let tolerance = settings.duration_tolerance_secs(dur_a);

            let min_key =
                ((dur_a - tolerance).max(0.0) / BUCKET_SIZE_SECS as f64).floor() as i64;
            let max_key = ((dur_a + tolerance) / BUCKET_SIZE_SECS as f64).floor() as i64;

            for cand_key in min_key..=max_key {
                if cand_key < bucket_key {
                    continue; // avoid symmetric duplicates
                }
                let Some(cand_indices) = buckets.get(&cand_key) else { continue };
                for &j_pos in cand_indices {
                    if j_pos <= i_pos {
                        continue; // avoid symmetric duplicates
                    }
                    let b = &videos[j_pos];

                    if !duration_filter_passes(a, b, settings) {
                        continue;
                    }
                    if !folder_filter_passes(a, b, settings) {
                        continue;
                    }

                    let is_dup = phash_check_duplicate(a, b, settings);
                    if is_dup {
                        let sim = phash_first_similarity(a, b);
                        merge(a, b, sim, MatchMethod::FrameSimilarity, None, videos, pairs, p2g, greps, gid, settings);
                    }
                }
            }
        }
    }
}
