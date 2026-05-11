pub mod audio;
pub mod comparison;
pub mod config;
pub mod db;
pub mod error;
pub mod ffmpeg;
pub mod phash;
pub mod scan;

pub use config::{FolderMatchMode, HardwareAccel, Settings};
pub use db::{Database, DuplicatePair, FileRecord, MatchMethod, ScanDatabase, SurrealDatabase};
pub use error::{VdfError, VdfResult};
pub use scan::ScanEngine;
