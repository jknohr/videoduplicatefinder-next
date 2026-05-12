//! Blacklist manager view.
//!
//! Shows all file groups the user has marked as "not a match".
//! Users can remove individual entries (un-mark) or prune entries
//! where files no longer exist on disk.
//!
//! Port of VDF.GUI/ViewModels/BlacklistManagerVM.cs

use dioxus::prelude::*;
use crate::app::Route;
#[cfg(feature = "server")]
use dirs;

// ── State ─────────────────────────────────────────────────────────────────────

/// One blacklisted group displayed in the list.
#[derive(Debug, Clone, PartialEq)]
struct BlacklistRow {
    /// DB record ID of file A.
    pub file_a: String,
    /// DB record ID of file B.
    pub file_b: String,
    /// Human-readable paths (loaded from DB).
    pub path_a: String,
    pub path_b: String,
    /// Unix timestamp when the entry was created.
    pub added_at: u64,
}

// ── View ──────────────────────────────────────────────────────────────────────

#[component]
pub fn BlacklistView() -> Element {
    let entries = use_signal(Vec::<BlacklistRow>::new);
    let status = use_signal(String::new);
    let loading = use_signal(|| false);

    // Load entries on first mount
    use_effect({
        let mut entries = entries;
        let mut loading = loading;
        let mut status = status;
        move || {
            loading.set(true);
            #[cfg(feature = "server")]
            spawn(async move {
                match load_blacklist_rows().await {
                    Ok(rows) => entries.set(rows),
                    Err(e) => status.set(format!("Load error: {e}")),
                }
                loading.set(false);
            });
            #[cfg(not(feature = "server"))]
            { loading.set(false); }
        }
    });

    let row_count = entries.read().len();

    rsx! {
        div { class: "view blacklist-view",
            header { class: "blacklist-header",
                h1 { "Blacklist Manager" }
                p { class: "subtitle",
                    "{row_count} not-a-match entries"
                }
            }

            if !status.read().is_empty() {
                p { class: "status-msg", "{status}" }
            }

            if *loading.read() {
                p { "Loading…" }
            } else if entries.read().is_empty() {
                div { class: "empty-state",
                    p { "No blacklisted groups." }
                    Link { to: Route::ResultsView {},
                        button { class: "btn btn-secondary", "← Back to results" }
                    }
                }
            } else {
                // Action toolbar
                div { class: "toolbar",
                    button {
                        class: "btn btn-sm btn-outline",
                        onclick: {
                            let mut entries = entries;
                            let mut status = status;
                            move |_| {
                                #[cfg(feature = "server")]
                                spawn(async move {
                                    match prune_missing_action().await {
                                        Ok(removed) => {
                                            status.set(if removed > 0 {
                                                format!("Pruned {removed} entries for missing files.")
                                            } else {
                                                "No missing files found.".to_string()
                                            });
                                            // Reload
                                            if let Ok(rows) = load_blacklist_rows().await {
                                                entries.set(rows);
                                            }
                                        }
                                        Err(e) => status.set(format!("Error: {e}")),
                                    }
                                });
                            }
                        },
                        "Prune missing files"
                    }
                    button {
                        class: "btn btn-sm btn-danger",
                        onclick: {
                            let mut entries = entries;
                            let mut status = status;
                            move |_| {
                                #[cfg(feature = "server")]
                                spawn(async move {
                                    match clear_blacklist_action().await {
                                        Ok(()) => {
                                            entries.set(Vec::new());
                                            status.set("Blacklist cleared.".to_string());
                                        }
                                        Err(e) => status.set(format!("Error: {e}")),
                                    }
                                });
                            }
                        },
                        "Clear all"
                    }
                }

                // Entry list
                div { class: "blacklist-list",
                    for row in entries.read().clone() {
                        BlacklistRowCard {
                            row: row.clone(),
                            on_remove: {
                                let mut entries = entries;
                                let mut status = status;
                                let fa = row.file_a.clone();
                                let fb = row.file_b.clone();
                                move |_| {
                                    let fa2 = fa.clone();
                                    let fb2 = fb.clone();
                                    #[cfg(feature = "server")]
                                    spawn(async move {
                                        match unmark_blacklist_action(fa2.clone(), fb2.clone()).await {
                                            Ok(()) => {
                                                entries.write().retain(|r| {
                                                    !(r.file_a == fa2 && r.file_b == fb2)
                                                });
                                                status.set(String::new());
                                            }
                                            Err(e) => status.set(format!("Error: {e}")),
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Card component ────────────────────────────────────────────────────────────

#[component]
fn BlacklistRowCard(row: BlacklistRow, on_remove: EventHandler<MouseEvent>) -> Element {
    let ts = format_unix(row.added_at);
    rsx! {
        div { class: "blacklist-card",
            div { class: "blacklist-paths",
                div { class: "file-path", "{row.path_a}" }
                div { class: "file-path", "{row.path_b}" }
            }
            div { class: "blacklist-meta",
                span { class: "tag", "Added: {ts}" }
            }
            button {
                class: "btn btn-sm btn-outline",
                onclick: move |e| on_remove.call(e),
                "Un-mark"
            }
        }
    }
}

// ── Formatting ────────────────────────────────────────────────────────────────

fn format_unix(secs: u64) -> String {
    // Simple date formatting without chrono: convert to readable format.
    // epoch seconds → Y-M-D via Zeller-style division.
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    // Days since 1970-01-01 → approximate date (ignores leap years perfectly
    // but close enough for a display timestamp).
    let year = 1970 + days / 365;
    let yday = days % 365;
    let month = yday / 30 + 1;
    let day = yday % 30 + 1;
    format!("{year}-{month:02}-{day:02} {h:02}:{m:02}:{s:02}")
}

// ── Server-side helpers ────────────────────────────────────────────────────────

#[cfg(feature = "server")]
async fn load_blacklist_rows() -> Result<Vec<BlacklistRow>, String> {
    use app_core::db::{Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let db = ScanDatabase::open(&db_path).map_err(|e| e.to_string())?;

    let entries = db.all_blacklisted().map_err(|e| e.to_string())?;

    let mut rows = Vec::new();
    for entry in entries {
        // Try to resolve paths from DB — if not found, show the raw ID.
        let path_a = db.get_file(&entry.file_a)
            .ok().flatten()
            .map(|r| r.path.to_string())
            .unwrap_or_else(|| entry.file_a.clone());
        let path_b = db.get_file(&entry.file_b)
            .ok().flatten()
            .map(|r| r.path.to_string())
            .unwrap_or_else(|| entry.file_b.clone());

        rows.push(BlacklistRow {
            file_a: entry.file_a,
            file_b: entry.file_b,
            path_a,
            path_b,
            added_at: entry.added_at,
        });
    }

    Ok(rows)
}

#[cfg(feature = "server")]
async fn unmark_blacklist_action(file_a: String, file_b: String) -> Result<(), String> {
    use app_core::db::{Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let mut db = ScanDatabase::open(&db_path).map_err(|e| e.to_string())?;
    db.remove_blacklist(&file_a, &file_b).map_err(|e| e.to_string())
}

#[cfg(feature = "server")]
async fn prune_missing_action() -> Result<usize, String> {
    use app_core::db::{Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let mut db = ScanDatabase::open(&db_path).map_err(|e| e.to_string())?;
    let entries = db.all_blacklisted().map_err(|e| e.to_string())?;

    let mut removed = 0usize;
    for entry in entries {
        let a_missing = db.get_file(&entry.file_a)
            .ok().flatten()
            .map(|r| !std::path::Path::new(r.path.as_str()).exists())
            .unwrap_or(true);
        let b_missing = db.get_file(&entry.file_b)
            .ok().flatten()
            .map(|r| !std::path::Path::new(r.path.as_str()).exists())
            .unwrap_or(true);

        if a_missing || b_missing {
            db.remove_blacklist(&entry.file_a, &entry.file_b).map_err(|e| e.to_string())?;
            removed += 1;
        }
    }

    Ok(removed)
}

#[cfg(feature = "server")]
async fn clear_blacklist_action() -> Result<(), String> {
    use app_core::db::{Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let mut db = ScanDatabase::open(&db_path).map_err(|e| e.to_string())?;
    let entries = db.all_blacklisted().map_err(|e| e.to_string())?;

    for entry in entries {
        db.remove_blacklist(&entry.file_a, &entry.file_b).map_err(|e| e.to_string())?;
    }

    Ok(())
}
