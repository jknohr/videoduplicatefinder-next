use camino::Utf8PathBuf;
use thiserror::Error;

pub type VdfResult<T> = Result<T, VdfError>;

#[derive(Debug, Error)]
pub enum VdfError {
    #[error("FFmpeg error {code} processing '{path}': {msg}")]
    Ffmpeg {
        path: Utf8PathBuf,
        code: i32,
        msg: String,
    },

    #[error("FFmpeg error {code}: {msg}")]
    FfmpegGeneral { code: i32, msg: String },

    #[error("No video stream found in '{path}'")]
    NoVideoStream { path: Utf8PathBuf },

    #[error("No audio stream found in '{path}'")]
    NoAudioStream { path: Utf8PathBuf },

    #[error("Seek failed at {seek_secs:.2}s in '{path}'")]
    SeekFailed { path: Utf8PathBuf, seek_secs: f64 },

    #[error("Frame decode timeout in '{path}'")]
    DecodeTimeout { path: Utf8PathBuf },

    #[error("Database error: {0}")]
    Database(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Non-UTF-8 path skipped: {0}")]
    NonUtf8Path(std::path::PathBuf),

    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl From<serde_json::Error> for VdfError {
    fn from(e: serde_json::Error) -> Self {
        VdfError::Serialization(e.to_string())
    }
}
