//! Database browser — paginated, sortable list of all scanned files.
//! Mirrors VDF.GUI/Views/DatabaseViewer.xaml.

use dioxus::prelude::*;

const PAGE_SIZE: usize = 50;

#[derive(Debug, Clone, PartialEq)]
struct DbRow {
    id: String,
    name: String,
    path: String,
    size_bytes: u64,
    is_image: bool,
    scanned_at: u64,
    has_sha256: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DbSort {
    Name,
    Path,
    SizeAsc,
    SizeDesc,
    DateAsc,
    DateDesc,
}

#[component]
pub fn DatabaseView() -> Element {
    let rows: Signal<Vec<DbRow>> = use_signal(Vec::new);
    let mut page = use_signal(|| 0usize);
    let mut sort = use_signal(|| DbSort::Name);
    let mut search = use_signal(String::new);
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);
    let mut loading = use_signal(|| false);

    // Load on mount
    use_effect({
        let mut rows = rows.clone();
        let mut loading = loading.clone();
        let mut error_msg = error_msg.clone();
        move || {
            #[cfg(feature = "server")]
            spawn(async move {
                *loading.write() = true;
                match load_db_files().await {
                    Ok(loaded) => {
                        *rows.write() = loaded;
                        *error_msg.write() = None;
                    }
                    Err(e) => {
                        *error_msg.write() = Some(e);
                    }
                }
                *loading.write() = false;
            });
        }
    });

    // Build the filtered + sorted view
    let search_val = search.read().to_lowercase();
    let mut visible: Vec<DbRow> = rows.read().iter()
        .filter(|r| {
            search_val.is_empty()
                || r.name.to_lowercase().contains(&search_val)
                || r.path.to_lowercase().contains(&search_val)
        })
        .cloned()
        .collect();

    match *sort.read() {
        DbSort::Name     => visible.sort_by(|a, b| a.name.cmp(&b.name)),
        DbSort::Path     => visible.sort_by(|a, b| a.path.cmp(&b.path)),
        DbSort::SizeAsc  => visible.sort_by_key(|r| r.size_bytes),
        DbSort::SizeDesc => visible.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes)),
        DbSort::DateAsc  => visible.sort_by_key(|r| r.scanned_at),
        DbSort::DateDesc => visible.sort_by(|a, b| b.scanned_at.cmp(&a.scanned_at)),
    }

    let total_pages = visible.len().saturating_sub(1) / PAGE_SIZE + 1;
    let cur_page = (*page.read()).min(total_pages.saturating_sub(1));
    let page_rows = visible.iter()
        .skip(cur_page * PAGE_SIZE)
        .take(PAGE_SIZE)
        .cloned()
        .collect::<Vec<_>>();

    rsx! {
        div { class: "view database-view",
            div { class: "db-toolbar",
                h1 { class: "view-title", "Database" }

                input {
                    class: "input db-search",
                    r#type: "search",
                    placeholder: "Filter by name or path…",
                    value: "{search}",
                    oninput: move |e| {
                        *search.write() = e.value();
                        *page.write() = 0;
                    },
                }

                span { class: "text-muted", "{visible.len()} files" }

                button {
                    class: "btn btn-sm btn-outline",
                    disabled: *loading.read(),
                    onclick: {
                        let mut rows = rows.clone();
                        let mut loading = loading.clone();
                        let mut error_msg = error_msg.clone();
                        move |_| {
                            #[cfg(feature = "server")]
                            spawn(async move {
                                *loading.write() = true;
                                match load_db_files().await {
                                    Ok(loaded) => { *rows.write() = loaded; *error_msg.write() = None; }
                                    Err(e) => { *error_msg.write() = Some(e); }
                                }
                                *loading.write() = false;
                            });
                        }
                    },
                    if *loading.read() { "Loading…" } else { "Refresh" }
                }
            }

            if let Some(err) = error_msg.read().as_ref() {
                div { class: "alert alert-error", "{err}" }
            }

            // Sort controls
            div { class: "db-sort",
                span { class: "text-muted", "Sort by:" }
                SortButton { label: "Name",     active: *sort.read() == DbSort::Name,     onclick: move |_| *sort.write() = DbSort::Name }
                SortButton { label: "Path",     active: *sort.read() == DbSort::Path,     onclick: move |_| *sort.write() = DbSort::Path }
                SortButton { label: "Size ↑",   active: *sort.read() == DbSort::SizeAsc,  onclick: move |_| *sort.write() = DbSort::SizeAsc }
                SortButton { label: "Size ↓",   active: *sort.read() == DbSort::SizeDesc, onclick: move |_| *sort.write() = DbSort::SizeDesc }
                SortButton { label: "Date ↑",   active: *sort.read() == DbSort::DateAsc,  onclick: move |_| *sort.write() = DbSort::DateAsc }
                SortButton { label: "Date ↓",   active: *sort.read() == DbSort::DateDesc, onclick: move |_| *sort.write() = DbSort::DateDesc }
            }

            // File table
            div { class: "db-table",
                div { class: "db-table-header",
                    span { class: "db-col db-col-name", "File" }
                    span { class: "db-col db-col-size", "Size" }
                    span { class: "db-col db-col-date", "Scanned" }
                    span { class: "db-col db-col-type", "Type" }
                    span { class: "db-col db-col-actions", "Actions" }
                }
                if page_rows.is_empty() && !*loading.read() {
                    div { class: "db-empty text-muted", "No files match the current filter." }
                }
                for row in page_rows.clone() {
                    DbRowCard {
                        row: row.clone(),
                        on_delete: {
                            let id = row.id.clone();
                            let mut rows = rows.clone();
                            move |_| {
                                let id2 = id.clone();
                                let mut rows2 = rows.clone();
                                #[cfg(feature = "server")]
                                spawn(async move {
                                    if let Ok(()) = delete_db_entry(id2.clone()).await {
                                        rows2.write().retain(|r| r.id != id2);
                                    }
                                });
                            }
                        },
                    }
                }
            }

            // Pagination
            if total_pages > 1 {
                div { class: "pagination",
                    button {
                        class: "btn btn-sm btn-ghost",
                        disabled: cur_page == 0,
                        onclick: move |_| { if *page.read() > 0 { *page.write() -= 1; } },
                        "← Prev"
                    }
                    span { class: "page-indicator", "Page {cur_page + 1} of {total_pages}" }
                    button {
                        class: "btn btn-sm btn-ghost",
                        disabled: cur_page + 1 >= total_pages,
                        onclick: move |_| { *page.write() += 1; },
                        "Next →"
                    }
                }
            }
        }
    }
}

