//! Scan view: folder management, scan controls, live progress + log.

use dioxus::prelude::*;
#[cfg(feature = "server")]
use app_core::scan::ScanProgress;

use crate::app::Route;
use crate::settings::UiSettings;
use crate::state::{AppState, ScanState};
use crate::state::scan_state::LogLevel;


#[component]
pub fn ScanView() -> Element {
    let mut scan_state = use_context::<Signal<ScanState>>();
    let app_state  = use_context::<Signal<AppState>>();

    let is_scanning = scan_state.read().is_scanning;

    rsx! {
        div { class: "view scan-view",
            h1 { "Scan" }

            // ── FFmpeg status banner ───────────────────────────────────────
            FfmpegBanner {}

            // ── Folder list ───────────────────────────────────────────────
            section { class: "folders",
                h2 { "Include Folders" }
                ul {
                    for dir in scan_state.read().settings.include_dirs.clone() {
                        li { class: "folder-entry",
                            span { "{dir}" }
                            button {
                                class: "btn-icon btn-remove",
                                onclick: move |_| {
                                    scan_state.write().settings.include_dirs.retain(|d| d != &dir);
                                },
                                "✕"
                            }
                        }
                    }
                }
                AddFolderButton { scan_state }
            }

            section { class: "folders",
                h2 { "Exclude Folders" }
                ul {
                    for dir in scan_state.read().settings.exclude_dirs.clone() {
                        li { class: "folder-entry",
                            span { "{dir}" }
                            button {
                                class: "btn-icon btn-remove",
                                onclick: move |_| {
                                    scan_state.write().settings.exclude_dirs.retain(|d| d != &dir);
                                },
                                "✕"
                            }
                        }
                    }
                }
                AddFolderButton { scan_state, exclude: true }
            }

            // ── Scan controls ─────────────────────────────────────────────
            section { class: "scan-controls",
                if is_scanning {
                    // Stop button — sets the cancel flag in the shared AtomicBool
                    button {
                        class: "btn btn-danger",
                        onclick: move |_| {
                            scan_state.read().request_cancel();
                        },
                        "Stop"
                    }

                    // Pause / Resume toggle
                    {
                        let is_paused = scan_state.read().is_paused;
                        rsx! {
                            button {
                                class: if is_paused { "btn btn-warning" } else { "btn btn-outline" },
                                onclick: move |_| {
                                    let paused = !scan_state.read().is_paused;
                                    scan_state.write().set_paused(paused);
                                },
                                if is_paused { "Resume" } else { "Pause" }
                            }
                        }
                    }
                } else {
                    button {
                        class: "btn btn-primary",
                        disabled: scan_state.read().settings.include_dirs.is_empty(),
                        onclick: move |_| {
                            #[cfg(feature = "server")]
                            {
                                let ui_settings = scan_state.read().settings.clone();
                                spawn(async move {
                                    run_scan(scan_state, app_state, ui_settings).await;
                                });
                            }
                        },
                        "Start Scan"
                    }
                }
            }

            // ── Progress ──────────────────────────────────────────────────
            if is_scanning || scan_state.read().progress > 0.0 {
                section { class: "scan-progress",
                    ProgressBar { value: scan_state.read().progress }
                    p { class: "progress-detail",
                        {
                            let s = scan_state.read();
                            if s.total_files > 0 && s.files_hashed < s.total_files {
                                format!("Hashing {}/{} files  ·  {} duplicates found",
                                    s.files_hashed, s.total_files, s.duplicates_found)
                            } else {
                                format!("{} files  ·  {} duplicates found",
                                    s.files_found, s.duplicates_found)
                            }
                        }
                    }
                }
            }

            // ── Live log ──────────────────────────────────────────────────
            section { class: "live-log",
                h2 { "Log" }
                div { class: "log-scroll",
                    for entry in scan_state.read().log_entries.iter().rev() {
                        p {
                            class: match entry.level {
                                LogLevel::Info  => "log-info",
                                LogLevel::Warn  => "log-warn",
                                LogLevel::Error => "log-error",
                            },
                            "{entry.message}"
                        }
                    }
                }
            }

            // ── Navigate to results ───────────────────────────────────────
            if !app_state.read().clusters.is_empty() {
                Link { to: Route::ResultsView {},
                    button { class: "btn btn-secondary",
                        "View {app_state.read().clusters.len()} duplicate groups →"
                    }
                }
            }
        }
    }
}

