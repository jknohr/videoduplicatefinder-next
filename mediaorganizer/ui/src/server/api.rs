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

/// Return all blacklisted file pairs from the database.
#[cfg(feature = "web")]
#[server(endpoint = "/api/blacklist")]
pub async fn get_blacklist() -> Result<Vec<(String, String, u64, Option<String>)>, ServerFnError> {
    use app_core::db::{Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf").join("db");

    let db = ScanDatabase::open(&db_path)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let entries = db.all_blacklisted()
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(entries.into_iter()
        .map(|e| (e.file_a, e.file_b, e.added_at, e.reason))
        .collect())
}

/// Add a file pair to the blacklist (mark as "not a match").
#[cfg(feature = "web")]
#[server(endpoint = "/api/blacklist_add")]
pub async fn add_to_blacklist(
    file_a: String,
    file_b: String,
    reason: Option<String>,
) -> Result<(), ServerFnError> {
    use app_core::db::{BlacklistEntry, Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf").join("db");

    let mut db = ScanDatabase::open(&db_path)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let entry = BlacklistEntry::new(file_a, file_b, reason);
    db.add_blacklist(entry)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

/// Remove a file pair from the blacklist.
#[cfg(feature = "web")]
#[server(endpoint = "/api/blacklist_remove")]
pub async fn remove_from_blacklist(file_a: String, file_b: String) -> Result<(), ServerFnError> {
    use app_core::db::{Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf").join("db");

    let mut db = ScanDatabase::open(&db_path)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    db.remove_blacklist(&file_a, &file_b)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

/// Re-hash a single file and re-run comparisons against all DB files.
///
/// Port of the quick-rescan path from C# ScanEngine.
/// After completion, the caller should reload duplicates via `load_duplicates()`.
#[cfg(feature = "web")]
#[server(endpoint = "/api/rescan_file")]
pub async fn rescan_file(path: String) -> Result<(), ServerFnError> {
    use app_core::db::ScanDatabase;
    use app_core::scan::ScanEngine;

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf").join("db");

    tokio::task::spawn_blocking(move || {
        let db = ScanDatabase::open(&db_path)
            .map_err(|e| e.to_string())?;

        let settings = app_core::config::Settings::default();
        let mut engine = ScanEngine::new(settings, db);

        engine.rescan_file(camino::Utf8Path::new(&path))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?
    .map_err(ServerFnError::new)
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

// Diff-frame endpoint — GET /api/diff_frame?path_a=...&pos_a=...&path_b=...&pos_b=...&w=...
// ---------------------------------------------------------------------------

/// Query parameters for GET /api/diff_frame
#[cfg(feature = "web")]
#[derive(serde::Deserialize)]
pub struct DiffFrameQuery {
    pub path_a: String,
    #[serde(default)]
    pub pos_a: f64,
    pub path_b: String,
    #[serde(default)]
    pub pos_b: f64,
    #[serde(default = "default_thumb_width")]
    pub w: u32,
}

/// Render the absolute pixel difference between one frame from each video.
///
/// Returns a JPEG where bright pixels indicate large per-channel differences.
/// Ports the pixel-diff overlay mode from `ThumbnailComparerVM` in C# VDF.
#[cfg(feature = "web")]
pub async fn diff_frame_handler(
    axum::extract::Query(params): axum::extract::Query<DiffFrameQuery>,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{header, StatusCode};
    use axum::response::Response;

    // Path traversal guard
    let path_a = std::path::PathBuf::from(&params.path_a);
    let path_b = std::path::PathBuf::from(&params.path_b);
    for p in [&path_a, &path_b] {
        if p.components().any(|c| c == std::path::Component::ParentDir) {
            return Response::builder()
                .status(StatusCode::FORBIDDEN)
                .body(Body::empty())
                .unwrap();
        }
    }

    let jpeg = tokio::task::spawn_blocking(move || {
        let a = camino::Utf8Path::from_path(&path_a)?;
        let b = camino::Utf8Path::from_path(&path_b)?;
        app_core::ffmpeg::extract_diff_jpeg(a, params.pos_a, b, params.pos_b, params.w)
    })
    .await
    .ok()
    .flatten();

    match jpeg {
        Some(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CACHE_CONTROL, "public, max-age=3600, immutable")
            .body(Body::from(bytes))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("diff frame extraction failed"))
            .unwrap(),
    }
}

/// Returns a brief FFmpeg status string for the UI banner.
///
/// "ready" means both ffmpeg and ffprobe are on PATH.
/// Any other string describes what's missing.
#[cfg(feature = "web")]
#[server(endpoint = "/api/ffmpeg_status")]
pub async fn get_ffmpeg_status() -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        use crate::server::ffmpeg_setup::{ffmpeg_status, FfmpegStatus, install_instructions};
        let status = ffmpeg_status().map(|s| match s {
            FfmpegStatus::Ready => "ready".to_string(),
            FfmpegStatus::MissingFfprobe { ffmpeg_path } =>
                format!("missing_ffprobe|ffmpeg at {}|{}", ffmpeg_path.display(), install_instructions()),
            FfmpegStatus::MissingFfmpeg { ffprobe_path } =>
                format!("missing_ffmpeg|ffprobe at {}|{}", ffprobe_path.display(), install_instructions()),
            FfmpegStatus::Missing =>
                format!("missing||{}", install_instructions()),
        }).unwrap_or_else(|| "unknown".to_string());
        Ok(status)
    }
    #[cfg(not(feature = "server"))]
    { Ok("ready".to_string()) }
}

/// Remove all file records whose path no longer exists on disk.
/// Returns the number of entries pruned.
/// Mirrors `DatabaseUtils.CleanupDatabase()` from C# VDF.Core.
#[cfg(feature = "web")]
#[server(endpoint = "/api/cleanup_database")]
pub async fn cleanup_database() -> Result<usize, ServerFnError> {
    #[cfg(feature = "server")]
    {
        use app_core::db::{Database, ScanDatabase};
        let db_path = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("vdf").join("db");
        tokio::task::spawn_blocking(move || {
            let mut db = ScanDatabase::open(&db_path).map_err(|e| ServerFnError::new(e.to_string()))?;
            let removed = db.prune_missing_files().map_err(|e| ServerFnError::new(e.to_string()))?;
            Ok::<usize, ServerFnError>(removed)
        }).await.map_err(|e| ServerFnError::new(e.to_string()))?
    }
    #[cfg(not(feature = "server"))]
    { Ok(0) }
}

// ---------------------------------------------------------------------------
// Bulk file operations — port of FileUtils.CopyFile from C# VDF.Core
// ---------------------------------------------------------------------------

/// Copy a list of files (by DB id) to `dest_folder`.
/// Deconflicts filenames by appending _N suffixes.
/// Returns the number of errors (0 = all succeeded).
#[cfg(feature = "web")]
#[server(endpoint = "/api/bulk_copy")]
pub async fn bulk_copy_files(
    file_ids: Vec<String>,
    dest_folder: String,
) -> Result<u32, ServerFnError> {
    #[cfg(feature = "server")]
    {
        use app_core::db::{Database, ScanDatabase};
        let db_path = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("vdf").join("db");
        tokio::task::spawn_blocking(move || {
            let db = ScanDatabase::open(&db_path).map_err(|e| ServerFnError::new(e.to_string()))?;
            std::fs::create_dir_all(&dest_folder)
                .map_err(|e| ServerFnError::new(e.to_string()))?;
            let mut errors = 0u32;
            for id in &file_ids {
                let Ok(Some(rec)) = db.get_file(id) else { errors += 1; continue; };
                let src = std::path::Path::new(rec.path.as_str());
                if !src.exists() { errors += 1; continue; }
                let dest = deconflict_path(&dest_folder, src);
                if std::fs::copy(src, &dest).is_err() { errors += 1; }
            }
            Ok(errors)
        }).await.map_err(|e| ServerFnError::new(e.to_string()))?
    }
    #[cfg(not(feature = "server"))]
    { let _ = (file_ids, dest_folder); Ok(0) }
}

/// Move (rename) a list of files to `dest_folder` and update DB paths.
/// Returns the number of errors.
#[cfg(feature = "web")]
#[server(endpoint = "/api/bulk_move")]
pub async fn bulk_move_files(
    file_ids: Vec<String>,
    dest_folder: String,
) -> Result<u32, ServerFnError> {
    #[cfg(feature = "server")]
    {
        use app_core::db::{Database, ScanDatabase};
        let db_path = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("vdf").join("db");
        tokio::task::spawn_blocking(move || {
            let mut db = ScanDatabase::open(&db_path).map_err(|e| ServerFnError::new(e.to_string()))?;
            std::fs::create_dir_all(&dest_folder)
                .map_err(|e| ServerFnError::new(e.to_string()))?;
            let mut errors = 0u32;
            for id in &file_ids {
                let Ok(Some(rec)) = db.get_file(id) else { errors += 1; continue; };
                let src = std::path::Path::new(rec.path.as_str());
                if !src.exists() { errors += 1; continue; }
                let dest = deconflict_path(&dest_folder, src);
                if let Err(_) = std::fs::rename(src, &dest) {
                    // rename across filesystems fails; fall back to copy+delete
                    if std::fs::copy(src, &dest).is_ok() {
                        let _ = std::fs::remove_file(src);
                    } else {
                        errors += 1;
                        continue;
                    }
                }
                // Update DB path
                if let Ok(new_path) = camino::Utf8PathBuf::from_path_buf(dest) {
                    let mut updated = rec;
                    updated.name = new_path.file_name().unwrap_or("").to_string();
                    updated.path = new_path;
                    let _ = db.upsert_file(updated);
                }
            }
            Ok(errors)
        }).await.map_err(|e| ServerFnError::new(e.to_string()))?
    }
    #[cfg(not(feature = "server"))]
    { let _ = (file_ids, dest_folder); Ok(0) }
}

/// Create symbolic links in `dest_folder` pointing to the original files.
/// Ports `CreateSymbolLinksForCheckedItemsCommand` from C# MainWindowVM.
/// Returns the number of errors.
#[cfg(feature = "web")]
#[server(endpoint = "/api/bulk_symlink")]
pub async fn bulk_create_symlinks(
    file_ids: Vec<String>,
    dest_folder: String,
) -> Result<u32, ServerFnError> {
    #[cfg(feature = "server")]
    {
        use app_core::db::{Database, ScanDatabase};
        let db_path = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("vdf").join("db");
        tokio::task::spawn_blocking(move || {
            let db = ScanDatabase::open(&db_path).map_err(|e| ServerFnError::new(e.to_string()))?;
            std::fs::create_dir_all(&dest_folder)
                .map_err(|e| ServerFnError::new(e.to_string()))?;
            let mut errors = 0u32;
            for id in &file_ids {
                let Ok(Some(rec)) = db.get_file(id) else { errors += 1; continue; };
                let src = std::path::Path::new(rec.path.as_str());
                if !src.exists() { errors += 1; continue; }
                let link = deconflict_path(&dest_folder, src);
                #[cfg(unix)]
                let res = std::os::unix::fs::symlink(src, &link);
                #[cfg(windows)]
                let res = std::os::windows::fs::symlink_file(src, &link);
                #[cfg(not(any(unix, windows)))]
                let res: std::io::Result<()> = Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "symlinks not supported on this platform"));
                if res.is_err() { errors += 1; }
            }
            Ok(errors)
        }).await.map_err(|e| ServerFnError::new(e.to_string()))?
    }
    #[cfg(not(feature = "server"))]
    { let _ = (file_ids, dest_folder); Ok(0) }
}

// ---------------------------------------------------------------------------
// Cleanup dry-run report — port of BuildCleanupDryRunReport from C# MainWindowVM
// ---------------------------------------------------------------------------

/// DTO for a single item in a dry-run cleanup report.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DryRunItem {
    pub path: String,
    pub size_bytes: u64,
    pub resolution: String,
}

/// DTO for one duplicate group in a dry-run cleanup report.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DryRunGroup {
    pub remove: Vec<DryRunItem>,
    pub keep:   Vec<DryRunItem>,
    pub estimated_savings_bytes: u64,
}

/// DTO for the full dry-run cleanup report.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DryRunReport {
    pub created_at: String,
    pub total_savings_bytes: u64,
    pub groups: Vec<DryRunGroup>,
}

