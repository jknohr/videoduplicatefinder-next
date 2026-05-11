pub mod audio;
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

pub use config::{FolderMatchMode, HardwareAccel, Settings};
pub use db::{
    BlacklistEntry, ContainerInfo, Database, DuplicatePair, FileRecord, FileFlags, Fingerprints,
    IframeFingerprint, LocationRecord, MatchMethod, MediaType, PhashFingerprint,
    ScanDatabase, ScanJob, SurrealDatabase, UserTag,
};
pub use error::{VdfError, VdfResult};
pub use ranker::{BestFlags, Criterion, compute_best_flags, default_criteria, pick_keeper};
pub use scan::ScanEngine;
