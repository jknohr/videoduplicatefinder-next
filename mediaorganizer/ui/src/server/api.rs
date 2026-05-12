//! #[server] functions for the web target.
//!
//! Each function runs on the Axum server. Dioxus generates a matching async
//! stub on the WASM client that calls it via HTTP.
//!
//! Real-time scan progress is streamed to the client via ServerEvents<ScanProgress>.
//! This replaces the Blazor Server / SignalR approach from the C# VDF.Web project.
//!
//! Raw Axum handlers (video streaming, HTTP 206) live at the bottom of this file
//! and are registered in mod.rs via `register_axum_routes()`.

#[cfg(feature = "web")]
use dioxus::prelude::*;
#[cfg(feature = "web")]
use app_core::scan::ScanProgress;
#[cfg(feature = "web")]
use crate::settings::UiSettings;

// ---------------------------------------------------------------------------
// Server-side scan control — one active scan per process.
// The cancel and pause AtomicBool flags are shared with the ScanEngine running
// in spawn_blocking. The client calls cancel_scan() / set_scan_paused() to
// toggle them without needing to share state across the WASM boundary.
// ---------------------------------------------------------------------------

#[cfg(feature = "server")]
mod scan_control {
    use std::sync::{Arc, OnceLock, atomic::{AtomicBool, Ordering}};

    static CANCEL: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    static PAUSE:  OnceLock<Arc<AtomicBool>> = OnceLock::new();

    fn cancel_flag() -> &'static Arc<AtomicBool> {
        CANCEL.get_or_init(|| Arc::new(AtomicBool::new(false)))
    }
    fn pause_flag() -> &'static Arc<AtomicBool> {
        PAUSE.get_or_init(|| Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel() { cancel_flag().store(true, Ordering::Relaxed); }
    pub fn set_pause(paused: bool) { pause_flag().store(paused, Ordering::Relaxed); }
    pub fn reset() {
        cancel_flag().store(false, Ordering::Relaxed);
        pause_flag().store(false, Ordering::Relaxed);
    }
    pub fn cancel_arc() -> Arc<AtomicBool> { Arc::clone(cancel_flag()) }
    pub fn pause_arc()  -> Arc<AtomicBool> { Arc::clone(pause_flag()) }
}

// ---------------------------------------------------------------------------
// #[server] RPC functions
// ---------------------------------------------------------------------------

/// Trigger a scan on the server. Streams ScanProgress events back to the client.
///
/// Client usage (inside a component):
/// ```rust
/// spawn(async {
///     let mut stream = trigger_scan(settings).await.unwrap();
///     while let Some(event) = stream.next().await {
///         // update scan_state signal
///     }
/// });
/// ```
#[cfg(feature = "web")]
#[server(endpoint = "/api/scan")]
pub async fn trigger_scan(ui_settings: UiSettings) -> Result<(), ServerFnError> {
    use tokio::sync::mpsc;
    use app_core::db::{Database, ScanDatabase};
    use app_core::scan::ScanEngine;

    let settings: app_core::config::Settings = ui_settings.into();

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let db = ScanDatabase::open(&db_path)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let (tx, mut rx) = mpsc::unbounded_channel::<ScanProgress>();

    // Reset and wire global cancel/pause flags into this scan engine
    scan_control::reset();
    let cancel = scan_control::cancel_arc();
    let pause  = scan_control::pause_arc();

    tokio::task::spawn_blocking(move || {
        let cb = std::sync::Arc::new(move |ev| { let _ = tx.send(ev); });
        let mut engine = ScanEngine::new(settings, db).with_progress(cb);
        engine.cancel = cancel;
        engine.pause  = pause;
        let _ = engine.run();
    });

    // Drain events — in fullstack mode Dioxus SSE handles the streaming
    while let Some(_event) = rx.recv().await {
        // TODO: push event to ServerEvents<ScanProgress> stream
    }

    Ok(())
}

/// Request cancellation of the active scan. Safe to call even when no scan is running.
#[cfg(feature = "web")]
#[server(endpoint = "/api/cancel_scan")]
pub async fn cancel_scan() -> Result<(), ServerFnError> {
    scan_control::cancel();
    Ok(())
}