// ── FFmpeg status banner ──────────────────────────────────────────────────────

/// Shows a warning banner if FFmpeg is not available on PATH.
///
/// Checks once on mount via `get_ffmpeg_status()` server function and caches
/// the result in a local signal. "ready" → no banner; anything else → warning.
#[component]
fn FfmpegBanner() -> Element {
    let mut status = use_signal(|| "checking".to_string());

    use_effect(move || {
        spawn(async move {
            #[cfg(all(feature = "server", feature = "web"))]
            {
                use crate::server::ffmpeg_setup::{ffmpeg_status, FfmpegStatus, install_instructions};
                let s = match ffmpeg_status() {
                    Some(FfmpegStatus::Ready) | None => "ready".to_string(),
                    Some(FfmpegStatus::MissingFfprobe { ffmpeg_path }) =>
                        format!("FFprobe not found (ffmpeg at {}). Metadata reading and SSIM will not work.", ffmpeg_path.display()),
                    Some(FfmpegStatus::MissingFfmpeg { ffprobe_path }) =>
                        format!("FFmpeg not found (ffprobe at {}). Video scanning will not work.", ffprobe_path.display()),
                    Some(FfmpegStatus::Missing) =>
                        format!("FFmpeg and FFprobe not found on PATH.\n{}", install_instructions()),
                };
                status.set(s);
            }
            #[cfg(not(all(feature = "server", feature = "web")))]
            { status.set("ready".to_string()); }
        });
    });

    let msg = status.read();
    if msg.as_str() == "ready" || msg.as_str() == "checking" {
        return rsx! {};
    }

    rsx! {
        div { class: "ffmpeg-banner banner-warn",
            span { class: "banner-icon", "⚠" }
            div { class: "banner-body",
                strong { "FFmpeg not available" }
                pre { class: "banner-msg", "{msg}" }
                a {
                    class: "banner-link",
                    href: "https://ffmpeg.org/download.html",
                    target: "_blank",
                    "FFmpeg download page →"
                }
            }
        }
    }
}

// ── Add folder button — desktop uses native file picker, web uses text input ──

#[component]
fn AddFolderButton(
    mut scan_state: Signal<ScanState>,
    #[props(default = false)] exclude: bool,
) -> Element {
    let mut input_path = use_signal(String::new);
    let mut show_input = use_signal(|| false);

    if *show_input.read() {
        rsx! {
            div { class: "add-folder-inline",
                input {
                    r#type: "text",
                    placeholder: "/path/to/folder",
                    value: "{input_path}",
                    oninput: move |e| input_path.set(e.value().clone()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            let path = camino::Utf8PathBuf::from(input_path.read().trim());
                            if !path.as_str().is_empty() {
                                if exclude {
                                    scan_state.write().settings.exclude_dirs.push(path);
                                } else {
                                    scan_state.write().settings.include_dirs.push(path);
                                }
                            }
                            input_path.set(String::new());
                            show_input.set(false);
                        }
                        if e.key() == Key::Escape {
                            show_input.set(false);
                        }
                    },
                }
                button {
                    class: "btn btn-sm",
                    onclick: move |_| {
                        let path = camino::Utf8PathBuf::from(input_path.read().trim());
                        if !path.as_str().is_empty() {
                            if exclude {
                                scan_state.write().settings.exclude_dirs.push(path);
                            } else {
                                scan_state.write().settings.include_dirs.push(path);
                            }
                        }
                        input_path.set(String::new());
                        show_input.set(false);
                    },
                    "Add"
                }
                button {
                    class: "btn btn-sm btn-ghost",
                    onclick: move |_| show_input.set(false),
                    "Cancel"
                }
            }
        }
    } else {
        rsx! {
            button {
                class: "btn btn-sm btn-outline",
                onclick: move |_| show_input.set(true),
                "+ Add Folder"
            }
        }
    }
}

// ── Progress bar ──────────────────────────────────────────────────────────────

#[component]
fn ProgressBar(value: f32) -> Element {
    let pct = (value * 100.0).clamp(0.0, 100.0);
    rsx! {
        div { class: "progress-bar",
            div {
                class: "progress-fill",
                style: "width: {pct:.1}%",
            }
            span { class: "progress-label", "{pct:.0}%" }
        }
    }
}

// ── Scan execution ────────────────────────────────────────────────────────────

