//! SurrealDB 3.0 graph database backend.
//!
//! Schema: namespace `vdf`, database `scanner`.  Storage: `kv-rocksdb`.
//!
//! Graph:
//!   file → in_folder      → location
//!   file → duplicate_of   → file   (full evidence edge)
//!   file → blacklisted    → file
//!   file → tagged_with    → user_tag
//!   file → scanned_in     → scan_job
//!   file → stem_of        → file

use crate::error::{VdfError, VdfResult};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use surrealdb::engine::local::{Db, RocksDb};
use surrealdb::Surreal;
use tracing::{debug, info, warn};

// ── Constants ─────────────────────────────────────────────────────────────────

const DB_SCHEMA_VERSION: u32 = 2;
const NS: &str = "vdf";
const DB_NAME: &str = "scanner";

// ═════════════════════════════════════════════════════════════════════════════
// ENUMS
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Video,
    Audio,
    Image,
    Document,
    #[default]
    Unknown,
}

impl MediaType {
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg"
            | "mpeg" | "ts" | "m2ts" | "mts" | "vob" | "3gp" | "ogv" | "rm" | "rmvb"
            | "divx" | "xvid" | "asf" | "f4v" | "hevc" | "264" | "265" => Self::Video,
            "mp3" | "flac" | "aac" | "ogg" | "opus" | "wav" | "aiff" | "m4a" | "wma"
            | "ape" | "alac" | "dts" | "ac3" | "mka" | "ra" => Self::Audio,
            "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" | "tiff" | "tif" | "heic"
            | "heif" | "avif" | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "orf" => {
                Self::Image
            }
            _ => Self::Unknown,
        }
    }
    pub fn is_video(self) -> bool { matches!(self, Self::Video) }
    pub fn is_audio(self) -> bool { matches!(self, Self::Audio) }
    pub fn is_image(self) -> bool { matches!(self, Self::Image) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMethod {
    FrameSimilarity,
    IframeTimeline,
    AudioFingerprint,
    Mpeg7Signature,
    SsimVerified,
    TemporalAverageHash,
}

// ═════════════════════════════════════════════════════════════════════════════
// CONTAINER + STREAM INFO
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub format: Option<String>,
    pub format_long: Option<String>,
    pub duration_secs: Option<f64>,
    pub start_time_secs: Option<f64>,
    pub overall_bitrate_kbps: Option<i64>,
    pub nb_streams: Option<u32>,
    pub probe_score: Option<i32>,
    pub chapters: Vec<ChapterInfo>,
    /// Width promoted from primary video stream for fast queries.
    pub width: Option<u32>,
    /// Height promoted from primary video stream for fast queries.
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterInfo {
    pub index: u32,
    pub title: Option<String>,
    pub start_secs: f64,
    pub end_secs: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VideoStreamInfo {
    pub index: u32,
    pub codec_name: String,
    pub codec_long_name: Option<String>,
    pub codec_tag: Option<String>,
    pub codec_profile: Option<String>,
    pub codec_level: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub coded_width: Option<u32>,
    pub coded_height: Option<u32>,
    pub display_aspect_ratio: Option<String>,
    pub sample_aspect_ratio: Option<String>,
    pub fps: Option<f32>,
    pub avg_fps: Option<f32>,
    pub pixel_format: Option<String>,
    pub bit_depth: Option<u32>,
    pub bitrate_kbps: Option<i64>,
    pub nb_frames: Option<u64>,
    pub duration_secs: Option<f64>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub color_space: Option<String>,
    pub color_range: Option<String>,
    pub color_primaries: Option<String>,
    pub color_transfer: Option<String>,
    pub hdr_format: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    pub index: u32,
    pub codec_name: String,
    pub codec_long_name: Option<String>,
    pub codec_profile: Option<String>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
    pub bits_per_sample: Option<u32>,
    pub bitrate_kbps: Option<i64>,
    pub duration_secs: Option<f64>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubtitleStreamInfo {
    pub index: u32,
    pub codec_name: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
}

// ═════════════════════════════════════════════════════════════════════════════
// FINGERPRINT PHASES
// ═════════════════════════════════════════════════════════════════════════════

/// One visual pHash sample taken at a specific timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhashSample {
    /// Presentation timestamp in seconds.
    pub ts: f64,
    /// 64-bit pHash stored as i64 (SurrealDB int is signed).
    pub hash: i64,
    /// Mean luminance of the 32×32 frame (0.0–1.0).
    pub brightness: f32,
    /// True if this sample was skipped in comparisons.
    pub skipped: bool,
    pub skip_reason: Option<String>,
}

/// Phase 1: standard perceptual hash fingerprint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PhashFingerprint {
    pub computed_at: Option<u64>,
    pub window_start_secs: Option<f64>,
    pub window_end_secs: Option<f64>,
    pub window_duration_secs: Option<f64>,
    pub samples: Vec<PhashSample>,
    pub sample_count: u32,
    pub usable_sample_count: u32,
    pub mean_brightness: Option<f32>,
    pub dark_frame_ratio: Option<f32>,
    /// Flat hash vector (usable samples only) for MTREE vector index.
    pub hash_vector: Vec<i64>,
}

impl PhashFingerprint {
    pub fn from_samples(samples: Vec<PhashSample>) -> Self {
        let total = samples.len() as u32;
        let usable: Vec<&PhashSample> = samples.iter().filter(|s| !s.skipped).collect();
        let usable_count = usable.len() as u32;
        let dark = samples
            .iter()
            .filter(|s| s.skipped && s.skip_reason.as_deref() == Some("too_dark"))
            .count() as u32;
        let mean_brightness = if usable.is_empty() {
            None
        } else {
            Some(usable.iter().map(|s| s.brightness).sum::<f32>() / usable.len() as f32)
        };
        let window_start = samples.first().map(|s| s.ts);
        let window_end = samples.last().map(|s| s.ts);
        let window_dur = match (window_start, window_end) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        };
        let hash_vector: Vec<i64> = usable.iter().map(|s| s.hash).collect();
        Self {
            computed_at: Some(unix_now()),
            window_start_secs: window_start,
            window_end_secs: window_end,
            window_duration_secs: window_dur,
            sample_count: total,
            usable_sample_count: usable_count,
            mean_brightness,
            dark_frame_ratio: if total > 0 {
                Some(dark as f32 / total as f32)
            } else {
                None
            },
            hash_vector,
            samples,
        }
    }

    pub fn usable_hashes(&self) -> Vec<u64> {
        self.hash_vector.iter().map(|&h| h as u64).collect()
    }
}

/// One I-frame sample with encode-quality metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IframeSample {
    pub ts: f64,
    pub hash: i64,
    pub brightness: Option<f32>,
    pub skipped: bool,
    pub skip_reason: Option<String>,
    pub f_size_bytes: Option<u64>,
    pub q: Option<f32>,
    pub psnr: Option<f32>,
    pub br_kbps: Option<f32>,
    pub avg_br_kbps: Option<f32>,
}

/// Phase 2: I-frame timeline fingerprint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IframeFingerprint {
    pub computed_at: Option<u64>,
    pub all_keyframe_timestamps: Vec<f32>,
    pub total_keyframes: u32,
    pub keyframe_density_per_min: Option<f32>,
    pub avg_gop_secs: Option<f32>,
    pub min_gop_secs: Option<f32>,
    pub max_gop_secs: Option<f32>,
    pub samples: Vec<IframeSample>,
    pub sample_count: u32,
    pub sample_interval_secs: Option<f64>,
    pub avg_q: Option<f32>,
    pub max_q: Option<f32>,
}

impl IframeFingerprint {
    pub fn usable_hashes(&self) -> Vec<u64> {
        self.samples
            .iter()
            .filter(|s| !s.skipped)
            .map(|s| s.hash as u64)
            .collect()
    }

    pub fn usable_timestamps(&self) -> Vec<f64> {
        self.samples.iter().filter(|s| !s.skipped).map(|s| s.ts).collect()
    }

    /// Build GOP metrics from the all_keyframe_timestamps list.
    pub fn compute_gop_stats(&mut self, duration_secs: f64) {
        let kf = &self.all_keyframe_timestamps;
        if kf.len() < 2 {
            return;
        }
        let gaps: Vec<f32> = kf.windows(2).map(|w| w[1] - w[0]).collect();
        self.avg_gop_secs = Some(gaps.iter().sum::<f32>() / gaps.len() as f32);
        self.min_gop_secs = gaps.iter().cloned().reduce(f32::min);
        self.max_gop_secs = gaps.iter().cloned().reduce(f32::max);
        if duration_secs > 0.0 {
            self.keyframe_density_per_min =
                Some(self.total_keyframes as f32 / (duration_secs as f32 / 60.0));
        }
    }
}