/// Build a dry-run report showing which files would be deleted if `to_remove_ids`
/// were deleted, and which files in each group would be kept.
///
/// Ports `ExportCheckedItemsCleanupDryRunReportCommand` from C# MainWindowVM.
#[cfg(feature = "web")]
#[server(endpoint = "/api/dry_run_report")]
pub async fn export_dry_run_report(
    to_remove_ids: Vec<String>,
) -> Result<DryRunReport, ServerFnError> {
    #[cfg(feature = "server")]
    {
        use app_core::db::{Database, ScanDatabase};
        let db_path = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("vdf").join("db");
        tokio::task::spawn_blocking(move || {
            let db = ScanDatabase::open(&db_path).map_err(|e| ServerFnError::new(e.to_string()))?;
            let all_pairs = db.all_duplicates().map_err(|e| ServerFnError::new(e.to_string()))?;
            let remove_set: std::collections::HashSet<&str> = to_remove_ids.iter().map(|s| s.as_str()).collect();

            // Group files by which duplicate cluster they belong to
            let mut cluster_map: std::collections::HashMap<String, std::collections::HashSet<String>> = std::collections::HashMap::new();
            for pair in &all_pairs {
                let key = {
                    let mut ids = vec![pair.file_a.clone(), pair.file_b.clone()];
                    ids.sort();
                    ids[0].clone()
                };
                cluster_map.entry(key.clone()).or_default().insert(pair.file_a.clone());
                cluster_map.entry(key).or_default().insert(pair.file_b.clone());
            }

            let mut groups: Vec<DryRunGroup> = Vec::new();
            let mut seen_remove: std::collections::HashSet<String> = std::collections::HashSet::new();

            for id in &to_remove_ids {
                if seen_remove.contains(id) { continue; }

                // Find which cluster this id belongs to
                let cluster_ids: Vec<String> = cluster_map.values()
                    .find(|set| set.contains(id.as_str()))
                    .map(|set| set.iter().cloned().collect())
                    .unwrap_or_else(|| vec![id.clone()]);

                let mut remove_items = Vec::new();
                let mut keep_items  = Vec::new();

                for cid in &cluster_ids {
                    if let Ok(Some(rec)) = db.get_file(cid) {
                        let resolution = if rec.width().unwrap_or(0) > 0 {
                            format!("{}×{}", rec.width().unwrap_or(0), rec.height().unwrap_or(0))
                        } else {
                            String::new()
                        };
                        let item = DryRunItem {
                            path: rec.path.to_string(),
                            size_bytes: rec.size_bytes,
                            resolution,
                        };
                        if remove_set.contains(cid.as_str()) {
                            seen_remove.insert(cid.clone());
                            remove_items.push(item);
                        } else {
                            keep_items.push(item);
                        }
                    }
                }

                if remove_items.is_empty() { continue; }
                let savings: u64 = remove_items.iter().map(|i| i.size_bytes).sum();
                groups.push(DryRunGroup { remove: remove_items, keep: keep_items, estimated_savings_bytes: savings });
            }

            let total_savings: u64 = groups.iter().map(|g| g.estimated_savings_bytes).sum();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            Ok(DryRunReport {
                created_at: format!("{now}"),
                total_savings_bytes: total_savings,
                groups,
            })
        }).await.map_err(|e| ServerFnError::new(e.to_string()))?
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = to_remove_ids;
        Ok(DryRunReport { created_at: String::new(), total_savings_bytes: 0, groups: vec![] })
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

/// Build a destination path, appending `_N` before the extension if a file
/// with the same name already exists at `dest_folder`.
/// Mirrors the deconflict loop in `FileUtils.CopyFile` from C# VDF.Core.
#[cfg(feature = "server")]
fn deconflict_path(dest_folder: &str, src: &std::path::Path) -> std::path::PathBuf {
    let file_name = src.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let mut dest = std::path::PathBuf::from(dest_folder).join(file_name);
    if !dest.exists() {
        return dest;
    }
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or(file_name);
    let ext  = src.extension().and_then(|e| e.to_str()).unwrap_or("");
    let mut n = 1u32;
    loop {
        let name = if ext.is_empty() { format!("{stem}_{n}") } else { format!("{stem}_{n}.{ext}") };
        dest = std::path::PathBuf::from(dest_folder).join(&name);
        if !dest.exists() { return dest; }
        n += 1;
    }
}
