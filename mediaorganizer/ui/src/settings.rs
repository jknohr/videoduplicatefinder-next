//! UI-local settings struct — always compiled, serializable for fullstack transport.
//!
//! On server feature: converts into core::config::Settings for the scan engine.
//! On WASM client: used directly; transmitted to the server via serde.

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

/// Mirror of core::config::FolderMatchMode — always compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FolderMatchMode {
    #[default]
    None,
    SameFolderOnly,
    DifferentFolderOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiSettings {
    pub include_dirs: Vec<Utf8PathBuf>,
    pub exclude_dirs: Vec<Utf8PathBuf>,
    pub min_similarity: f32,
    pub percent_duration_difference: f64,
    pub duration_diff_min_secs: f64,
    pub duration_diff_max_secs: f64,
    pub thumbnail_count: usize,
    pub iframe_fingerprint: bool,
    pub iframe_sample_interval_secs: f64,
    pub max_iframe_samples: usize,
    pub iframe_match_percent: f32,
    pub iframe_min_consecutive: usize,
    pub iframe_max_gap: usize,
    pub iframe_hash_threshold: f32,
    pub partial_clip_detection: bool,
    pub partial_clip_min_similarity: f32,
    pub skip_start_secs: f64,
    pub skip_end_secs: f64,
    pub include_images: bool,
    pub include_sub_directories: bool,
    pub folder_match_mode: FolderMatchMode,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            include_dirs: Vec::new(),
            exclude_dirs: Vec::new(),
            min_similarity: 0.95,
            percent_duration_difference: 20.0,
            duration_diff_min_secs: 0.0,
            duration_diff_max_secs: 0.0,
            thumbnail_count: 5,
            iframe_fingerprint: false,
            iframe_sample_interval_secs: 30.0,
            max_iframe_samples: 300,
            iframe_match_percent: 0.40,
            iframe_min_consecutive: 3,
            iframe_max_gap: 0,
            iframe_hash_threshold: 0.85,
            partial_clip_detection: false,
            partial_clip_min_similarity: 0.99,
            skip_start_secs: 0.0,
            skip_end_secs: 0.0,
            include_images: false,
            include_sub_directories: true,
            folder_match_mode: FolderMatchMode::None,
        }
    }
}

#[cfg(feature = "server")]
impl From<UiSettings> for core::config::Settings {
    fn from(s: UiSettings) -> Self {
        let mut c = core::config::Settings::default();
        c.include_dirs = s.include_dirs;
        c.exclude_dirs = s.exclude_dirs;
        c.min_similarity = s.min_similarity;
        c.percent_duration_difference = s.percent_duration_difference;
        c.duration_diff_min_secs = s.duration_diff_min_secs;
        c.duration_diff_max_secs = s.duration_diff_max_secs;
        c.thumbnail_count = s.thumbnail_count;
        c.iframe_fingerprint = s.iframe_fingerprint;
        c.iframe_sample_interval_secs = s.iframe_sample_interval_secs;
        c.max_iframe_samples = s.max_iframe_samples;
        c.iframe_match_percent = s.iframe_match_percent;
        c.iframe_min_consecutive = s.iframe_min_consecutive;
        c.iframe_max_gap = s.iframe_max_gap;
        c.iframe_hash_threshold = s.iframe_hash_threshold;
        c.partial_clip_detection = s.partial_clip_detection;
        c.partial_clip_min_similarity = s.partial_clip_min_similarity;
        c.skip_start_secs = s.skip_start_secs;
        c.skip_end_secs = s.skip_end_secs;
        c.include_images = s.include_images;
        c.include_sub_directories = s.include_sub_directories;
        c.folder_match_mode = match s.folder_match_mode {
            FolderMatchMode::None => core::config::FolderMatchMode::None,
            FolderMatchMode::SameFolderOnly => core::config::FolderMatchMode::SameFolderOnly,
            FolderMatchMode::DifferentFolderOnly => core::config::FolderMatchMode::DifferentFolderOnly,
        };
        c
    }
}