/// Phase 4: scene-change detection event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneEvent {
    pub ts: f64,
    pub score: f32,
    pub is_significant: bool,
}

/// Phase 4: scene fingerprint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneFingerprint {
    pub computed_at: Option<u64>,
    pub events: Vec<SceneEvent>,
    pub event_count: u32,
    pub significant_event_count: u32,
    pub effective_skip_start_secs: Option<f64>,
    pub intro_cut_score: Option<f32>,
}

/// Phase 3: temporal average hash (tblend pre-filter).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TemporalAvgFingerprint {
    pub computed_at: Option<u64>,
    /// 64-bit pHash of the blended frame, stored as i64.
    pub hash: Option<i64>,
    pub window_start_secs: Option<f64>,
    pub window_end_secs: Option<f64>,
    pub mean_brightness: Option<f32>,
}

/// Phase 5: MPEG-7 coarse video signature.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mpeg7Fingerprint {
    pub computed_at: Option<u64>,
    pub sig_path: Option<String>,
    pub sig_size_bytes: Option<u64>,
    pub sig_sha256: Option<String>,
    pub windows_analyzed: Option<u32>,
    pub duration_covered_secs: Option<f64>,
    pub frames_processed: Option<u32>,
}

/// Phase 6: Chromaprint audio fingerprint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChromaprintFingerprint {
    pub computed_at: Option<u64>,
    pub audio_stream_index: Option<u32>,
    pub input_sample_rate: Option<u32>,
    pub input_channels: Option<u32>,
    pub duration_analyzed_secs: Option<f64>,
    pub fingerprint_length: Option<u32>,
    /// One u32 per second, stored as i64 for SurrealDB (int is signed).
    pub fingerprint: Vec<i64>,
    pub silence_ratio: Option<f32>,
    pub is_reliable: bool,
    pub error: Option<String>,
}

impl ChromaprintFingerprint {
    pub fn from_u32_vec(
        fp: Vec<u32>,
        stream_idx: Option<u32>,
        duration: Option<f64>,
    ) -> Self {
        let len = fp.len() as u32;
        let silence = fp.iter().filter(|&&v| v == 0).count() as u32;
        let silence_ratio = if len > 0 { Some(silence as f32 / len as f32) } else { None };
        let is_reliable = silence_ratio.map(|r| r < 0.5).unwrap_or(false)
            && duration.map(|d| d >= 10.0).unwrap_or(false);
        Self {
            computed_at: Some(unix_now()),
            audio_stream_index: stream_idx,
            input_sample_rate: Some(11025),
            input_channels: Some(1),
            duration_analyzed_secs: duration,
            fingerprint_length: Some(len),
            fingerprint: fp.into_iter().map(|v| v as i64).collect(),
            silence_ratio,
            is_reliable,
            error: None,
        }
    }

    pub fn as_u32_vec(&self) -> Vec<u32> {
        self.fingerprint.iter().map(|&v| v as u32).collect()
    }
}

/// All fingerprint phases collected together.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Fingerprints {
    /// SHA-256 of the Settings struct at time of compute — stale when mismatched.
    pub settings_hash: Option<String>,
    /// True only when every enabled phase ran successfully.
    pub all_phases_complete: bool,
    pub phash: Option<PhashFingerprint>,
    pub iframe: Option<IframeFingerprint>,
    pub scene: Option<SceneFingerprint>,
    pub temporal_avg: Option<TemporalAvgFingerprint>,
    pub mpeg7: Option<Mpeg7Fingerprint>,
    pub chromaprint: Option<ChromaprintFingerprint>,
}

