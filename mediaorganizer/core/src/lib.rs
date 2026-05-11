pub mod audio;
pub mod comparison;
pub mod config;
pub mod db;
pub mod error;
pub mod ffmpeg;
pub mod phash;
pub mod scan;

pub use config::{FolderMatchMode, HardwareAccel, Settings};
pub use db::{
    BlacklistEntry, ContainerInfo, Database, DuplicatePair, FileRecord, FileFlags, Fingerprints,
    IframeFingerprint, LocationRecord, MatchMethod, MediaType, PhashFingerprint,
    ScanDatabase, ScanJob, SurrealDatabase, UserTag,
};
pub use error::{VdfError, VdfResult};
pub use scan::ScanEngine;
