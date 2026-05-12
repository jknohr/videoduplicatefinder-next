//! Reactive scan progress state shared across all components.

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use crate::settings::UiSettings;

/// One entry in the live log panel.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Global scan progress state.
///
/// Provided at the App root via `use_context_provider(|| Signal::new(ScanState::default()))`.
/// Components read and write it via `use_context::<Signal<ScanState>>()`.
#[derive(Debug, Clone)]
pub struct ScanState {
    pub settings: UiSettings,
    pub is_scanning: bool,
    pub is_paused: bool,
    /// 0.0 = not started, 1.0 = complete.
    pub progress: f32,
    /// Total files discovered (set when discovery phase completes).
    pub total_files: usize,
    /// Files hashed so far.
    pub files_hashed: usize,
    /// Total files discovered (alias kept for display).
    pub files_found: usize,
    /// Duplicate pairs found so far.
    pub duplicates_found: usize,
    /// Capped ring buffer of log entries shown in the live log panel.
    pub log_entries: Vec<LogEntry>,
    /// Shared with the ScanEngine — set to true to stop scanning.
    pub cancel_flag: Arc<AtomicBool>,
    /// Shared with the ScanEngine — set to true to pause, false to resume.
    pub pause_flag: Arc<AtomicBool>,
}

impl Default for ScanState {
    fn default() -> Self {
        Self {
            settings: load_saved_settings().unwrap_or_default(),
            is_scanning: false,
            is_paused: false,
            progress: 0.0,
            total_files: 0,
            files_hashed: 0,
            files_found: 0,
            duplicates_found: 0,
            log_entries: Vec::new(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            pause_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Load persisted settings from `<config_local_dir>/vdf/settings.json`.
/// Returns `None` if the file is missing or unparseable.
fn load_saved_settings() -> Option<UiSettings> {
    let path = dirs::config_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("settings.json");
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

impl ScanState {
    const MAX_LOG: usize = 500;

    pub fn push_log(&mut self, level: LogLevel, message: impl Into<String>) {
        if self.log_entries.len() >= Self::MAX_LOG {
            self.log_entries.remove(0);
        }
        self.log_entries.push(LogEntry { level, message: message.into() });
    }

    pub fn reset(&mut self) {
        self.is_scanning = false;
        self.is_paused = false;
        self.progress = 0.0;
        self.total_files = 0;
        self.files_hashed = 0;
        self.files_found = 0;
        self.duplicates_found = 0;
        self.log_entries.clear();
        self.cancel_flag.store(false, Ordering::Relaxed);
        self.pause_flag.store(false, Ordering::Relaxed);
    }

    pub fn request_cancel(&self) {
        self.cancel_flag.store(true, Ordering::Relaxed);
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.is_paused = paused;
        self.pause_flag.store(paused, Ordering::Relaxed);
    }
}