// ═════════════════════════════════════════════════════════════════════════════
// ANALYSIS LAYER
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlackSegment {
    pub start_secs: f64,
    pub end_secs: f64,
    pub duration_secs: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SilenceSegment {
    pub start_secs: f64,
    pub end_secs: f64,
    pub duration_secs: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Analysis {
    // Visual — aggregate
    pub mean_brightness: Option<f32>,
    pub brightness_variance: Option<f32>,
    pub dark_frame_ratio: Option<f32>,
    pub blur_score: Option<f32>,
    pub scene_cut_rate: Option<f32>,
    // Visual — segments
    pub black_segments: Vec<BlackSegment>,
    pub total_black_secs: Option<f64>,
    // Audio — aggregate
    pub rms_db: Option<f32>,
    pub peak_db: Option<f32>,
    pub integrated_lufs: Option<f32>,
    pub dynamic_range_db: Option<f32>,
    pub silence_ratio: Option<f32>,
    pub clipping_ratio: Option<f32>,
    // Audio — segments
    pub silence_segments: Vec<SilenceSegment>,
    pub total_silence_secs: Option<f64>,
    // Pre-filter flags (derived; fast-exit in compare_all)
    pub is_too_dark: bool,
    pub is_silent: bool,
    pub is_clipping: bool,
    pub has_decode_errors: bool,
    pub is_intro_dominated: bool,
    pub is_unreliable_fingerprint: bool,
    // Per-group quality ranking flags (set by ranker::compute_best_flags after scan)
    pub best_flags: Option<crate::ranker::BestFlags>,
}

// ═════════════════════════════════════════════════════════════════════════════
// FLAGS LAYER  (mirrors C# EntryFlags)
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileFlags {
    pub manually_excluded: bool,
    pub thumbnail_error: bool,
    pub metadata_error: bool,
    pub no_audio_track: bool,
    pub audio_fingerprint_error: bool,
    pub silent_audio_track: bool,
    pub is_missing: bool,
    pub scan_error: Option<String>,
    pub is_placeholder: bool,
}

// ═════════════════════════════════════════════════════════════════════════════
// TAGS & EXTERNAL IDS
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContainerTags {
    pub title: Option<String>,
    pub description: Option<String>,
    pub comment: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub language: Option<String>,
    pub copyright: Option<String>,
    pub encoder: Option<String>,
    pub show: Option<String>,
    pub season_number: Option<i32>,
    pub episode_sort: Option<i32>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub composer: Option<String>,
    pub track: Option<String>,
    pub disc: Option<String>,
    pub bpm: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalIds {
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub musicbrainz_recording_id: Option<String>,
    pub musicbrainz_release_id: Option<String>,
    pub acoustid: Option<String>,
    pub isrc: Option<String>,
    /// Path to cached MPEG-7 binary signature file on disk.
    pub mpeg7_sig_path: Option<String>,
}

// ═════════════════════════════════════════════════════════════════════════════
// FILE RECORD  — the central `file` graph node
// ═════════════════════════════════════════════════════════════════════════════

pub type FileId = String;

/// Full media file record.  All sub-schemas are optional objects; the scan
/// engine populates them progressively across the three hashing phases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    // ── Identity ────────────────────────────────────────────────────────────
    pub id: FileId,
    pub path: Utf8PathBuf,
    pub name: String,
    pub extension: String,
    pub media_type: MediaType,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    pub created_at: Option<u64>,
    pub modified_at: Option<u64>,
    pub scanned_at: u64,

    // ── Sub-schemas ──────────────────────────────────────────────────────────
    pub container: Option<ContainerInfo>,
    pub video_streams: Vec<VideoStreamInfo>,
    pub audio_streams: Vec<AudioStreamInfo>,
    pub subtitle_streams: Vec<SubtitleStreamInfo>,
    pub tags: Option<ContainerTags>,
    pub fingerprints: Option<Fingerprints>,
    pub analysis: Option<Analysis>,
    pub flags: Option<FileFlags>,
    pub external_ids: Option<ExternalIds>,
}

impl FileRecord {
    pub fn new(path: Utf8PathBuf, size_bytes: u64) -> Self {
        let name = path.file_name().unwrap_or(path.as_str()).to_string();
        let extension = path.extension().unwrap_or("").to_lowercase();
        let media_type = MediaType::from_extension(&extension);
        Self {
            id: file_id(path.as_str()),
            path,
            name,
            extension,
            media_type,
            size_bytes,
            sha256: None,
            created_at: None,
            modified_at: None,
            scanned_at: unix_now(),
            container: None,
            video_streams: vec![],
            audio_streams: vec![],
            subtitle_streams: vec![],
            tags: None,
            fingerprints: None,
            analysis: None,
            flags: None,
            external_ids: None,
        }
    }

    /// Populate container + stream info from ffmpeg MediaInfo.
    pub fn set_media_info(&mut self, info: crate::ffmpeg::MediaInfo) {
        self.container = Some(ContainerInfo {
            duration_secs: Some(info.duration_secs),
            overall_bitrate_kbps: if info.bit_rate > 0 {
                Some(info.bit_rate / 1000)
            } else {
                None
            },
            width: Some(info.width),
            height: Some(info.height),
            ..Default::default()
        });
        self.video_streams = vec![VideoStreamInfo {
            index: 0,
            codec_name: info.video_codec.clone(),
            width: Some(info.width),
            height: Some(info.height),
            fps: Some(info.frame_rate as f32),
            bitrate_kbps: if info.bit_rate > 0 { Some(info.bit_rate / 1000) } else { None },
            pixel_format: info.pixel_format.clone(),
            is_default: true,
            ..Default::default()
        }];
        if info.has_audio {
            self.audio_streams = vec![AudioStreamInfo {
                index: 1,
                codec_name: info.audio_codec.clone().unwrap_or_default(),
                sample_rate_hz: info.audio_sample_rate,
                channels: info.audio_channels,
                bitrate_kbps: info.audio_bit_rate.map(|b| b / 1000),
                is_default: true,
                ..Default::default()
            }];
        }
    }

    /// Set standard pHash fingerprint from raw (timestamp_secs → hash) pairs.
    pub fn set_phash_from_map(&mut self, map: &std::collections::HashMap<u64, u64>) {
        let mut samples: Vec<PhashSample> = map
            .iter()
            .map(|(&ts_ms, &hash)| PhashSample {
                ts: ts_ms as f64 / 1000.0,
                hash: hash as i64,
                brightness: 0.5, // placeholder; caller can patch if brightness is known
                skipped: false,
                skip_reason: None,
            })
            .collect();
        samples.sort_by(|a, b| a.ts.partial_cmp(&b.ts).unwrap_or(std::cmp::Ordering::Equal));
        let fp = PhashFingerprint::from_samples(samples);
        self.fingerprints.get_or_insert_with(Fingerprints::default).phash = Some(fp);
    }

    /// Set I-frame fingerprint from parallel (timestamps, hashes) vectors.
    pub fn set_iframe_fingerprint(&mut self, timestamps: Vec<f64>, hashes: Vec<u64>) {
        let samples: Vec<IframeSample> = timestamps
            .iter()
            .zip(hashes.iter())
            .map(|(&ts, &hash)| IframeSample {
                ts,
                hash: hash as i64,
                brightness: None,
                skipped: false,
                skip_reason: None,
                f_size_bytes: None,
                q: None,
                psnr: None,
                br_kbps: None,
                avg_br_kbps: None,
            })
            .collect();
        let count = samples.len() as u32;
        let mut ifp = IframeFingerprint {
            computed_at: Some(unix_now()),
            samples,
            sample_count: count,
            ..Default::default()
        };
        let dur = self.duration_secs();
        if dur > 0.0 {
            ifp.compute_gop_stats(dur);
        }
        self.fingerprints
            .get_or_insert_with(Fingerprints::default)
            .iframe = Some(ifp);
    }

    /// Set audio (Chromaprint) fingerprint.
    pub fn set_audio_fingerprint(&mut self, fp: Vec<u32>) {
        let dur = Some(self.duration_secs()).filter(|&d| d > 0.0);
        let chroma = ChromaprintFingerprint::from_u32_vec(fp, Some(0), dur);
        self.fingerprints
            .get_or_insert_with(Fingerprints::default)
            .chromaprint = Some(chroma);
    }

    // ── Convenience accessors ────────────────────────────────────────────────

    pub fn duration_secs(&self) -> f64 {
        self.container.as_ref().and_then(|c| c.duration_secs).unwrap_or(0.0)
    }
    pub fn width(&self) -> Option<u32> {
        self.container.as_ref().and_then(|c| c.width)
    }
    pub fn height(&self) -> Option<u32> {
        self.container.as_ref().and_then(|c| c.height)
    }
    pub fn is_image(&self) -> bool {
        self.media_type.is_image()
    }
    pub fn is_too_dark(&self) -> bool {
        self.analysis.as_ref().map(|a| a.is_too_dark).unwrap_or(false)
    }
    pub fn is_silent(&self) -> bool {
        self.analysis.as_ref().map(|a| a.is_silent).unwrap_or(false)
    }
    pub fn is_manually_excluded(&self) -> bool {
        self.flags.as_ref().map(|f| f.manually_excluded).unwrap_or(false)
    }
    pub fn has_audio(&self) -> bool {
        !self.audio_streams.is_empty()
    }

    /// Usable pHash values (non-skipped) in timestamp order.
    pub fn phash_hashes(&self) -> Vec<u64> {
        self.fingerprints
            .as_ref()
            .and_then(|fp| fp.phash.as_ref())
            .map(|p| p.usable_hashes())
            .unwrap_or_default()
    }

    /// First pHash value (for quick single-hash comparison).
    pub fn first_phash(&self) -> Option<u64> {
        self.fingerprints
            .as_ref()
            .and_then(|fp| fp.phash.as_ref())
            .and_then(|p| p.hash_vector.first())
            .map(|&h| h as u64)
    }

    /// I-frame hashes (usable only).
    pub fn iframe_hashes(&self) -> Vec<u64> {
        self.fingerprints
            .as_ref()
            .and_then(|fp| fp.iframe.as_ref())
            .map(|i| i.usable_hashes())
            .unwrap_or_default()
    }

    /// I-frame timestamps (usable only), in seconds.
    pub fn iframe_timestamps(&self) -> Vec<f64> {
        self.fingerprints
            .as_ref()
            .and_then(|fp| fp.iframe.as_ref())
            .map(|i| i.usable_timestamps())
            .unwrap_or_default()
    }

    /// Chromaprint audio fingerprint as Vec<u32>.
    pub fn audio_fingerprint(&self) -> Vec<u32> {
        self.fingerprints
            .as_ref()
            .and_then(|fp| fp.chromaprint.as_ref())
            .map(|c| c.as_u32_vec())
            .unwrap_or_default()
    }

    /// Whether the chromaprint fingerprint is present and non-empty.
    pub fn has_audio_fingerprint(&self) -> bool {
        self.fingerprints
            .as_ref()
            .and_then(|fp| fp.chromaprint.as_ref())
            .map(|c| !c.fingerprint.is_empty())
            .unwrap_or(false)
    }

    /// Temporal average hash for fast pre-filter.
    pub fn temporal_avg_hash(&self) -> Option<u64> {
        self.fingerprints
            .as_ref()
            .and_then(|fp| fp.temporal_avg.as_ref())
            .and_then(|t| t.hash)
            .map(|h| h as u64)
    }

    // ── Quality-ranking accessors (used by ranker.rs) ────────────────────────

    /// Frame rate of the primary video stream (fps).
    pub fn frame_rate(&self) -> Option<f32> {
        self.video_streams.first().and_then(|v| v.fps)
    }

    /// Bit-rate of the primary video stream in kbps.
    pub fn video_bitrate_kbps(&self) -> Option<i64> {
        self.video_streams.first().and_then(|v| v.bitrate_kbps)
    }

    /// Sample rate of the primary audio stream in Hz.
    pub fn audio_sample_rate(&self) -> Option<u32> {
        self.audio_streams.first().and_then(|a| a.sample_rate_hz)
    }

    /// Bit-rate of the primary audio stream in kbps.
    pub fn audio_bitrate_kbps(&self) -> Option<i64> {
        self.audio_streams.first().and_then(|a| a.bitrate_kbps)
    }

    /// Numeric rank for the HDR format of the primary video stream.
    ///
    /// Higher = better.  Mirrors the ordering used in VDF.Core
    /// (DolbyVision > HDR10+ > HDR10 > HLG > SDR).
    pub fn hdr_format_rank(&self) -> u8 {
        let hdr = self.video_streams.first().and_then(|v| v.hdr_format.as_deref());
        match hdr {
            Some(f) if f.contains("Dolby Vision") => 4,
            Some(f) if f.contains("HDR10+")       => 3,
            Some(f) if f.contains("HDR10")         => 2,
            Some(f) if f.contains("HLG")           => 1,
            _                                       => 0,
        }
    }

    /// Overall (container-level) bit-rate in kbps.
    pub fn overall_bitrate_kbps(&self) -> Option<i64> {
        self.container.as_ref().and_then(|c| c.overall_bitrate_kbps)
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// DUPLICATE PAIR  — full evidence edge
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicatePair {
    pub file_a: FileId,
    pub file_b: FileId,
    pub similarity: f32,
    pub method: MatchMethod,
    /// Seconds into file_a where file_b content begins (partial-clip detection).
    pub clip_offset_secs: Option<f64>,
    /// Audio alignment offset in seconds.
    pub audio_offset_secs: Option<f64>,
    /// Longest consecutive matching I-frame run at the best offset.
    pub consecutive_frames: Option<u32>,
    /// Start index into the longer file's I-frame array.
    pub best_offset_idx: Option<u32>,
    /// SSIM score from second-pass verification (0.0–1.0).
    pub ssim_score: Option<f32>,
    /// True if match required horizontally flipping one file (mirror detection).
    pub is_flipped: bool,
    /// Per-frame pHash similarity scores for the matched window.
    pub phash_scores: Vec<f32>,
}

impl DuplicatePair {
    pub fn new(
        file_a: FileId,
        file_b: FileId,
        similarity: f32,
        method: MatchMethod,
    ) -> Self {
        Self {
            file_a,
            file_b,
            similarity,
            method,
            clip_offset_secs: None,
            audio_offset_secs: None,
            consecutive_frames: None,
            best_offset_idx: None,
            ssim_score: None,
            is_flipped: false,
            phash_scores: vec![],
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// OTHER NODE TYPES
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationRecord {
    pub id: String,
    pub path: String,
    pub name: String,
    pub label: Option<String>,
    pub mount_type: Option<String>,
    pub scanned_at: u64,
    pub file_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanJob {
    pub id: String,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    /// "running" | "complete" | "failed" | "cancelled"
    pub status: String,
    pub files_discovered: u64,
    pub files_hashed: u64,
    pub files_compared: u64,
    pub duplicates_found: u64,
    pub settings_snapshot: serde_json::Value,
    pub error: Option<String>,
}

impl ScanJob {
    pub fn new(settings: serde_json::Value) -> Self {
        let id =
            format!("{:x}", Sha256::digest(unix_now().to_string().as_bytes()))[..16].to_string();
        Self {
            id,
            started_at: unix_now(),
            finished_at: None,
            status: "running".to_string(),
            files_discovered: 0,
            files_hashed: 0,
            files_compared: 0,
            duplicates_found: 0,
            settings_snapshot: settings,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserTag {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlacklistEntry {
    pub file_a: FileId,
    pub file_b: FileId,
    pub added_at: u64,
    pub reason: Option<String>,
}

impl BlacklistEntry {
    pub fn new(file_a: FileId, file_b: FileId, reason: Option<String>) -> Self {
        Self { file_a, file_b, added_at: unix_now(), reason }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// DATABASE TRAIT
// ═════════════════════════════════════════════════════════════════════════════

pub trait Database: Send + Sync {
    // ── File CRUD ────────────────────────────────────────────────────────────
    fn upsert_file(&mut self, record: FileRecord) -> VdfResult<()>;
    fn get_file(&self, id: &str) -> VdfResult<Option<FileRecord>>;
    fn get_file_by_path(&self, path: &Utf8Path) -> VdfResult<Option<FileRecord>>;
    fn all_files(&self) -> VdfResult<Vec<FileRecord>>;
    fn delete_file(&mut self, id: &str) -> VdfResult<()>;
    fn count_files(&self) -> VdfResult<u64>;

    // ── Duplicate edges ───────────────────────────────────────────────────────
    fn add_duplicate(&mut self, pair: DuplicatePair) -> VdfResult<()>;
    fn all_duplicates(&self) -> VdfResult<Vec<DuplicatePair>>;
    fn duplicates_of(&self, file_id: &str) -> VdfResult<Vec<DuplicatePair>>;
    fn clear_duplicates(&mut self) -> VdfResult<()>;
    fn pair_is_blacklisted(&self, id_a: &str, id_b: &str) -> VdfResult<bool>;

    // ── Blacklist ─────────────────────────────────────────────────────────────
    fn add_blacklist(&mut self, entry: BlacklistEntry) -> VdfResult<()>;
    fn all_blacklisted(&self) -> VdfResult<Vec<BlacklistEntry>>;
    fn remove_blacklist(&mut self, file_a: &str, file_b: &str) -> VdfResult<()>;

    // ── Scan jobs ─────────────────────────────────────────────────────────────
    fn create_scan_job(&mut self, job: ScanJob) -> VdfResult<()>;
    fn update_scan_job(&mut self, job: &ScanJob) -> VdfResult<()>;
    fn finish_scan_job(&mut self, id: &str, ok: bool, error: Option<String>) -> VdfResult<()>;
    fn latest_scan_job(&self) -> VdfResult<Option<ScanJob>>;

    // ── User tags ─────────────────────────────────────────────────────────────
    fn create_tag(&mut self, tag: UserTag) -> VdfResult<()>;
    fn all_tags(&self) -> VdfResult<Vec<UserTag>>;
    fn tag_file(&mut self, file_id: &str, tag_id: &str) -> VdfResult<()>;
    fn untag_file(&mut self, file_id: &str, tag_id: &str) -> VdfResult<()>;

    // ── Queries ───────────────────────────────────────────────────────────────
    fn files_in_location(&self, location_path: &str) -> VdfResult<Vec<FileRecord>>;
    fn files_needing_fingerprint(&self) -> VdfResult<Vec<FileRecord>>;
    fn files_with_errors(&self) -> VdfResult<Vec<FileRecord>>;

    // ── Housekeeping ──────────────────────────────────────────────────────────
    fn flush(&mut self) -> VdfResult<()>;
    fn db_version(&self) -> u32;

    // ── Bulk operations ───────────────────────────────────────────────────────

    /// Delete a file record and all its edges by record ID.
    /// Equivalent to `delete_file` but also removes all `duplicate_of` edges
    /// attached to the file node.
    fn remove_file(&mut self, id: &str) -> VdfResult<()>;

    /// Delete every file, location, duplicate edge, and scan-job record in
    /// the database, leaving the schema intact.
    fn clear_all(&mut self) -> VdfResult<()>;
}

// ═════════════════════════════════════════════════════════════════════════════
// SCHEMA DDL
// ═════════════════════════════════════════════════════════════════════════════

const SCHEMA_DDL: &str = "
USE NS vdf DB scanner;

-- ── location ──────────────────────────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS location SCHEMALESS;
DEFINE FIELD IF NOT EXISTS path       ON location TYPE string;
DEFINE FIELD IF NOT EXISTS name       ON location TYPE string;
DEFINE FIELD IF NOT EXISTS label      ON location TYPE option<string>;
DEFINE FIELD IF NOT EXISTS mount_type ON location TYPE option<string>;
DEFINE FIELD IF NOT EXISTS scanned_at ON location TYPE int;
DEFINE FIELD IF NOT EXISTS file_count ON location TYPE int DEFAULT 0;
DEFINE INDEX IF NOT EXISTS location_path_unique ON location FIELDS path UNIQUE;

-- ── file ──────────────────────────────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS file SCHEMALESS;
DEFINE FIELD IF NOT EXISTS path            ON file TYPE string;
DEFINE FIELD IF NOT EXISTS name            ON file TYPE string;
DEFINE FIELD IF NOT EXISTS extension       ON file TYPE string;
DEFINE FIELD IF NOT EXISTS media_type      ON file TYPE string;
DEFINE FIELD IF NOT EXISTS size_bytes      ON file TYPE int;
DEFINE FIELD IF NOT EXISTS sha256          ON file TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at      ON file TYPE option<int>;
DEFINE FIELD IF NOT EXISTS modified_at     ON file TYPE option<int>;
DEFINE FIELD IF NOT EXISTS scanned_at      ON file TYPE int;
DEFINE FIELD IF NOT EXISTS container       ON file TYPE option<object>;
DEFINE FIELD IF NOT EXISTS video_streams   ON file TYPE array<object>;
DEFINE FIELD IF NOT EXISTS audio_streams   ON file TYPE array<object>;
DEFINE FIELD IF NOT EXISTS subtitle_streams ON file TYPE array<object>;
DEFINE FIELD IF NOT EXISTS tags            ON file TYPE option<object>;
DEFINE FIELD IF NOT EXISTS fingerprints    ON file TYPE option<object>;
DEFINE FIELD IF NOT EXISTS analysis        ON file TYPE option<object>;
DEFINE FIELD IF NOT EXISTS flags           ON file TYPE option<object>;
DEFINE FIELD IF NOT EXISTS external_ids    ON file TYPE option<object>;
DEFINE INDEX IF NOT EXISTS file_path_unique  ON file FIELDS path UNIQUE;
DEFINE INDEX IF NOT EXISTS file_media_type   ON file FIELDS media_type;
DEFINE INDEX IF NOT EXISTS file_extension    ON file FIELDS extension;
DEFINE INDEX IF NOT EXISTS file_size         ON file FIELDS size_bytes;
DEFINE INDEX IF NOT EXISTS file_modified     ON file FIELDS modified_at;
DEFINE INDEX IF NOT EXISTS file_duration     ON file FIELDS container.duration_secs;
DEFINE INDEX IF NOT EXISTS file_width        ON file FIELDS container.width;
DEFINE INDEX IF NOT EXISTS file_too_dark     ON file FIELDS analysis.is_too_dark;
DEFINE INDEX IF NOT EXISTS file_silent       ON file FIELDS analysis.is_silent;
DEFINE INDEX IF NOT EXISTS file_unreliable   ON file FIELDS analysis.is_unreliable_fingerprint;
DEFINE INDEX IF NOT EXISTS file_missing      ON file FIELDS flags.is_missing;
DEFINE INDEX IF NOT EXISTS file_placeholder  ON file FIELDS flags.is_placeholder;
DEFINE INDEX IF NOT EXISTS file_fp_complete  ON file FIELDS fingerprints.all_phases_complete;
DEFINE INDEX IF NOT EXISTS file_kf_density   ON file FIELDS fingerprints.iframe.keyframe_density_per_min;
DEFINE ANALYZER IF NOT EXISTS media_text
    TOKENIZERS blank, class
    FILTERS lowercase, ascii, snowball(english);
DEFINE INDEX IF NOT EXISTS file_name_fts ON file FIELDS name
    SEARCH ANALYZER media_text BM25(1.2, 0.75);

-- ── in_folder  (file → location) ─────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS in_folder TYPE RELATION IN file OUT location SCHEMALESS;

-- ── duplicate_of  (file → file)  full evidence ────────────────────────────────
DEFINE TABLE IF NOT EXISTS duplicate_of TYPE RELATION IN file OUT file SCHEMALESS;
DEFINE FIELD IF NOT EXISTS similarity         ON duplicate_of TYPE float;
DEFINE FIELD IF NOT EXISTS method             ON duplicate_of TYPE string;
DEFINE FIELD IF NOT EXISTS clip_offset_secs   ON duplicate_of TYPE option<float>;
DEFINE FIELD IF NOT EXISTS audio_offset_secs  ON duplicate_of TYPE option<float>;
DEFINE FIELD IF NOT EXISTS consecutive_frames ON duplicate_of TYPE option<int>;
DEFINE FIELD IF NOT EXISTS best_offset_idx    ON duplicate_of TYPE option<int>;
DEFINE FIELD IF NOT EXISTS ssim_score         ON duplicate_of TYPE option<float>;
DEFINE FIELD IF NOT EXISTS is_flipped         ON duplicate_of TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS phash_scores       ON duplicate_of TYPE array<float>;
DEFINE FIELD IF NOT EXISTS discovered_at      ON duplicate_of TYPE int;
DEFINE INDEX IF NOT EXISTS dup_similarity ON duplicate_of FIELDS similarity;
DEFINE INDEX IF NOT EXISTS dup_method     ON duplicate_of FIELDS method;

-- ── blacklisted  (file → file) ────────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS blacklisted TYPE RELATION IN file OUT file SCHEMALESS;
DEFINE FIELD IF NOT EXISTS added_at ON blacklisted TYPE int;
DEFINE FIELD IF NOT EXISTS reason   ON blacklisted TYPE option<string>;

-- ── stem_of  (file → file) ────────────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS stem_of TYPE RELATION IN file OUT file SCHEMALESS;
DEFINE FIELD IF NOT EXISTS stem_type       ON stem_of TYPE string;
DEFINE FIELD IF NOT EXISTS stem_label      ON stem_of TYPE option<string>;
DEFINE FIELD IF NOT EXISTS separator_tool  ON stem_of TYPE option<string>;
DEFINE FIELD IF NOT EXISTS separator_model ON stem_of TYPE option<string>;
DEFINE FIELD IF NOT EXISTS quality_score   ON stem_of TYPE option<float>;

-- ── scan_job ──────────────────────────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS scan_job SCHEMALESS;
DEFINE FIELD IF NOT EXISTS started_at       ON scan_job TYPE int;
DEFINE FIELD IF NOT EXISTS finished_at      ON scan_job TYPE option<int>;
DEFINE FIELD IF NOT EXISTS status           ON scan_job TYPE string;
DEFINE FIELD IF NOT EXISTS files_discovered ON scan_job TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS files_hashed     ON scan_job TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS files_compared   ON scan_job TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS duplicates_found ON scan_job TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS settings_snapshot ON scan_job TYPE option<object>;
DEFINE FIELD IF NOT EXISTS error            ON scan_job TYPE option<string>;
DEFINE INDEX IF NOT EXISTS scan_job_status  ON scan_job FIELDS status;
DEFINE INDEX IF NOT EXISTS scan_job_started ON scan_job FIELDS started_at;

-- ── scanned_in  (file → scan_job) ────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS scanned_in TYPE RELATION IN file OUT scan_job SCHEMALESS;
DEFINE FIELD IF NOT EXISTS phase      ON scanned_in TYPE string;
DEFINE FIELD IF NOT EXISTS scanned_at ON scanned_in TYPE int;

-- ── user_tag ──────────────────────────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS user_tag SCHEMALESS;
DEFINE FIELD IF NOT EXISTS name        ON user_tag TYPE string;
DEFINE FIELD IF NOT EXISTS color       ON user_tag TYPE option<string>;
DEFINE FIELD IF NOT EXISTS description ON user_tag TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at  ON user_tag TYPE int;
DEFINE INDEX IF NOT EXISTS tag_name_unique ON user_tag FIELDS name UNIQUE;

-- ── tagged_with  (file → user_tag) ───────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS tagged_with TYPE RELATION IN file OUT user_tag SCHEMALESS;
DEFINE FIELD IF NOT EXISTS added_at ON tagged_with TYPE int;

-- ── encode_profile ────────────────────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS encode_profile SCHEMALESS;
DEFINE FIELD IF NOT EXISTS name       ON encode_profile TYPE string;
DEFINE FIELD IF NOT EXISTS codec      ON encode_profile TYPE string;
DEFINE FIELD IF NOT EXISTS crf        ON encode_profile TYPE option<int>;
DEFINE FIELD IF NOT EXISTS preset     ON encode_profile TYPE option<string>;
DEFINE FIELD IF NOT EXISTS hw_accel   ON encode_profile TYPE option<string>;
DEFINE FIELD IF NOT EXISTS extra_args ON encode_profile TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON encode_profile TYPE int;

-- ── encode_job ────────────────────────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS encode_job SCHEMALESS;
DEFINE FIELD IF NOT EXISTS status         ON encode_job TYPE string;
DEFINE FIELD IF NOT EXISTS ffmpeg_command ON encode_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS started_at     ON encode_job TYPE int;
DEFINE FIELD IF NOT EXISTS finished_at    ON encode_job TYPE option<int>;
DEFINE FIELD IF NOT EXISTS input_size     ON encode_job TYPE option<int>;
DEFINE FIELD IF NOT EXISTS output_size    ON encode_job TYPE option<int>;
DEFINE FIELD IF NOT EXISTS savings_bytes  ON encode_job TYPE option<int>;
DEFINE FIELD IF NOT EXISTS error          ON encode_job TYPE option<string>;

-- ── trim_edit ─────────────────────────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS trim_edit SCHEMALESS;
DEFINE FIELD IF NOT EXISTS in_point_secs  ON trim_edit TYPE option<float>;
DEFINE FIELD IF NOT EXISTS out_point_secs ON trim_edit TYPE option<float>;
DEFINE FIELD IF NOT EXISTS crop_left      ON trim_edit TYPE option<int>;
DEFINE FIELD IF NOT EXISTS crop_right     ON trim_edit TYPE option<int>;
DEFINE FIELD IF NOT EXISTS crop_top       ON trim_edit TYPE option<int>;
DEFINE FIELD IF NOT EXISTS crop_bottom    ON trim_edit TYPE option<int>;
DEFINE FIELD IF NOT EXISTS created_at     ON trim_edit TYPE int;
DEFINE FIELD IF NOT EXISTS label          ON trim_edit TYPE option<string>;

-- ── meta  (version singleton) ─────────────────────────────────────────────────
DEFINE TABLE IF NOT EXISTS meta SCHEMALESS;
";

// ═════════════════════════════════════════════════════════════════════════════
// SURREAL DATABASE IMPL
// ═════════════════════════════════════════════════════════════════════════════

pub struct SurrealDatabase {
    rt: tokio::runtime::Runtime,
    db: Surreal<Db>,
}

impl SurrealDatabase {
    pub fn open(path: impl AsRef<std::path::Path>) -> VdfResult<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| VdfError::Database(e.to_string()))?;

        let db_path = path.as_ref().to_path_buf();
        let db = rt
            .block_on(async move {
                let db: Surreal<Db> = Surreal::new::<RocksDb>(db_path).await?;
                db.use_ns(NS).use_db(DB_NAME).await?;
                db.query(SCHEMA_DDL).await?;
                db.upsert::<Option<serde_json::Value>>(("meta", "version"))
                    .content(serde_json::json!({ "version": DB_SCHEMA_VERSION }))
                    .await?;
                Ok::<Surreal<Db>, surrealdb::Error>(db)
            })
            .map_err(|e: surrealdb::Error| VdfError::Database(e.to_string()))?;

        info!(
            "opened SurrealDB (NS={NS} DB={DB_NAME}) schema_v{DB_SCHEMA_VERSION} at {}",
            path.as_ref().display()
        );
        Ok(Self { rt, db })
    }

    fn ensure_location(&self, folder_path: &str) -> VdfResult<String> {
        let loc_id = folder_id(folder_path);
        let name = std::path::Path::new(folder_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(folder_path)
            .to_string();
        let path_str = folder_path.to_string();
        let loc_id2 = loc_id.clone();
        let now = unix_now();
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query(
                    "UPSERT type::thing('location', $id) \
                     SET path = $path, name = $name, scanned_at = $now",
                )
                .bind(("id", loc_id2))
                .bind(("path", path_str))
                .bind(("name", name))
                .bind(("now", now))
                .await
                .map(|_| ())
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;
        Ok(loc_id)
    }
}

impl Database for SurrealDatabase {
    fn upsert_file(&mut self, record: FileRecord) -> VdfResult<()> {
        let folder = record.path.parent().map(|p| p.as_str()).unwrap_or("").to_string();
        let loc_id = self.ensure_location(&folder)?;
        let file_id_str = record.id.clone();

        let json_val = serde_json::to_value(&record)
            .map_err(|e| VdfError::Database(e.to_string()))?;

        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query("UPSERT type::thing('file', $id) CONTENT $data")
                    .bind(("id", file_id_str.clone()))
                    .bind(("data", json_val))
                    .await?;
                db.query(
                    "RELATE type::thing('file', $fid) -> in_folder \
                     -> type::thing('location', $lid)",
                )
                .bind(("fid", file_id_str))
                .bind(("lid", loc_id))
                .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        debug!("upserted file:{}", record.id);
        Ok(())
    }

    fn get_file(&self, id: &str) -> VdfResult<Option<FileRecord>> {
        let id = id.to_string();
        let db = &self.db;
        let raw: Vec<serde_json::Value> = self
            .rt
            .block_on(async move {
                let mut res = db
                    .query("SELECT * FROM type::thing('file', $id)")
                    .bind(("id", id))
                    .await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        Ok(raw
            .into_iter()
            .next()
            .and_then(|v| serde_json::from_value::<FileRecord>(v).ok()))
    }

    fn get_file_by_path(&self, path: &Utf8Path) -> VdfResult<Option<FileRecord>> {
        self.get_file(&file_id(path.as_str()))
    }

    fn all_files(&self) -> VdfResult<Vec<FileRecord>> {
        let db = &self.db;
        let raw: Vec<serde_json::Value> = self
            .rt
            .block_on(async {
                let mut res = db.query("SELECT * FROM file").await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        let records: Vec<FileRecord> = raw
            .into_iter()
            .filter_map(|v| {
                serde_json::from_value::<FileRecord>(v)
                    .map_err(|e| warn!("failed to deserialize FileRecord: {e}"))
                    .ok()
            })
            .collect();
        Ok(records)
    }

    fn delete_file(&mut self, id: &str) -> VdfResult<()> {
        let id = id.to_string();
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query("DELETE type::thing('file', $id)").bind(("id", id)).await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))
    }

    fn count_files(&self) -> VdfResult<u64> {
        let db = &self.db;
        let rows: Vec<serde_json::Value> = self
            .rt
            .block_on(async {
                let mut res =
                    db.query("SELECT count() AS n FROM file GROUP ALL").await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;
        Ok(rows
            .first()
            .and_then(|r| r.get("n"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0))
    }

    fn add_duplicate(&mut self, pair: DuplicatePair) -> VdfResult<()> {
        let fa = pair.file_a.clone();
        let fb = pair.file_b.clone();
        let method = format!("{:?}", pair.method);
        let now = unix_now();
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query(
                    "RELATE type::thing('file', $a) -> duplicate_of -> type::thing('file', $b) \
                     SET similarity = $sim, method = $method, \
                         clip_offset_secs = $clip, audio_offset_secs = $audio, \
                         consecutive_frames = $consec, best_offset_idx = $best, \
                         ssim_score = $ssim, is_flipped = $flip, \
                         phash_scores = $scores, discovered_at = $now",
                )
                .bind(("a", fa))
                .bind(("b", fb))
                .bind(("sim", pair.similarity))
                .bind(("method", method))
                .bind(("clip", pair.clip_offset_secs))
                .bind(("audio", pair.audio_offset_secs))
                .bind(("consec", pair.consecutive_frames))
                .bind(("best", pair.best_offset_idx))
                .bind(("ssim", pair.ssim_score))
                .bind(("flip", pair.is_flipped))
                .bind(("scores", pair.phash_scores))
                .bind(("now", now))
                .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        debug!(
            "added duplicate_of: {} → {} ({:.1}% via {:?})",
            pair.file_a,
            pair.file_b,
            pair.similarity * 100.0,
            pair.method,
        );
        Ok(())
    }

    fn all_duplicates(&self) -> VdfResult<Vec<DuplicatePair>> {
        let db = &self.db;
        let rows: Vec<serde_json::Value> = self
            .rt
            .block_on(async {
                let mut res = db
                    .query(
                        "SELECT in, out, similarity, method, clip_offset_secs, \
                                 audio_offset_secs, consecutive_frames, best_offset_idx, \
                                 ssim_score, is_flipped, phash_scores \
                         FROM duplicate_of",
                    )
                    .await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        Ok(rows.into_iter().filter_map(parse_duplicate_row).collect())
    }

    fn duplicates_of(&self, id: &str) -> VdfResult<Vec<DuplicatePair>> {
        let id = id.to_string();
        let db = &self.db;
        let rows: Vec<serde_json::Value> = self
            .rt
            .block_on(async move {
                let mut res = db
                    .query(
                        "SELECT in, out, similarity, method, clip_offset_secs, \
                                 audio_offset_secs, consecutive_frames, best_offset_idx, \
                                 ssim_score, is_flipped, phash_scores \
                         FROM duplicate_of \
                         WHERE in = type::thing('file', $id) \
                            OR out = type::thing('file', $id)",
                    )
                    .bind(("id", id))
                    .await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        Ok(rows.into_iter().filter_map(parse_duplicate_row).collect())
    }

    fn clear_duplicates(&mut self) -> VdfResult<()> {
        let db = &self.db;
        self.rt
            .block_on(async {
                db.query("DELETE duplicate_of").await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;
        debug!("cleared all duplicate_of edges");
        Ok(())
    }

    fn pair_is_blacklisted(&self, id_a: &str, id_b: &str) -> VdfResult<bool> {
        let a = id_a.to_string();
        let b = id_b.to_string();
        let db = &self.db;
        let rows: Vec<serde_json::Value> = self
            .rt
            .block_on(async move {
                let mut res = db
                    .query(
                        "SELECT * FROM blacklisted \
                         WHERE (in = type::thing('file', $a) AND out = type::thing('file', $b)) \
                            OR (in = type::thing('file', $b) AND out = type::thing('file', $a))",
                    )
                    .bind(("a", a))
                    .bind(("b", b))
                    .await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;
        Ok(!rows.is_empty())
    }

    fn add_blacklist(&mut self, entry: BlacklistEntry) -> VdfResult<()> {
        let fa = entry.file_a.clone();
        let fb = entry.file_b.clone();
        let now = entry.added_at;
        let reason = entry.reason.clone();
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query(
                    "RELATE type::thing('file', $a) -> blacklisted -> type::thing('file', $b) \
                     SET added_at = $now, reason = $reason",
                )
                .bind(("a", fa))
                .bind(("b", fb))
                .bind(("now", now))
                .bind(("reason", reason))
                .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))
    }

    fn all_blacklisted(&self) -> VdfResult<Vec<BlacklistEntry>> {
        let db = &self.db;
        let rows: Vec<serde_json::Value> = self
            .rt
            .block_on(async {
                let mut res =
                    db.query("SELECT in, out, added_at, reason FROM blacklisted").await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let file_a = extract_record_id(row.get("in")?);
                let file_b = extract_record_id(row.get("out")?);
                let added_at = row.get("added_at")?.as_u64().unwrap_or(0);
                let reason =
                    row.get("reason").and_then(|v| v.as_str()).map(String::from);
                Some(BlacklistEntry { file_a, file_b, added_at, reason })
            })
            .collect())
    }

    fn remove_blacklist(&mut self, file_a: &str, file_b: &str) -> VdfResult<()> {
        let a = file_a.to_string();
        let b = file_b.to_string();
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query(
                    "DELETE blacklisted \
                     WHERE (in = type::thing('file', $a) AND out = type::thing('file', $b)) \
                        OR (in = type::thing('file', $b) AND out = type::thing('file', $a))",
                )
                .bind(("a", a))
                .bind(("b", b))
                .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))
    }

    fn create_scan_job(&mut self, job: ScanJob) -> VdfResult<()> {
        let id = job.id.clone();
        let val =
            serde_json::to_value(&job).map_err(|e| VdfError::Database(e.to_string()))?;
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query("UPSERT type::thing('scan_job', $id) CONTENT $data")
                    .bind(("id", id))
                    .bind(("data", val))
                    .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))
    }

    fn update_scan_job(&mut self, job: &ScanJob) -> VdfResult<()> {
        let id = job.id.clone();
        let val =
            serde_json::to_value(job).map_err(|e| VdfError::Database(e.to_string()))?;
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query("UPSERT type::thing('scan_job', $id) CONTENT $data")
                    .bind(("id", id))
                    .bind(("data", val))
                    .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))
    }

    fn finish_scan_job(&mut self, id: &str, ok: bool, error: Option<String>) -> VdfResult<()> {
        let id = id.to_string();
        let status = if ok { "complete".to_string() } else { "failed".to_string() };
        let now = unix_now();
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query(
                    "UPDATE type::thing('scan_job', $id) \
                     SET status = $status, finished_at = $now, error = $error",
                )
                .bind(("id", id))
                .bind(("status", status))
                .bind(("now", now))
                .bind(("error", error))
                .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))
    }

    fn latest_scan_job(&self) -> VdfResult<Option<ScanJob>> {
        let db = &self.db;
        let rows: Vec<serde_json::Value> = self
            .rt
            .block_on(async {
                let mut res = db
                    .query(
                        "SELECT * FROM scan_job ORDER BY started_at DESC LIMIT 1",
                    )
                    .await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        Ok(rows
            .into_iter()
            .next()
            .and_then(|v| serde_json::from_value::<ScanJob>(v).ok()))
    }

    fn create_tag(&mut self, tag: UserTag) -> VdfResult<()> {
        let id = tag.id.clone();
        let val =
            serde_json::to_value(&tag).map_err(|e| VdfError::Database(e.to_string()))?;
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query("UPSERT type::thing('user_tag', $id) CONTENT $data")
                    .bind(("id", id))
                    .bind(("data", val))
                    .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))
    }

    fn all_tags(&self) -> VdfResult<Vec<UserTag>> {
        let db = &self.db;
        let rows: Vec<serde_json::Value> = self
            .rt
            .block_on(async {
                let mut res =
                    db.query("SELECT * FROM user_tag ORDER BY name").await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        Ok(rows
            .into_iter()
            .filter_map(|v| serde_json::from_value::<UserTag>(v).ok())
            .collect())
    }

    fn tag_file(&mut self, file_id: &str, tag_id: &str) -> VdfResult<()> {
        let fid = file_id.to_string();
        let tid = tag_id.to_string();
        let now = unix_now();
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query(
                    "RELATE type::thing('file', $fid) -> tagged_with \
                     -> type::thing('user_tag', $tid) SET added_at = $now",
                )
                .bind(("fid", fid))
                .bind(("tid", tid))
                .bind(("now", now))
                .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))
    }

    fn untag_file(&mut self, file_id: &str, tag_id: &str) -> VdfResult<()> {
        let fid = file_id.to_string();
        let tid = tag_id.to_string();
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query(
                    "DELETE tagged_with \
                     WHERE in = type::thing('file', $fid) \
                       AND out = type::thing('user_tag', $tid)",
                )
                .bind(("fid", fid))
                .bind(("tid", tid))
                .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))
    }

    fn files_in_location(&self, location_path: &str) -> VdfResult<Vec<FileRecord>> {
        let loc_id = folder_id(location_path);
        let db = &self.db;
        let rows: Vec<serde_json::Value> = self
            .rt
            .block_on(async move {
                let mut res = db
                    .query(
                        "SELECT <-in_folder<-(file.*) AS f \
                         FROM type::thing('location', $lid)",
                    )
                    .bind(("lid", loc_id))
                    .await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        Ok(rows
            .into_iter()
            .filter_map(|v| {
                let file = v.get("f").cloned().unwrap_or(v);
                serde_json::from_value::<FileRecord>(file).ok()
            })
            .collect())
    }

    fn files_needing_fingerprint(&self) -> VdfResult<Vec<FileRecord>> {
        let db = &self.db;
        let rows: Vec<serde_json::Value> = self
            .rt
            .block_on(async {
                let mut res = db
                    .query(
                        "SELECT * FROM file \
                         WHERE fingerprints.all_phases_complete = false \
                            OR fingerprints IS NONE",
                    )
                    .await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        Ok(rows
            .into_iter()
            .filter_map(|v| serde_json::from_value::<FileRecord>(v).ok())
            .collect())
    }

    fn files_with_errors(&self) -> VdfResult<Vec<FileRecord>> {
        let db = &self.db;
        let rows: Vec<serde_json::Value> = self
            .rt
            .block_on(async {
                let mut res = db
                    .query(
                        "SELECT * FROM file \
                         WHERE flags.metadata_error = true \
                            OR flags.thumbnail_error = true \
                            OR flags.scan_error IS NOT NONE",
                    )
                    .await?;
                res.take::<Vec<serde_json::Value>>(0)
            })
            .map_err(|e| VdfError::Database(e.to_string()))?;

        Ok(rows
            .into_iter()
            .filter_map(|v| serde_json::from_value::<FileRecord>(v).ok())
            .collect())
    }

    fn remove_file(&mut self, id: &str) -> VdfResult<()> {
        let id = id.to_string();
        let db = &self.db;
        self.rt
            .block_on(async move {
                // Delete all duplicate_of edges involving this file, then the file node.
                db.query(
                    "DELETE duplicate_of WHERE in = type::thing('file', $id) \
                        OR out = type::thing('file', $id); \
                     DELETE type::thing('file', $id);",
                )
                .bind(("id", id))
                .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))
    }

    fn clear_all(&mut self) -> VdfResult<()> {
        let db = &self.db;
        self.rt
            .block_on(async move {
                db.query(
                    "DELETE file; \
                     DELETE location; \
                     DELETE duplicate_of; \
                     DELETE in_folder; \
                     DELETE scan_job; \
                     DELETE blacklist; \
                     DELETE analysis;",
                )
                .await?;
                Ok::<_, surrealdb::Error>(())
            })
            .map_err(|e| VdfError::Database(e.to_string()))
    }

    fn flush(&mut self) -> VdfResult<()> {
        debug!("SurrealDB flush: no-op (RocksDB writes are immediately durable)");
        Ok(())
    }

    fn db_version(&self) -> u32 {
        DB_SCHEMA_VERSION
    }
}

/// `ScanDatabase` is always `SurrealDatabase`.
pub type ScanDatabase = SurrealDatabase;

// ═════════════════════════════════════════════════════════════════════════════
// HELPERS
// ═════════════════════════════════════════════════════════════════════════════

/// Stable 16-char hex ID from SHA-256(path string).
pub fn file_id(path: impl AsRef<str>) -> FileId {
    let hash = Sha256::digest(path.as_ref().as_bytes());
    format!("{:x}", hash)[..16].to_string()
}

fn folder_id(path: &str) -> String {
    file_id(path)
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn extract_record_id(v: &serde_json::Value) -> FileId {
    match v {
        serde_json::Value::String(s) => {
            s.split(':').nth(1).unwrap_or(s.as_str()).to_string()
        }
        serde_json::Value::Object(o) => o
            .get("id")
            .and_then(|id| id.as_str())
            .and_then(|s| s.split(':').nth(1))
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

fn parse_duplicate_row(row: serde_json::Value) -> Option<DuplicatePair> {
    let file_a = extract_record_id(row.get("in")?);
    let file_b = extract_record_id(row.get("out")?);
    let similarity = row.get("similarity")?.as_f64()? as f32;
    let method_str = row.get("method")?.as_str().unwrap_or("");
    let method = match method_str {
        "FrameSimilarity" => MatchMethod::FrameSimilarity,
        "IframeTimeline" => MatchMethod::IframeTimeline,
        "AudioFingerprint" => MatchMethod::AudioFingerprint,
        "Mpeg7Signature" => MatchMethod::Mpeg7Signature,
        "SsimVerified" => MatchMethod::SsimVerified,
        "TemporalAverageHash" => MatchMethod::TemporalAverageHash,
        _ => MatchMethod::FrameSimilarity,
    };
    let clip_offset_secs = row.get("clip_offset_secs").and_then(|v| v.as_f64());
    let audio_offset_secs = row.get("audio_offset_secs").and_then(|v| v.as_f64());
    let consecutive_frames =
        row.get("consecutive_frames").and_then(|v| v.as_u64()).map(|v| v as u32);
    let best_offset_idx =
        row.get("best_offset_idx").and_then(|v| v.as_u64()).map(|v| v as u32);
    let ssim_score = row.get("ssim_score").and_then(|v| v.as_f64()).map(|v| v as f32);
    let is_flipped = row.get("is_flipped").and_then(|v| v.as_bool()).unwrap_or(false);
    let phash_scores = row
        .get("phash_scores")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect())
        .unwrap_or_default();

    Some(DuplicatePair {
        file_a,
        file_b,
        similarity,
        method,
        clip_offset_secs,
        audio_offset_secs,
        consecutive_frames,
        best_offset_idx,
        ssim_score,
        is_flipped,
        phash_scores,
    })
}

// ═════════════════════════════════════════════════════════════════════════════
// TESTS
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb::engine::local::Mem;

    async fn mem_db() -> Surreal<Db> {
        let db: Surreal<Db> = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns(NS).use_db(DB_NAME).await.unwrap();
        db.query(SCHEMA_DDL).await.unwrap();
        db
    }

    fn make_mem_db() -> SurrealDatabase {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let db = rt.block_on(mem_db());
        SurrealDatabase { rt, db }
    }

    #[test]
    fn insert_and_retrieve_file() {
        let mut db = make_mem_db();
        let rec = FileRecord::new(Utf8PathBuf::from("/test/video.mp4"), 1024);
        let id = rec.id.clone();
        db.upsert_file(rec).unwrap();
        let got = db.get_file(&id).unwrap().unwrap();
        assert_eq!(got.path.as_str(), "/test/video.mp4");
        assert_eq!(got.media_type, MediaType::Video);
    }

    #[test]
    fn media_type_detected_from_extension() {
        let img = FileRecord::new("/test/photo.jpg".into(), 100);
        assert!(img.is_image());
        let aud = FileRecord::new("/test/song.flac".into(), 200);
        assert!(aud.media_type.is_audio());
        let vid = FileRecord::new("/test/movie.mkv".into(), 300);
        assert!(vid.media_type.is_video());
    }

    #[test]
    fn all_files_returns_all_inserted() {
        let mut db = make_mem_db();
        db.upsert_file(FileRecord::new("/a.mp4".into(), 100)).unwrap();
        db.upsert_file(FileRecord::new("/b.mp4".into(), 200)).unwrap();
        assert_eq!(db.all_files().unwrap().len(), 2);
    }

    #[test]
    fn duplicate_of_full_evidence_edge() {
        let mut db = make_mem_db();
        let fa = FileRecord::new("/a.mp4".into(), 100);
        let fb = FileRecord::new("/b.mp4".into(), 200);
        let id_a = fa.id.clone();
        let id_b = fb.id.clone();
        db.upsert_file(fa).unwrap();
        db.upsert_file(fb).unwrap();

        let mut pair = DuplicatePair::new(id_a, id_b, 0.97, MatchMethod::IframeTimeline);
        pair.consecutive_frames = Some(15);
        pair.best_offset_idx = Some(3);
        pair.ssim_score = Some(0.92);
        pair.phash_scores = vec![0.95, 0.98, 0.96];
        db.add_duplicate(pair).unwrap();

        let pairs = db.all_duplicates().unwrap();
        assert_eq!(pairs.len(), 1);
        assert!((pairs[0].similarity - 0.97).abs() < 1e-4);
        assert_eq!(pairs[0].consecutive_frames, Some(15));
        assert_eq!(pairs[0].method, MatchMethod::IframeTimeline);
    }

    #[test]
    fn blacklist_bidirectional() {
        let mut db = make_mem_db();
        let fa = FileRecord::new("/x.mp4".into(), 1);
        let fb = FileRecord::new("/y.mp4".into(), 2);
        let id_a = fa.id.clone();
        let id_b = fb.id.clone();
        db.upsert_file(fa).unwrap();
        db.upsert_file(fb).unwrap();

        assert!(!db.pair_is_blacklisted(&id_a, &id_b).unwrap());
        db.add_blacklist(BlacklistEntry::new(
            id_a.clone(),
            id_b.clone(),
            Some("known false positive".into()),
        ))
        .unwrap();
        assert!(db.pair_is_blacklisted(&id_a, &id_b).unwrap());
        assert!(db.pair_is_blacklisted(&id_b, &id_a).unwrap());
    }

    #[test]
    fn phash_fingerprint_usable_hashes() {
        let samples = vec![
            PhashSample {
                ts: 10.0,
                hash: 0xDEAD_BEEF_i64,
                brightness: 0.5,
                skipped: false,
                skip_reason: None,
            },
            PhashSample {
                ts: 20.0,
                hash: 0x1234_5678_i64,
                brightness: 0.01,
                skipped: true,
                skip_reason: Some("too_dark".into()),
            },
            PhashSample {
                ts: 30.0,
                hash: 0xCAFE_BABE_i64,
                brightness: 0.6,
                skipped: false,
                skip_reason: None,
            },
        ];
        let fp = PhashFingerprint::from_samples(samples);
        assert_eq!(fp.sample_count, 3);
        assert_eq!(fp.usable_sample_count, 2);
        assert_eq!(fp.usable_hashes().len(), 2);
        assert!((fp.dark_frame_ratio.unwrap() - 1.0 / 3.0).abs() < 1e-4);
    }

    #[test]
    fn scan_job_lifecycle() {
        let mut db = make_mem_db();
        let job = ScanJob::new(serde_json::json!({"min_similarity": 0.95}));
        let id = job.id.clone();
        db.create_scan_job(job).unwrap();
        let latest = db.latest_scan_job().unwrap().unwrap();
        assert_eq!(latest.status, "running");
        db.finish_scan_job(&id, true, None).unwrap();
        let done = db.latest_scan_job().unwrap().unwrap();
        assert_eq!(done.status, "complete");
    }

    #[test]
    fn clear_duplicates_removes_all() {
        let mut db = make_mem_db();
        db.upsert_file(FileRecord::new("/x.mp4".into(), 1)).unwrap();
        db.upsert_file(FileRecord::new("/y.mp4".into(), 2)).unwrap();
        db.add_duplicate(DuplicatePair::new(
            file_id("/x.mp4"),
            file_id("/y.mp4"),
            0.95,
            MatchMethod::FrameSimilarity,
        ))
        .unwrap();
        assert_eq!(db.all_duplicates().unwrap().len(), 1);
        db.clear_duplicates().unwrap();
        assert_eq!(db.all_duplicates().unwrap().len(), 0);
    }

    #[test]
    fn chromaprint_silence_detection() {
        let silent_fp: Vec<u32> = vec![0u32; 100];
        let chroma = ChromaprintFingerprint::from_u32_vec(silent_fp, Some(0), Some(100.0));
        assert_eq!(chroma.silence_ratio, Some(1.0));
        assert!(!chroma.is_reliable);

        let real_fp: Vec<u32> = (1u32..=100).collect();
        let chroma2 = ChromaprintFingerprint::from_u32_vec(real_fp, Some(0), Some(100.0));
        assert_eq!(chroma2.silence_ratio, Some(0.0));
        assert!(chroma2.is_reliable);
    }
}
