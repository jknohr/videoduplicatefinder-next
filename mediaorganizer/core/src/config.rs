//! Application settings, persisted as JSON in the platform config directory.

use crate::error::{VdfError, VdfResult};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Folder-match constraint for pairwise comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FolderMatchMode {
    /// Compare all files regardless of folder (default).
    #[default]
    None,
    /// Only compare files in the same folder.
    SameFolderOnly,
    /// Only compare files in different folders.
    DifferentFolderOnly,
}

/// All user-configurable settings. New fields should carry `#[serde(default)]`
/// to load cleanly from older config files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub settings_version: u32,

    // --- Scan directories ---
    pub include_dirs: Vec<Utf8PathBuf>,
    pub exclude_dirs: Vec<Utf8PathBuf>,
    pub include_images: bool,
    pub include_sub_directories: bool,
    pub ignore_readonly_folders: bool,
    pub ignore_reparse_points: bool,
    pub exclude_hard_links: bool,

    // --- Similarity ---
    /// Minimum similarity (0.0–1.0) to report as a duplicate.
    pub min_similarity: f32,

    // --- Duration tolerance ---
    /// Percentage of video duration allowed to differ. 0 = disabled (use seconds only).
    /// Matches C# PercentDurationDifference.
    pub percent_duration_difference: f64,
    /// Floor for the percentage-derived tolerance (seconds). 0 = disabled.
    pub duration_diff_min_secs: f64,
    /// Ceiling for the percentage-derived tolerance (seconds). 0 = disabled.
    pub duration_diff_max_secs: f64,

    // --- Frame sampling ---
    pub skip_start_secs: f64,
    pub skip_start_percent: f32,
    pub skip_end_secs: f64,
    pub skip_end_percent: f32,
    pub max_sampling_duration_secs: f64,
    pub thumbnail_count: usize,

    // --- Comparison options ---
    pub use_phashing: bool,
    pub compare_horizontally_flipped: bool,
    pub ignore_black_pixels: bool,
    pub ignore_white_pixels: bool,
    pub folder_match_mode: FolderMatchMode,
    pub same_folder_depth: usize,
    pub scan_against_entire_database: bool,

    // --- File path filters ---
    pub filter_by_path_contains: bool,
    pub path_contains_texts: Vec<String>,
    pub filter_by_path_not_contains: bool,
    pub path_not_contains_texts: Vec<String>,
    pub filter_by_file_size: bool,
    pub min_file_size_bytes: u64,
    pub max_file_size_bytes: u64,

    // --- Logging ---
    pub log_excluded_files: bool,

    // --- I-frame timeline fingerprint ---
    pub iframe_fingerprint: bool,
    pub iframe_sample_interval_secs: f64,
    pub max_iframe_samples: usize,
    pub iframe_match_percent: f32,
    pub iframe_min_consecutive: usize,
    pub iframe_max_gap: usize,
    pub iframe_hash_threshold: f32,

    // --- Temporal average hash ---
    pub temporal_avg_hash: bool,
    pub temporal_avg_start_secs: f64,
    pub temporal_avg_window_secs: f64,

    // --- Scene-aware skip ---
    pub scene_aware_skip: bool,
    pub scene_detection_threshold: f32,
    pub scene_skip_count: usize,

    // --- MPEG-7 signature ---
    pub mpeg7_signature: bool,

    // --- SSIM verification ---
    pub ssim_verification: bool,
    pub ssim_verify_min_sim: f32,
    pub ssim_verify_max_sim: f32,
    pub ssim_reject_threshold: f32,
    pub ssim_window_secs: f64,

    // --- Audio / partial-clip ---
    pub partial_clip_detection: bool,
    pub partial_clip_min_ratio: f32,
    pub partial_clip_min_similarity: f32,

    // --- Performance ---
    pub parallelism: usize,
    pub hardware_accel: HardwareAccel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HardwareAccel {
    #[default]
    None,
    Vaapi,
    Cuda,
    VideoToolbox,
    D3d11va,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_version: 1,
            include_dirs: vec![],
            exclude_dirs: vec![],
            include_images: false,
            include_sub_directories: true,
            ignore_readonly_folders: false,
            ignore_reparse_points: false,
            exclude_hard_links: false,
            min_similarity: 0.96,
            percent_duration_difference: 20.0,
            duration_diff_min_secs: 0.0,
            duration_diff_max_secs: 0.0,
            skip_start_secs: 0.0,
            skip_start_percent: 0.0,
            skip_end_secs: 0.0,
            skip_end_percent: 0.0,
            max_sampling_duration_secs: 0.0,
            thumbnail_count: 5,
            use_phashing: true,
            compare_horizontally_flipped: false,
            ignore_black_pixels: false,
            ignore_white_pixels: false,
            folder_match_mode: FolderMatchMode::None,
            same_folder_depth: 1,
            scan_against_entire_database: false,
            filter_by_path_contains: false,
            path_contains_texts: vec![],
            filter_by_path_not_contains: false,
            path_not_contains_texts: vec![],
            filter_by_file_size: false,
            min_file_size_bytes: 0,
            max_file_size_bytes: 0,
            log_excluded_files: false,
            iframe_fingerprint: false,
            iframe_sample_interval_secs: 30.0,
            max_iframe_samples: 300,
            iframe_match_percent: 0.40,
            iframe_min_consecutive: 3,
            iframe_max_gap: 0,
            iframe_hash_threshold: 0.85,
            temporal_avg_hash: false,
            temporal_avg_start_secs: 120.0,
            temporal_avg_window_secs: 60.0,
            scene_aware_skip: false,
            scene_detection_threshold: 14.0,
            scene_skip_count: 1,
            mpeg7_signature: false,
            ssim_verification: false,
            ssim_verify_min_sim: 0.80,
            ssim_verify_max_sim: 0.95,
            ssim_reject_threshold: 0.90,
            ssim_window_secs: 10.0,
            partial_clip_detection: false,
            partial_clip_min_ratio: 0.10,
            partial_clip_min_similarity: 0.80,
            parallelism: num_cpus(),
            hardware_accel: HardwareAccel::None,
        }
    }
}

