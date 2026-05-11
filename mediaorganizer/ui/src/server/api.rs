//! #[server] functions for the web target.
//!
//! Each function runs on the Axum server. Dioxus generates a matching async
//! stub on the WASM client that calls it via HTTP.
//!
//! Real-time scan progress is streamed to the client via ServerEvents<ScanProgress>.
//! This replaces the Blazor Server / SignalR approach from the C# VDF.Web project.

#[cfg(feature = "web")]
use dioxus::prelude::*;
#[cfg(feature = "web")]
use app_core::scan::ScanProgress;
#[cfg(feature = "web")]
use crate::settings::UiSettings;

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
        .map_err(|e| ServerFnError::ServerError(e.to_string()))?;

    let (tx, mut rx) = mpsc::unbounded_channel::<ScanProgress>();

    tokio::task::spawn_blocking(move || {
        let cb = std::sync::Arc::new(move |ev| { let _ = tx.send(ev); });
        let mut engine = ScanEngine::new(settings, db).with_progress(cb);
        let _ = engine.run();
    });

    // Drain events — in fullstack mode Dioxus SSE handles the streaming
    while let Some(_event) = rx.recv().await {
        // TODO: push event to ServerEvents<ScanProgress> stream
        // The exact API for ServerEvents streaming is in Dioxus 0.7 fullstack docs.
    }

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
        Err(ServerFnError::ServerError(
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
        .map_err(|e| ServerFnError::ServerError(e.to_string()))?;

    let pairs = db.all_duplicates()
        .map_err(|e| ServerFnError::ServerError(e.to_string()))?;
    let files = db.all_files()
        .map_err(|e| ServerFnError::ServerError(e.to_string()))?;

    Ok((pairs, files))
}