/// Pause or resume the active scan.
#[cfg(feature = "web")]
#[server(endpoint = "/api/pause_scan")]
pub async fn set_scan_paused(paused: bool) -> Result<(), ServerFnError> {
    scan_control::set_pause(paused);
    Ok(())
}

/// Read container-level metadata tags for a media file (ffprobe).
#[cfg(feature = "web")]
#[server(endpoint = "/api/read_tags")]
pub async fn read_tags(path: String) -> Result<std::collections::HashMap<String, String>, ServerFnError> {
    use camino::Utf8Path;
    let tags = app_core::read_metadata_tags(Utf8Path::new(&path));
    Ok(tags)
}

/// Write container-level metadata tags to a media file (ffmpeg -c copy, atomic rename).
#[cfg(feature = "web")]
#[server(endpoint = "/api/write_tags")]
pub async fn write_tags(
    path: String,
    tags: std::collections::HashMap<String, String>,
) -> Result<(), ServerFnError> {
    use camino::Utf8Path;
    let (ok, err) = app_core::write_metadata_tags(Utf8Path::new(&path), &tags);
    if ok {
        Ok(())
    } else {
        Err(ServerFnError::new(
            err.unwrap_or_else(|| "write_tags failed".to_string())
        ))
    }
}

/// Load all duplicate clusters from the database.
#[cfg(feature = "web")]
#[server(endpoint = "/api/duplicates")]
pub async fn load_duplicates() -> Result<
    (Vec<app_core::db::DuplicatePair>, Vec<app_core::db::FileRecord>),
    ServerFnError,
> {
    use app_core::db::{Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let db = ScanDatabase::open(&db_path)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let pairs = db.all_duplicates()
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let files = db.all_files()
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok((pairs, files))
}

/// Delete a file from the database and optionally from disk.
#[cfg(feature = "web")]
#[server(endpoint = "/api/delete_file")]
pub async fn delete_file(file_id: String, from_disk: bool) -> Result<(), ServerFnError> {
    use app_core::db::{Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let mut db = ScanDatabase::open(&db_path)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    if from_disk {
        let record = db.get_file(&file_id)
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        if let Some(rec) = record {
            if let Err(e) = std::fs::remove_file(&rec.path) {
                return Err(ServerFnError::new(
                    format!("failed to delete {}: {e}", rec.path)
                ));
            }
        }
    }

    db.delete_file(&file_id)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

/// Remove the duplicate_of edge between two files (mark pair as not a match).
#[cfg(feature = "web")]
#[server(endpoint = "/api/remove_pair")]
pub async fn remove_duplicate_pair(file_a: String, file_b: String) -> Result<(), ServerFnError> {
    use app_core::db::{Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let mut db = ScanDatabase::open(&db_path)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    db.remove_duplicate_pair(&file_a, &file_b)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Raw Axum handlers — video streaming with HTTP 206 / Range support
// ---------------------------------------------------------------------------

/// Query parameters for GET /api/video?path=...
#[cfg(feature = "web")]
#[derive(serde::Deserialize)]
pub struct VideoQuery {
    pub path: String,
}

/// Stream a video file with HTTP 206 Partial Content support.
///
/// The browser `<video>` element issues Range requests to seek without
/// downloading the entire file. This handler:
///   1. Validates the absolute file path (never follows relative paths).
///   2. Parses `Range: bytes=start-end` (RFC 7233).
///   3. Returns 206 with the requested byte range.
///   4. Falls back to 200 full response when no Range header is present.
#[cfg(feature = "web")]
pub async fn video_stream_handler(
    axum::extract::Query(params): axum::extract::Query<VideoQuery>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{header, StatusCode};
    use axum::response::Response;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let path = std::path::Path::new(&params.path);

    if !path.is_absolute() {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from("path must be absolute"))
            .unwrap();
    }

    let meta = match tokio::fs::metadata(path).await {
        Ok(m) if m.is_file() => m,
        _ => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("file not found"))
                .unwrap();
        }
    };

    let file_size = meta.len();
    let content_type = mime_for_ext(path);

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_range(s, file_size));

    let (start, end) = match range {
        Some((s, e)) => (s, e),
        None => {
            let file = match tokio::fs::File::open(path).await {
                Ok(f) => f,
                Err(_) => {
                    return Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::from("cannot open file"))
                        .unwrap();
                }
            };
            let stream = tokio_util::io::ReaderStream::new(file);
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CONTENT_LENGTH, file_size)
                .header(header::ACCEPT_RANGES, "bytes")
                .body(Body::from_stream(stream))
                .unwrap();
        }
    };

    let end = end.min(file_size.saturating_sub(1));
    if start > end || start >= file_size {
        return Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header("Content-Range", format!("bytes */{file_size}"))
            .body(Body::empty())
            .unwrap();
    }

    let chunk_len = end - start + 1;

    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("cannot open file"))
                .unwrap();
        }
    };

    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from("seek failed"))
            .unwrap();
    }

    let limited = file.take(chunk_len);
    let stream = tokio_util::io::ReaderStream::new(limited);

    Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, chunk_len)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{file_size}"),
        )
        .body(Body::from_stream(stream))
        .unwrap()
}

