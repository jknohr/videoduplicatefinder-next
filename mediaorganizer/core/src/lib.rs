pub mod audio;
pub mod blacklist;
pub mod comparison;
pub mod config;
pub mod db;
pub mod error;
pub mod ffmpeg;
pub mod hardlink;
pub mod mpeg7;
pub mod phash;
pub mod ranker;
pub mod scan;
pub mod thumbnail;
pub mod utils;

pub use config::{FolderMatchMode, HardwareAccel, Settings};
pub use db::{
    BlacklistEntry, ContainerInfo, Database, DuplicatePair, FileRecord, FileFlags, Fingerprints,
    IframeFingerprint, LocationRecord, MatchMethod, MediaType, PhashFingerprint,
    ScanDatabase, ScanJob, SurrealDatabase, UserTag,
};
pub use error::{VdfError, VdfResult};
pub use blacklist::{Blacklist, BlacklistEntry as BlacklistGroup, compute_blacklisted_ids, load as load_blacklist, paths_equal, prune_missing, save as save_blacklist};
pub use ranker::{BestFlags, Criterion, compute_best_flags, default_criteria, pick_keeper};
pub use scan::ScanEngine;
pub use ffmpeg::{
    which_ffmpeg, which_ffprobe, long_path_fix,
    extract_thumbnail_jpeg, extract_temporal_average_hash,
    compute_ssim_at_offset, get_scene_change_timestamps, read_metadata_tags, write_metadata_tags,
    MediaInfo,
};
pub use utils::{
    state_folder, settings_folder, resolve_database_folder,
    move_to_trash, is_on_same_filesystem,
    format_duration, bytes_to_string,
    can_write_to_directory, is_running_in_container,
};
pub use thumbnail::{build_composite, try_write_joined_jpeg, decode_jpeg, jpeg_encode};