impl Settings {
    /// Allowed duration tolerance in seconds for a video of the given duration.
    ///
    /// Mirrors C# Settings.GetDurationToleranceSeconds exactly:
    /// - When `percent_duration_difference > 0`, tolerance = duration × percent / 100,
    ///   clamped to [duration_diff_min_secs, duration_diff_max_secs] when non-zero.
    /// - When `percent_duration_difference == 0`, tolerance comes from the seconds
    ///   bounds only (flat tolerance mode).
    pub fn duration_tolerance_secs(&self, duration_secs: f64) -> f64 {
        if self.percent_duration_difference > 0.0 {
            let mut tol = duration_secs * self.percent_duration_difference / 100.0;
            if self.duration_diff_min_secs > 0.0 {
                tol = tol.max(self.duration_diff_min_secs);
            }
            if self.duration_diff_max_secs > 0.0 {
                tol = tol.min(self.duration_diff_max_secs);
            }
            tol.max(0.0)
        } else {
            // Percent rule disabled: flat tolerance from seconds bounds
            self.duration_diff_min_secs
                .max(self.duration_diff_max_secs)
                .max(0.0)
        }
    }

    /// Load settings from the platform config directory. Returns defaults if
    /// the file doesn't exist.
    pub fn load() -> VdfResult<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let json = std::fs::read_to_string(&path)?;
        serde_json::from_str(&json).map_err(|e| VdfError::Config(e.to_string()))
    }

    /// Persist settings to the platform config directory.
    pub fn save(&self) -> VdfResult<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Compute effective skip-start in seconds given video duration.
    pub fn effective_skip_start(&self, duration_secs: f64) -> f64 {
        let by_pct = duration_secs * self.skip_start_percent as f64 / 100.0;
        self.skip_start_secs.max(by_pct)
    }

    /// Compute effective skip-end in seconds given video duration.
    pub fn effective_skip_end(&self, duration_secs: f64) -> f64 {
        let by_pct = duration_secs * self.skip_end_percent as f64 / 100.0;
        self.skip_end_secs.max(by_pct)
    }

    /// Compute the sample timestamps (seconds) for standard pHash sampling.
    pub fn sample_timestamps(&self, duration_secs: f64, n_samples: usize) -> Vec<f64> {
        let start = self.effective_skip_start(duration_secs);
        let end_offset = self.effective_skip_end(duration_secs);
        let effective_dur = if self.max_sampling_duration_secs > 0.0 {
            duration_secs.min(self.max_sampling_duration_secs)
        } else {
            duration_secs
        };
        let window = (effective_dur - start - end_offset).max(0.0);
        (0..n_samples)
            .map(|i| start + window * (i as f64 + 0.5) / n_samples as f64)
            .collect()
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("vdf")
        .join("settings.json")
}

fn num_cpus() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_roundtrip() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let s2: Settings = serde_json::from_str(&json).unwrap();
        assert!((s.min_similarity - s2.min_similarity).abs() < 1e-6);
    }

    #[test]
    fn skip_arithmetic() {
        let mut s = Settings::default();
        s.skip_start_secs = 30.0;
        s.skip_start_percent = 5.0; // 5% of 200s = 10s → max(30, 10) = 30
        assert_eq!(s.effective_skip_start(200.0), 30.0);

        s.skip_start_secs = 5.0;
        // 5% of 200 = 10s → max(5, 10) = 10
        assert_eq!(s.effective_skip_start(200.0), 10.0);
    }

    #[test]
    fn sample_timestamps_count() {
        let s = Settings::default();
        let ts = s.sample_timestamps(600.0, 5);
        assert_eq!(ts.len(), 5);
        assert!(ts[0] >= 0.0 && ts[4] <= 600.0);
    }
}
