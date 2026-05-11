//! Reactive scan progress state shared across all components.

use core::config::Settings;

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
    pub settings: Settings,
    pub is_scanning: bool,
    /// 0.0 = not started, 1.0 = complete.
    pub progress: f32,
    /// Total files discovered so far.
    pub files_found: usize,
    /// Duplicate pairs found so far.
    pub duplicates_found: usize,
    /// Capped ring buffer of log entries shown in the live log panel.
    pub log_entries: Vec<LogEntry>,
}

impl Default for ScanState {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            is_scanning: false,
            progress: 0.0,
            files_found: 0,
            duplicates_found: 0,
            log_entries: Vec::new(),
        }
    }
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
        self.progress = 0.0;
        self.files_found = 0;
        self.duplicates_found = 0;
        self.log_entries.clear();
    }
}