/// Run the full scan in a background thread, streaming ScanProgress events back
/// to the UI via a tokio channel.
///
/// On the web target this becomes a #[server] call and uses SSE; the async
/// bridge is the same from the component's perspective.
#[cfg(feature = "server")]
async fn run_scan(
    mut scan_state: Signal<ScanState>,
    mut app_state: Signal<AppState>,
    ui_settings: UiSettings,
) {
    use tokio::sync::mpsc;
    use app_core::db::{Database, ScanDatabase};
    use app_core::scan::ScanEngine;

    let settings: app_core::config::Settings = ui_settings.into();

    scan_state.write().reset();
    scan_state.write().is_scanning = true;

    let (tx, mut rx) = mpsc::unbounded_channel::<ScanProgress>();

    // Open the database
    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let db = match ScanDatabase::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            scan_state.write().push_log(LogLevel::Error, format!("DB open failed: {e}"));
            scan_state.write().is_scanning = false;
            return;
        }
    };

    // Wire cancel/pause flags from ScanState into the scan engine
    let cancel = std::sync::Arc::clone(&scan_state.read().cancel_flag);
    let pause  = std::sync::Arc::clone(&scan_state.read().pause_flag);

    // Run scan on a blocking thread so async runtime stays responsive
    let handle = tokio::task::spawn_blocking(move || {
        let cb = std::sync::Arc::new(move |ev| { let _ = tx.send(ev); });
        let mut engine = ScanEngine::new(settings, db).with_progress(cb);
        engine.cancel = cancel;
        engine.pause  = pause;
        engine.run()
    });

    // Drain progress events while the scan runs
    while let Some(event) = rx.recv().await {
        match event {
            ScanProgress::FileDiscovered { path } => {
                let mut s = scan_state.write();
                s.files_found += 1;
                s.push_log(LogLevel::Info, format!("found   {path}"));
            }
            ScanProgress::DiscoveryComplete { total } => {
                let mut s = scan_state.write();
                s.total_files = total;
                s.push_log(LogLevel::Info, format!("discovery done — {total} files"));
            }
            ScanProgress::FileHashed { path, .. } => {
                let mut s = scan_state.write();
                s.files_hashed += 1;
                if s.total_files > 0 {
                    s.progress = (s.files_hashed as f32 / s.total_files as f32) * 0.7;
                }
                let hashed = s.files_hashed;
                let total  = s.total_files;
                s.push_log(LogLevel::Info, format!("hashed  [{hashed}/{total}]  {path}"));
            }
            ScanProgress::ComparisonStarted { total_pairs } => {
                scan_state.write().push_log(
                    LogLevel::Info,
                    format!("comparing {total_pairs} pairs…"),
                );
            }
            ScanProgress::DuplicateFound { file_a, file_b, similarity } => {
                let mut s = scan_state.write();
                s.duplicates_found += 1;
                s.push_log(
                    LogLevel::Info,
                    format!("MATCH  {:.1}%  {file_a}  ↔  {file_b}", similarity * 100.0),
                );
            }
            ScanProgress::ScanComplete { files, duplicates } => {
                let mut s = scan_state.write();
                s.progress = 1.0;
                s.push_log(
                    LogLevel::Info,
                    format!("done — {files} files, {duplicates} duplicate groups"),
                );
                crate::notify::notify_desktop(
                    "Scan complete",
                    &format!("{files} files scanned, {duplicates} duplicate groups found"),
                );
            }
            ScanProgress::ScanAborted => {
                scan_state.write().push_log(LogLevel::Warn, "Scan stopped by user");
            }
            ScanProgress::Error { path, msg } => {
                scan_state.write().push_log(LogLevel::Error, format!("error {path}: {msg}"));
            }
        }
    }

    // Collect results from the finished scan
    match handle.await {
        Ok(Ok(())) => {
            // Re-open DB to read results (scan engine consumed it)
            if let Ok(db) = ScanDatabase::open(&db_path) {
                let pairs = db.all_duplicates().unwrap_or_default();
                let files = db.all_files().unwrap_or_default();
                app_state.write().load_clusters(pairs, files);
            }
        }
        Ok(Err(e)) => {
            scan_state.write().push_log(LogLevel::Error, format!("scan failed: {e}"));
        }
        Err(e) => {
            scan_state.write().push_log(LogLevel::Error, format!("task panic: {e}"));
        }
    }

    scan_state.write().is_scanning = false;
}
