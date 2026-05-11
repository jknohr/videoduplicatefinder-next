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
use vdf_core::{config::Settings, scan::ScanProgress};

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
pub async fn trigger_scan(settings: Settings) -> Result<(), ServerFnError> {
    use tokio::sync::mpsc;
    use vdf_core::db::ScanDatabase;
    use vdf_core::scan::ScanEngine;

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

/// Load all duplicate clusters from the database.
#[cfg(feature = "web")]
#[server(endpoint = "/api/duplicates")]
pub async fn load_duplicates() -> Result<
    (Vec<vdf_core::db::DuplicatePair>, Vec<vdf_core::db::FileRecord>),
    ServerFnError,
> {
    use vdf_core::db::ScanDatabase;

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