// ── Sub-components ────────────────────────────────────────────────────────────

#[component]
fn SortButton(label: &'static str, active: bool, onclick: EventHandler<()>) -> Element {
    rsx! {
        button {
            class: if active { "btn btn-xs btn-primary" } else { "btn btn-xs btn-ghost" },
            onclick: move |_| onclick.call(()),
            "{label}"
        }
    }
}

#[component]
fn DbRowCard(row: DbRow, on_delete: EventHandler<()>) -> Element {
    let scanned_dt = format_unix(row.scanned_at);
    let type_label = if row.is_image { "Image" } else { "Video" };

    rsx! {
        div { class: "db-row",
            div { class: "db-col db-col-name",
                div { class: "db-filename", "{row.name}" }
                div { class: "db-filepath text-muted", "{row.path}" }
            }
            div { class: "db-col db-col-size text-muted", "{format_bytes(row.size_bytes)}" }
            div { class: "db-col db-col-date text-muted", "{scanned_dt}" }
            div { class: "db-col db-col-type",
                span { class: if row.is_image { "badge badge-image" } else { "badge badge-video" }, "{type_label}" }
            }
            div { class: "db-col db-col-actions",
                button {
                    class: "btn btn-xs btn-danger",
                    title: "Remove from database (does not delete file from disk)",
                    onclick: move |_| on_delete.call(()),
                    "Remove"
                }
            }
        }
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────────

fn format_bytes(b: u64) -> String {
    if b >= 1_000_000_000 {
        format!("{:.1} GB", b as f64 / 1_000_000_000.0)
    } else if b >= 1_000_000 {
        format!("{:.1} MB", b as f64 / 1_000_000.0)
    } else if b >= 1_000 {
        format!("{:.0} KB", b as f64 / 1_000.0)
    } else {
        format!("{b} B")
    }
}

fn format_unix(ts: u64) -> String {
    // Simple YYYY-MM-DD HH:MM from Unix seconds (UTC approximation).
    // Using chrono would be ideal but avoiding the dependency for now.
    let secs = ts;
    let days  = secs / 86400;
    let rem   = secs % 86400;
    let h     = rem / 3600;
    let m     = (rem % 3600) / 60;
    // Julian day → Gregorian calendar (Fliegel algorithm)
    let jd = days as i64 + 2440588; // 1970-01-01 is JD 2440588
    let l = jd + 68569;
    let n = (4 * l) / 146097;
    let l = l - (146097 * n + 3) / 4;
    let i = (4000 * (l + 1)) / 1461001;
    let l = l - (1461 * i) / 4 + 31;
    let j = (80 * l) / 2447;
    let d = l - (2447 * j) / 80;
    let l = j / 11;
    let m_num = j + 2 - 12 * l;
    let y = 100 * (n - 49) + i + l;
    format!("{y:04}-{m_num:02}-{d:02} {h:02}:{m:02}")
}

// ── Server helpers ────────────────────────────────────────────────────────────

#[cfg(feature = "server")]
async fn load_db_files() -> Result<Vec<DbRow>, String> {
    use app_core::db::{Database, ScanDatabase};
    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf").join("db");
    let db = ScanDatabase::open(&db_path).map_err(|e| e.to_string())?;
    let files = db.all_files().map_err(|e| e.to_string())?;
    Ok(files.into_iter().map(|f| {
        let is_image = f.is_image();
        DbRow {
            id: f.id,
            name: f.name,
            path: f.path.to_string(),
            size_bytes: f.size_bytes,
            is_image,
            scanned_at: f.scanned_at,
            has_sha256: f.sha256.is_some(),
        }
    }).collect())
}

#[cfg(feature = "server")]
async fn delete_db_entry(file_id: String) -> Result<(), String> {
    use app_core::db::{Database, ScanDatabase};
    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf").join("db");
    let mut db = ScanDatabase::open(&db_path).map_err(|e| e.to_string())?;
    db.delete_file(&file_id).map_err(|e| e.to_string())
}