/// Parse `Range: bytes=start-end` or `bytes=start-` (RFC 7233).
#[cfg(feature = "web")]
fn parse_range(header: &str, file_size: u64) -> Option<(u64, u64)> {
    let s = header.strip_prefix("bytes=")?;
    let (start_str, end_str) = s.split_once('-')?;
    let start: u64 = start_str.parse().ok()?;
    let end: u64 = if end_str.is_empty() {
        file_size.saturating_sub(1)
    } else {
        end_str.parse().ok()?
    };
    Some((start, end))
}

// ---------------------------------------------------------------------------
// Thumbnail endpoint — GET /api/thumbnail?path=...&pos=...&w=...
// ---------------------------------------------------------------------------

/// Query parameters for GET /api/thumbnail
#[cfg(feature = "web")]
#[derive(serde::Deserialize)]
pub struct ThumbnailQuery {
    pub path: String,
    /// Position in seconds (default 0 = let ffmpeg pick the first decodable frame)
    #[serde(default)]
    pub pos: f64,
    /// Max width in pixels (default 200; 0 = full resolution)
    #[serde(default = "default_thumb_width")]
    pub w: u32,
}

#[cfg(feature = "web")]
fn default_thumb_width() -> u32 { 200 }

/// Extract and serve a single JPEG thumbnail from a video/image at the given position.
///
/// Uses `app_core::ffmpeg::extract_thumbnail_jpeg` which spawns ffmpeg as a subprocess.
/// Results are NOT cached — the Dioxus image element caches via browser HTTP cache headers.
#[cfg(feature = "web")]
pub async fn thumbnail_handler(
    axum::extract::Query(params): axum::extract::Query<ThumbnailQuery>,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{header, StatusCode};
    use axum::response::Response;

    let path_str = params.path.clone();
    let pos = params.pos;
    let w = params.w;

    // Validate path is absolute
    let path = std::path::Path::new(&path_str);
    if !path.is_absolute() {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from("path must be absolute"))
            .unwrap();
    }

    let jpeg = tokio::task::spawn_blocking(move || {
        use camino::Utf8Path;
        use app_core::config::HardwareAccel;
        app_core::ffmpeg::extract_thumbnail_jpeg(
            Utf8Path::new(&path_str),
            pos,
            w,
            HardwareAccel::None,
        )
    })
    .await
    .ok()
    .flatten();

    match jpeg {
        Some(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CONTENT_LENGTH, bytes.len())
            .header(header::CACHE_CONTROL, "public, max-age=3600")
            .body(Body::from(bytes))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("thumbnail extraction failed"))
            .unwrap(),
    }
}

/// Return a MIME content-type string for the given file path.
#[cfg(feature = "web")]
fn mime_for_ext(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("webm")              => "video/webm",
        Some("mkv")               => "video/x-matroska",
        Some("avi")               => "video/x-msvideo",
        Some("mov")               => "video/quicktime",
        Some("wmv")               => "video/x-ms-wmv",
        Some("flv")               => "video/x-flv",
        Some("ts")                => "video/mp2t",
        Some("ogv")               => "video/ogg",
        Some("3gp")               => "video/3gpp",
        _                         => "application/octet-stream",
    }
}
