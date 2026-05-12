//! Relocate Files view — update file paths in the database when files have moved.
//!
//! Port of VDF.GUI/Views/RelocateFilesDialog.axaml + RelocateFilesDialogVM.cs.
//!
//! Two modes:
//!   A (Prefix) — replace a path prefix across all DB records instantly.
//!   B (Rescan) — walk new directories to re-locate missing files by size,
//!                mtime, and optionally duration.

use dioxus::prelude::*;
#[cfg(feature = "server")]
use dirs;

// ── Public component ──────────────────────────────────────────────────────────

#[component]
pub fn RelocateView() -> Element {
    let mut mode = use_signal(|| RelocateMode::Prefix);
    let mut old_prefix = use_signal(String::new);
    let mut new_prefix = use_signal(String::new);
    let mut scan_roots: Signal<Vec<String>> = use_signal(Vec::new);
    let mut use_mtime = use_signal(|| true);
    let mut use_duration = use_signal(|| false);
    let mut preview: Signal<Vec<RelocateRow>> = use_signal(Vec::new);
    let mut loading = use_signal(|| false);
    let mut status = use_signal(String::new);
    let mut root_input = use_signal(String::new);

    rsx! {
        div { class: "view relocate-view",
            h1 { "Relocate Files" }
            p { class: "text-muted",
                "Update file paths in the database when media files have moved or been remounted."
            }

            // ── Mode selector ─────────────────────────────────────────────
            div { class: "mode-selector",
                label { class: "radio-label",
                    input {
                        r#type: "radio",
                        name: "mode",
                        checked: *mode.read() == RelocateMode::Prefix,
                        onchange: {
                            let mut m = mode.clone();
                            move |_| m.set(RelocateMode::Prefix)
                        },
                    }
                    " Replace path prefix"
                }
                label { class: "radio-label",
                    input {
                        r#type: "radio",
                        name: "mode",
                        checked: *mode.read() == RelocateMode::Rescan,
                        onchange: {
                            let mut m = mode.clone();
                            move |_| m.set(RelocateMode::Rescan)
                        },
                    }
                    " Rescan folders to find missing files"
                }
            }

            // ── Mode A — prefix replace ───────────────────────────────────
            if *mode.read() == RelocateMode::Prefix {
                div { class: "prefix-inputs",
                    div { class: "input-row",
                        label { "From prefix:" }
                        input {
                            r#type: "text",
                            class: "input",
                            placeholder: "/old/mount/point",
                            value: "{old_prefix}",
                            oninput: move |e| old_prefix.set(e.value()),
                        }
                    }
                    div { class: "input-row",
                        label { "To prefix:" }
                        input {
                            r#type: "text",
                            class: "input",
                            placeholder: "/new/mount/point",
                            value: "{new_prefix}",
                            oninput: move |e| new_prefix.set(e.value()),
                        }
                    }
                }
            }

            // ── Mode B — rescan ───────────────────────────────────────────
            if *mode.read() == RelocateMode::Rescan {
                div { class: "rescan-inputs",
                    h3 { "Scan folders" }
                    ul { class: "root-list",
                        for (idx, root) in scan_roots.read().iter().enumerate() {
                            li {
                                span { "{root}" }
                                button {
                                    class: "btn btn-xs btn-ghost",
                                    onclick: {
                                        let mut sr = scan_roots.clone();
                                        move |_| { sr.write().remove(idx); }
                                    },
                                    "✕"
                                }
                            }
                        }
                    }
                    div { class: "add-root-row",
                        input {
                            r#type: "text",
                            class: "input",
                            placeholder: "/path/to/folder",
                            value: "{root_input}",
                            oninput: move |e| root_input.set(e.value()),
                        }
                        button {
                            class: "btn btn-sm btn-outline",
                            onclick: {
                                let mut ri = root_input.clone();
                                let mut sr = scan_roots.clone();
                                move |_| {
                                    let v = ri.read().trim().to_string();
                                    if !v.is_empty() && !sr.read().contains(&v) {
                                        sr.write().push(v);
                                        ri.set(String::new());
                                    }
                                }
                            },
                            "Add folder"
                        }
                    }

                    div { class: "refine-options",
                        label {
                            input {
                                r#type: "checkbox",
                                checked: *use_mtime.read(),
                                onchange: move |e| use_mtime.set(e.checked()),
                            }
                            " Match by last-modified time (within 2 s)"
                        }
                        label {
                            input {
                                r#type: "checkbox",
                                checked: *use_duration.read(),
                                onchange: move |e| use_duration.set(e.checked()),
                            }
                            " Refine by duration (within 0.5 s)"
                        }
                    }
                }
            }

            // ── Toolbar ───────────────────────────────────────────────────
            div { class: "relocate-toolbar",
                button {
                    class: "btn btn-primary",
                    disabled: *loading.read(),
                    onclick: {
                        let md = mode.clone();
                        let op = old_prefix.clone();
                        let np = new_prefix.clone();
                        let sr = scan_roots.clone();
                        let um = use_mtime.clone();
                        let ud = use_duration.clone();
                        let mut pv = preview.clone();
                        let mut ld = loading.clone();
                        let mut st = status.clone();
                        move |_| {
                            let mode_val = *md.read();
                            let old_p = op.read().trim().to_string();
                            let new_p = np.read().trim().to_string();
                            let roots = sr.read().clone();
                            let mtime = *um.read();
                            let dur = *ud.read();
                            spawn(async move {
                                *ld.write() = true;
                                *st.write() = "Building preview…".to_string();
                                #[cfg(feature = "server")]
                                {
                                    match build_preview(mode_val, old_p, new_p, roots, mtime, dur).await {
                                        Ok(rows) => {
                                            let n = rows.len();
                                            *pv.write() = rows;
                                            *st.write() = format!("{n} entries found");
                                        }
                                        Err(e) => { *st.write() = format!("Error: {e}"); }
                                    }
                                }
                                *ld.write() = false;
                            });
                        }
                    },
                    "Build preview"
                }

                button {
                    class: "btn btn-ghost",
                    onclick: {
                        let mut pv = preview.clone();
                        move |_| { pv.write().iter_mut().for_each(|r| r.selected = true); }
                    },
                    "Check all"
                }
                button {
                    class: "btn btn-ghost",
                    onclick: {
                        let mut pv = preview.clone();
                        move |_| { pv.write().iter_mut().for_each(|r| r.selected = false); }
                    },
                    "Uncheck all"
                }

                button {
                    class: "btn btn-success",
                    disabled: preview.read().iter().all(|r| !r.selected),
                    onclick: {
                        let pv = preview.clone();
                        let mut st = status.clone();
                        move |_| {
                            let rows = pv.read().clone();
                            spawn(async move {
                                #[cfg(feature = "server")]
                                match apply_relocate(rows).await {
                                    Ok(n) => { *st.write() = format!("Applied {n} path updates."); }
                                    Err(e) => { *st.write() = format!("Error: {e}"); }
                                }
                            });
                        }
                    },
                    "Apply selected"
                }

                if !status.read().is_empty() {
                    span { class: "status-msg text-muted", "{status}" }
                }
                if *loading.read() {
                    span { class: "text-muted", " Working…" }
                }
            }

            // ── Preview table ─────────────────────────────────────────────
            if !preview.read().is_empty() {
                div { class: "preview-scroll",
                    table { class: "preview-table",
                        thead {
                            tr {
                                th { "Apply" }
                                th { "Old path" }
                                th { "New path" }
                                th { "Confidence" }
                                th { "Note" }
                            }
                        }
                        tbody {
                            for (idx, row) in preview.read().iter().enumerate() {
                                tr { class: if row.new_path.is_none() { "row-notfound" } else { "" },
                                    td {
                                        input {
                                            r#type: "checkbox",
                                            checked: row.selected,
                                            onchange: {
                                                let mut pv = preview.clone();
                                                move |e| { pv.write()[idx].selected = e.checked(); }
                                            },
                                        }
                                    }
                                    td { class: "path-cell", "{row.old_path}" }
                                    td { class: "path-cell",
                                        if let Some(ref np) = row.new_path {
                                            "{np}"
                                        } else {
                                            span { class: "text-muted", "—" }
                                        }
                                    }
                                    td { class: "confidence-cell", "{row.confidence}" }
                                    td { class: "note-cell", "{row.note}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelocateMode {
    Prefix,
    Rescan,
}

#[derive(Debug, Clone)]
pub struct RelocateRow {
    pub old_path: String,
    pub new_path: Option<String>,
    pub selected: bool,
    pub confidence: String,
    pub note: String,
    /// File ID in the database (used when applying updates)
    pub file_id: String,
}

// ── Server-side logic ─────────────────────────────────────────────────────────

/// Build the preview list.
#[cfg(feature = "server")]
async fn build_preview(
    mode: RelocateMode,
    old_prefix: String,
    new_prefix: String,
    scan_roots: Vec<String>,
    use_mtime: bool,
    use_duration: bool,
) -> Result<Vec<RelocateRow>, String> {
    use app_core::db::{Database, ScanDatabase};
    use tokio::task::spawn_blocking;

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let rows = spawn_blocking(move || -> Result<Vec<RelocateRow>, String> {
        let db = ScanDatabase::open(&db_path).map_err(|e| e.to_string())?;
        let files = db.all_files().map_err(|e| e.to_string())?;

        match mode {
            RelocateMode::Prefix => build_prefix_preview(files, &old_prefix, &new_prefix),
            RelocateMode::Rescan => build_rescan_preview(files, &scan_roots, use_mtime, use_duration),
        }
    }).await.map_err(|e| e.to_string())??;

    Ok(rows)
}

/// Mode A: simple prefix replacement.
#[cfg(feature = "server")]
fn build_prefix_preview(
    files: Vec<app_core::db::FileRecord>,
    old_prefix: &str,
    new_prefix: &str,
) -> Result<Vec<RelocateRow>, String> {
    if old_prefix.is_empty() || new_prefix.is_empty() {
        return Err("Both prefix fields must be filled in.".to_string());
    }

    let old = old_prefix.trim_end_matches('/');
    let new_p = new_prefix.trim_end_matches('/');

    let rows: Vec<RelocateRow> = files.into_iter().filter_map(|f| {
        let path = f.path.as_str();
        if !path.starts_with(old) { return None; }
        let suffix = &path[old.len()..];
        let new_path = format!("{new_p}{suffix}");
        Some(RelocateRow {
            old_path: path.to_string(),
            new_path: Some(new_path),
            selected: true,
            confidence: "Prefix".to_string(),
            note: "Prefix replace".to_string(),
            file_id: f.id.clone(),
        })
    }).collect();

    Ok(rows)
}

/// Mode B: find missing files by walking scan_roots, match by size → mtime → duration.
#[cfg(feature = "server")]
fn build_rescan_preview(
    files: Vec<app_core::db::FileRecord>,
    scan_roots: &[String],
    use_mtime: bool,
    use_duration: bool,
) -> Result<Vec<RelocateRow>, String> {
    use std::collections::HashMap;

    if scan_roots.is_empty() {
        return Err("Add at least one scan folder.".to_string());
    }

    // 1) Identify missing files
    let missing: Vec<&app_core::db::FileRecord> = files.iter()
        .filter(|f| !std::path::Path::new(f.path.as_str()).exists())
        .collect();

    // 2) Index files in scan_roots by size → Vec<(path, mtime_secs)>
    let mut by_size: HashMap<u64, Vec<(String, u64)>> = HashMap::new();
    for root in scan_roots {
        let root_path = std::path::Path::new(root);
        if !root_path.is_dir() { continue; }
        walk_dir(root_path, &mut by_size);
    }

    // 3) Match each missing file
    let rows: Vec<RelocateRow> = missing.into_iter().map(|f| {
        let candidates = match by_size.get(&f.size_bytes) {
            Some(v) => v.clone(),
            None => return RelocateRow {
                old_path: f.path.to_string(),
                new_path: None,
                selected: false,
                confidence: "NotFound".to_string(),
                note: "No same-size file in scan roots".to_string(),
                file_id: f.id.clone(),
            },
        };

        if candidates.len() == 1 {
            return RelocateRow {
                old_path: f.path.to_string(),
                new_path: Some(candidates[0].0.clone()),
                selected: true,
                confidence: "SizeOnly".to_string(),
                note: "Unique by size".to_string(),
                file_id: f.id.clone(),
            };
        }

        // Refine by mtime
        let mut filtered: Vec<&(String, u64)> = candidates.iter().collect();
        if use_mtime {
            // recorded mtime is stored as scanned_at (epoch secs) — use as proxy
            let recorded = f.scanned_at;
            filtered.retain(|c| c.1.abs_diff(recorded) <= 2);
        }

        // Refine by duration via ffprobe
        if use_duration && filtered.len() > 1 {
            let recorded_dur = f.duration_secs();
            if recorded_dur > 0.0 {
                filtered.retain(|c| {
                    quick_duration(&c.0)
                        .map(|d| (d - recorded_dur).abs() <= 0.5)
                        .unwrap_or(false)
                });
            }
        }

        match filtered.len() {
            0 => RelocateRow {
                old_path: f.path.to_string(),
                new_path: None,
                selected: false,
                confidence: "NotFound".to_string(),
                note: "No candidate after refinements".to_string(),
                file_id: f.id.clone(),
            },
            1 => {
                let conf = if use_duration && use_mtime { "SizeModifiedDuration" }
                    else if use_mtime { "SizeAndModified" }
                    else { "SizeOnly" };
                RelocateRow {
                    old_path: f.path.to_string(),
                    new_path: Some(filtered[0].0.clone()),
                    selected: true,
                    confidence: conf.to_string(),
                    note: format!("Resolved by {conf}"),
                    file_id: f.id.clone(),
                }
            }
            n => RelocateRow {
                old_path: f.path.to_string(),
                new_path: None,
                selected: false,
                confidence: "Ambiguous".to_string(),
                note: format!("{n} candidates remain"),
                file_id: f.id.clone(),
            },
        }
    }).collect();

    Ok(rows)
}

/// Recursively walk `dir`, collecting (path, mtime_secs) indexed by file size.
#[cfg(feature = "server")]
fn walk_dir(dir: &std::path::Path, by_size: &mut std::collections::HashMap<u64, Vec<(String, u64)>>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, by_size);
        } else if path.is_file() {
            if let Ok(meta) = std::fs::metadata(&path) {
                let size = meta.len();
                let mtime = meta.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if let Some(p) = path.to_str() {
                    by_size.entry(size).or_default().push((p.to_string(), mtime));
                }
            }
        }
    }
}

/// Quick ffprobe duration check for a candidate file.
///
/// Port of `QuickMeta.TryRead()` from C#.
#[cfg(feature = "server")]
fn quick_duration(path: &str) -> Option<f64> {
    app_core::ffmpeg::probe_media(camino::Utf8Path::new(path))
        .ok()
        .map(|info| info.duration_secs)
}

/// Apply selected path updates to the SurrealDB database.
#[cfg(feature = "server")]
async fn apply_relocate(rows: Vec<RelocateRow>) -> Result<usize, String> {
    use app_core::db::{Database, ScanDatabase};
    use tokio::task::spawn_blocking;

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let count = spawn_blocking(move || -> Result<usize, String> {
        let mut db = ScanDatabase::open(&db_path).map_err(|e| e.to_string())?;
        let mut applied = 0usize;
        for row in &rows {
            let Some(ref new_path_str) = row.new_path else { continue };
            if !row.selected { continue; }
            let new_path = camino::Utf8PathBuf::from(new_path_str);
            // Load the existing record, update the path, re-upsert
            if let Ok(Some(mut rec)) = db.get_file(&row.file_id) {
                rec.name = new_path.file_name().unwrap_or("").to_string();
                rec.path = new_path;
                db.upsert_file(rec).map_err(|e| e.to_string())?;
                applied += 1;
            }
        }
        Ok(applied)
    }).await.map_err(|e| e.to_string())??;

    Ok(count)
}
