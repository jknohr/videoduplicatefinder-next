//! Results view: duplicate clusters loaded from the SurrealDB graph.
//!
//! Each cluster is a set of files connected by duplicate_of edges.
//! The graph traversal (union-find over edges) is done in AppState::load_clusters.

use dioxus::prelude::*;
use crate::app::Route;
use crate::state::AppState;
use crate::state::app_state::{DuplicateCluster, ResultSort};
#[cfg(feature = "server")]
use dirs;

#[cfg(feature = "server")]
use app_core::db::DuplicatePair;
#[cfg(not(feature = "server"))]
use crate::state::app_state::stubs::DuplicatePair;

#[component]
pub fn ResultsView() -> Element {
    let app_state = use_context::<Signal<AppState>>();
    let clusters = app_state.read().clusters.clone();
    let mut search = use_signal(String::new);

    // Client-side path filter
    let filtered: Vec<&DuplicateCluster> = clusters.iter().filter(|c| {
        let q = search.read();
        if q.is_empty() { return true; }
        c.files.iter().any(|f| f.path.as_str().to_lowercase().contains(q.to_lowercase().as_str()))
    }).collect();

    rsx! {
        div { class: "view results-view",
            header { class: "results-header",
                h1 { "Duplicate Groups" }
                p { class: "subtitle",
                    "{filtered.len()} groups across {total_files_ref(&filtered)} files"
                }
                div { class: "search-row",
                    input {
                        r#type: "text",
                        class: "search-box",
                        placeholder: "Filter by path…",
                        value: "{search}",
                        oninput: move |e| search.set(e.value().clone()),
                    }
                    if !search.read().is_empty() {
                        button {
                            class: "btn btn-xs btn-ghost",
                            onclick: move |_| search.set(String::new()),
                            "✕"
                        }
                    }
                }
                ResultsToolbar { app_state }
                AutoSelectBar { app_state }
            }

            if clusters.is_empty() {
                EmptyState {}
            } else if filtered.is_empty() {
                div { class: "empty-state",
                    p { "No groups match \"{search}\"." }
                }
            } else {
                div { class: "cluster-list",
                    for cluster in filtered {
                        {cluster_card(cluster, app_state)}
                    }
                }
            }
        }
    }
}

fn total_files_ref(clusters: &[&DuplicateCluster]) -> usize {
    clusters.iter().map(|c| c.files.len()).sum()}

// ── Toolbar ───────────────────────────────────────────────────────────────────

#[component]
fn ResultsToolbar(mut app_state: Signal<AppState>) -> Element {
    let current_sort = app_state.read().sort;

    rsx! {
        div { class: "toolbar",
            label { "Sort:" }
            select {
                onchange: move |e| {
                    app_state.write().sort = match e.value().as_str() {
                        "sim_asc"  => ResultSort::SimilarityAsc,
                        "size_desc" => ResultSort::SizeDesc,
                        _          => ResultSort::SimilarityDesc,
                    };
                    let sort = app_state.read().sort;
                    apply_sort(&mut app_state.write().clusters, sort);
                },
                option { value: "sim_desc", selected: current_sort == ResultSort::SimilarityDesc, "Similarity ↓" }
                option { value: "sim_asc",  selected: current_sort == ResultSort::SimilarityAsc,  "Similarity ↑" }
                option { value: "size_desc", selected: current_sort == ResultSort::SizeDesc, "Size ↓" }
            }

            label { "Method:" }
            select {
                onchange: move |e| {
                    app_state.write().method_filter = match e.value().as_str() {
                        "all" => None,
                        v     => Some(v.to_string()),
                    };
                },
                option { value: "all", "All methods" }
                option { value: "FrameSimilarity", "Frame hash" }
                option { value: "IframeTimeline", "I-frame timeline" }
                option { value: "AudioFingerprint", "Audio fingerprint" }
            }
        }
    }
}

// ── Auto-select bar ───────────────────────────────────────────────────────────

/// Auto-select: marks files for deletion based on quality heuristics.
/// The selected file IDs are stored in AppState::selected_for_action.
#[component]
fn AutoSelectBar(mut app_state: Signal<AppState>) -> Element {
    rsx! {
        div { class: "autoselect-bar",
            span { class: "bar-label", "Auto-select:" }

            button {
                class: "btn btn-xs btn-outline",
                title: "In each group, mark the smallest file for deletion (keep the largest)",
                onclick: move |_| {
                    let mut state = app_state.write();
                    for cluster in &mut state.clusters {
                        if cluster.files.len() < 2 { continue; }
                        let max_size = cluster.files.iter().map(|f| f.size_bytes).max().unwrap_or(0);
                        for f in &mut cluster.files {
                            // mark via selected_pair field reuse; proper per-file select tracked via app_state extension
                            let _ = f; let _ = max_size; // actual check-state TODO: extend AppState
                        }
                    }
                },
                "Smallest file"
            }

            button {
                class: "btn btn-xs btn-outline",
                title: "In each group, trash all files with identical content hash — keep one",
                onclick: move |_| {
                    // Select all but one file per cluster where all pHashes are identical (sim == 1.0)
                    let mut state = app_state.write();
                    let to_remove: Vec<String> = state.clusters.iter()
                        .filter(|c| c.max_similarity >= 0.9999)
                        .flat_map(|c| c.files.iter().skip(1).map(|f| f.id.clone()))
                        .collect();
                    state.selected_for_action = to_remove;
                },
                "100% equal"
            }

            if !app_state.read().selected_for_action.is_empty() {
                span { class: "selected-count",
                    "{app_state.read().selected_for_action.len()} selected"
                }
                button {
                    class: "btn btn-xs btn-danger",
                    title: "Move selected files to system trash",
                    onclick: move |_| {
                        let ids = app_state.read().selected_for_action.clone();
                        let mut state = app_state;
                        spawn(async move {
                            for id in &ids {
                                #[cfg(feature = "server")]
                                let _ = delete_file_action(id.clone(), true).await;
                            }
                            let mut s = state.write();
                            for id in &ids {
                                s.remove_file(id);
                            }
                            s.selected_for_action.clear();
                        });
                    },
                    "Trash selected"
                }
                MoveToFolderInline { app_state }
                button {
                    class: "btn btn-xs btn-ghost",
                    onclick: move |_| app_state.write().selected_for_action.clear(),
                    "Clear selection"
                }
            }
        }
    }
}

/// Inline move-to-folder widget shown when files are selected.
#[component]
fn MoveToFolderInline(mut app_state: Signal<AppState>) -> Element {
    let mut dest = use_signal(String::new);
    let mut show = use_signal(|| false);

    if !*show.read() {
        return rsx! {
            button {
                class: "btn btn-xs btn-outline",
                onclick: move |_| show.set(true),
                "Move to folder…"
            }
        };
    }

    rsx! {
        div { class: "move-inline",
            input {
                r#type: "text",
                class: "move-dest-input",
                placeholder: "/destination/folder",
                value: "{dest}",
                oninput: move |e| dest.set(e.value().clone()),
            }
            button {
                class: "btn btn-xs btn-primary",
                disabled: dest.read().trim().is_empty(),
                onclick: move |_| {
                    let ids = app_state.read().selected_for_action.clone();
                    let destination = dest.read().trim().to_string();
                    let mut state = app_state;
                    spawn(async move {
                        let mut moved_ids = Vec::new();
                        for id in &ids {
                            #[cfg(feature = "server")]
                            match move_file_action(id.clone(), destination.clone()).await {
                                Ok(()) => moved_ids.push(id.clone()),
                                Err(_) => {}
                            }
                        }
                        let mut s = state.write();
                        for id in &moved_ids {
                            s.remove_file(id);
                        }
                        s.selected_for_action.clear();
                    });
                    dest.set(String::new());
                    show.set(false);
                },
                "Move"
            }
            button {
                class: "btn btn-xs btn-ghost",
                onclick: move |_| { show.set(false); dest.set(String::new()); },
                "Cancel"
            }
        }
    }
}

fn apply_sort(clusters: &mut Vec<DuplicateCluster>, sort: ResultSort) {
    match sort {
        ResultSort::SimilarityDesc => clusters.sort_by(|a, b| {
            b.max_similarity.partial_cmp(&a.max_similarity).unwrap_or(std::cmp::Ordering::Equal)
        }),
        ResultSort::SimilarityAsc => clusters.sort_by(|a, b| {
            a.max_similarity.partial_cmp(&b.max_similarity).unwrap_or(std::cmp::Ordering::Equal)
        }),
        ResultSort::SizeDesc => clusters.sort_by(|a, b| {
            let sa: u64 = a.files.iter().map(|f| f.size_bytes).sum();
            let sb: u64 = b.files.iter().map(|f| f.size_bytes).sum();
            sb.cmp(&sa)
        }),
    }
}

// ── Cluster card ──────────────────────────────────────────────────────────────

fn cluster_card(cluster: &DuplicateCluster, mut app_state: Signal<AppState>) -> Element {
    let method_filter = app_state.read().method_filter.clone();
    let best_edge = cluster.edges.iter()
        .max_by(|a, b| a.similarity.partial_cmp(&b.similarity).unwrap_or(std::cmp::Ordering::Equal))
        .cloned();

    let Some(edge) = best_edge else { return rsx! {} };

    let method_str = method_label(&edge);

    // Skip if method filter is active and doesn't match
    if let Some(ref filter) = method_filter {
        if &method_str != filter {
            return rsx! {};
        }
    }

    // Precompute all owned values before entering rsx! (closures must be 'static)
    let max_similarity = cluster.max_similarity;
    let file_count = cluster.files.len();
    let clip_offset = edge.clip_offset_secs;
    let fa = cluster.files.first().map(|f| f.id.clone()).unwrap_or_default();
    let fb = cluster.files.get(1).map(|f| f.id.clone()).unwrap_or_default();

    // File rows: pre-build display tuples (id, name, path, dur, w, h, size)
    let file_rows: Vec<(String, String, String, f64, u32, u32, u64)> = cluster.files.iter().map(|f| {
        (
            f.id.clone(),
            f.name.clone(),
            f.path.to_string(),
            f.duration_secs(),
            f.width().unwrap_or(0),
            f.height().unwrap_or(0),
            f.size_bytes,
        )
    }).collect();

    // IDs for cluster-level actions
    let cluster_file_ids: Vec<String> = cluster.files.iter().map(|f| f.id.clone()).collect();

    // Track which file's metadata editor is open (by file path, None = closed)
    let mut meta_open_path: Signal<Option<String>> = use_signal(|| None);

    rsx! {
        div { class: "cluster-card",
            div { class: "cluster-header",
                SimilarityBadge { value: max_similarity }
                MethodBadge { method: method_str.clone() }
                span { class: "file-count", "{file_count} files" }
            }

            div { class: "cluster-files",
                for (fid, name, path, dur, w, h, size) in file_rows {
                    div { class: "file-row",
                        div { class: "file-info",
                            div { class: "file-name", "{name}" }
                            div { class: "file-meta",
                                if dur > 0.0 {
                                    span { class: "tag", "{format_duration(dur)}" }
                                }
                                if w > 0 {
                                    span { class: "tag", "{w}×{h}" }
                                }
                                span { class: "tag", "{format_bytes(size)}" }
                            }
                            div { class: "file-path text-muted", "{path}" }
                        }
                        div { class: "file-actions",
                            // Metadata editor toggle
                            button {
                                class: "btn btn-xs btn-ghost",
                                title: "Edit metadata tags",
                                onclick: {
                                    let p = path.clone();
                                    let mut meta_open = meta_open_path;
                                    move |_| {
                                        let mut open = meta_open.write();
                                        if open.as_deref() == Some(&p) {
                                            *open = None;
                                        } else {
                                            *open = Some(p.clone());
                                        }
                                    }
                                },
                                "⋮"
                            }
                            button {
                                class: "btn btn-xs btn-outline",
                                title: "Send to trash",
                                onclick: {
                                    let id = fid.clone();
                                    let mut state = app_state;
                                    move |_| {
                                        let id2 = id.clone();
                                        #[cfg(feature = "server")]
                                        spawn(async move {
                                            if delete_file_action(id2.clone(), true).await.is_ok() {
                                                state.write().remove_file(&id2);
                                            }
                                        });
                                    }
                                },
                                "🗑"
                            }
                            button {
                                class: "btn btn-xs btn-danger",
                                title: "Delete permanently",
                                onclick: {
                                    let id = fid.clone();
                                    let mut state = app_state;
                                    move |_| {
                                        let id2 = id.clone();
                                        #[cfg(feature = "server")]
                                        spawn(async move {
                                            if delete_file_action(id2.clone(), false).await.is_ok() {
                                                state.write().remove_file(&id2);
                                            }
                                        });
                                    }
                                },
                                "✕"
                            }
                        }
                    }
                    // Inline metadata editor — visible when this file's path is selected
                    if meta_open_path.read().as_deref() == Some(&path) {
                        MetadataEditorInline { file_path: path.clone() }
                    }
                }
            }

            if let Some(offset) = clip_offset {
                div { class: "evidence-row",
                    span { class: "evidence-label", "Clip offset:" }
                    span { "{offset:.1}s into the longer file" }
                }
            }

            div { class: "cluster-actions",
                if file_count == 2 {
                    button {
                        class: "btn btn-sm btn-outline",
                        onclick: {
                            let fa2 = fa.clone();
                            let fb2 = fb.clone();
                            move |_| { app_state.write().selected_pair = Some((fa2.clone(), fb2.clone())); }
                        },
                        Link {
                            to: Route::CompareView { file_a: fa.clone(), file_b: fb.clone() },
                            "Compare →"
                        }
                    }
                }
                button {
                    class: "btn btn-sm btn-secondary",
                    title: "Mark as not-a-match — hides this group on future scans",
                    onclick: {
                        let ids = cluster_file_ids.clone();
                        let mut state = app_state;
                        move |_| {
                            let ids2 = ids.clone();
                            #[cfg(feature = "server")]
                            spawn(async move {
                                if blacklist_group_action(ids2.clone()).await.is_ok() {
                                    state.write().remove_cluster_containing(&ids2);
                                }
                            });
                        }
                    },
                    "Blacklist group"
                }
            }
        }
    }
}

// ── Badges ────────────────────────────────────────────────────────────────────

#[component]
fn SimilarityBadge(value: f32) -> Element {
    let pct = (value * 100.0).round() as u32;
    let class = if pct >= 98 { "badge badge-exact" }
        else if pct >= 90 { "badge badge-high" }
        else { "badge badge-medium" };
    rsx! { span { class, "{pct}%" } }
}

#[component]
fn MethodBadge(method: String) -> Element {
    let (label, class) = match method.as_str() {
        "FrameSimilarity"    => ("Frame hash",         "badge badge-blue"),
        "IframeTimeline"     => ("I-frame timeline",   "badge badge-purple"),
        "AudioFingerprint"   => ("Audio fingerprint",  "badge badge-green"),
        "Mpeg7Signature"     => ("MPEG-7",             "badge badge-orange"),
        "SsimVerified"       => ("SSIM verified",      "badge badge-teal"),
        "TemporalAverageHash" => ("Temporal avg",      "badge badge-gray"),
        other                => (other,                "badge badge-gray"),
    };
    rsx! { span { class, "{label}" } }
}

// ── Empty state ───────────────────────────────────────────────────────────────

#[component]
fn EmptyState() -> Element {
    rsx! {
        div { class: "empty-state",
            p { "No duplicates found yet." }
            Link { to: Route::ScanView {},
                button { class: "btn btn-primary", "Start a scan" }
            }
        }
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────────

fn method_label(pair: &DuplicatePair) -> String {
    #[cfg(feature = "server")]
    { format!("{:?}", pair.method) }
    #[cfg(not(feature = "server"))]
    { pair.method_str.clone() }
}

fn format_duration(secs: f64) -> String {
    let h = (secs / 3600.0) as u64;
    let m = ((secs % 3600.0) / 60.0) as u64;
    let s = (secs % 60.0) as u64;
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

fn format_bytes(bytes: u64) -> String {
    const MB: u64 = 1_048_576;
    const GB: u64 = 1_073_741_824;
    if bytes >= GB { format!("{:.1} GB", bytes as f64 / GB as f64) }
    else if bytes >= MB { format!("{:.0} MB", bytes as f64 / MB as f64) }
    else { format!("{} KB", bytes / 1024) }
}

// ── File action helpers (server / desktop only) ───────────────────────────────

/// Delete or trash a file from disk and remove it from the database.
#[cfg(feature = "server")]
pub(crate) async fn delete_file_action(file_id: String, to_trash: bool) -> Result<(), String> {
    use app_core::db::{Database, ScanDatabase};
    use app_core::utils::move_to_trash;

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let mut db = ScanDatabase::open(&db_path)
        .map_err(|e| e.to_string())?;

    if let Some(rec) = db.get_file(&file_id).map_err(|e| e.to_string())? {
        let path = std::path::Path::new(rec.path.as_str());
        if path.exists() {
            if to_trash {
                if !move_to_trash(path) {
                    return Err(format!("Failed to move {} to trash", rec.path));
                }
            } else {
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
    }

    db.remove_file(&file_id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Move a file to a destination folder, updating its path in the database.
///
/// Port of ScanService.MoveItems() from VDF.Web — deconflicts name collisions by
/// appending _1, _2, … to the filename.
#[cfg(feature = "server")]
pub(crate) async fn move_file_action(
    file_id: String,
    destination_folder: String,
) -> Result<(), String> {
    use app_core::db::{Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let mut db = ScanDatabase::open(&db_path).map_err(|e| e.to_string())?;

    let rec = db.get_file(&file_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("file not found: {file_id}"))?;

    let src = std::path::Path::new(rec.path.as_str());
    if !src.exists() {
        return Err(format!("file not found on disk: {}", rec.path));
    }

    std::fs::create_dir_all(&destination_folder)
        .map_err(|e| format!("cannot create destination: {e}"))?;

    let file_name = src.file_name()
        .and_then(|n| n.to_str())
        .ok_or("invalid filename")?;

    // Deconflict: append _N if name already exists at destination
    let mut dest = std::path::PathBuf::from(&destination_folder).join(file_name);
    if dest.exists() {
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or(file_name);
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("");
        let mut n = 1u32;
        loop {
            let candidate = if ext.is_empty() {
                format!("{stem}_{n}")
            } else {
                format!("{stem}_{n}.{ext}")
            };
            dest = std::path::PathBuf::from(&destination_folder).join(&candidate);
            if !dest.exists() { break; }
            n += 1;
        }
    }

    std::fs::rename(src, &dest).map_err(|e| e.to_string())?;

    // Update the file path in the database
    let new_path = camino::Utf8PathBuf::from_path_buf(dest)
        .map_err(|_| "destination path is not valid UTF-8".to_string())?;
    let mut updated = rec;
    updated.path = new_path.clone();
    updated.name = new_path.file_name().unwrap_or("").to_string();
    db.upsert_file(updated).map_err(|e| e.to_string())?;

    Ok(())
}

/// Add blacklist edges for every pair in the group so future scans ignore them.
#[cfg(feature = "server")]
pub(crate) async fn blacklist_group_action(file_ids: Vec<String>) -> Result<(), String> {
    use app_core::db::{BlacklistEntry, Database, ScanDatabase};

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    let mut db = ScanDatabase::open(&db_path)
        .map_err(|e| e.to_string())?;

    for i in 0..file_ids.len() {
        for j in (i + 1)..file_ids.len() {
            let entry = BlacklistEntry::new(
                file_ids[i].clone(),
                file_ids[j].clone(),
                Some("user_marked".to_string()),
            );
            db.add_blacklist(entry).map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

// ── Metadata editor ───────────────────────────────────────────────────────────

/// Inline metadata tag editor. Opens a panel below a file row showing all
/// container tags read via ffprobe; allows editing and saving.
///
/// C# ref: VDF.GUI — EditMetadata dialog triggered from file context menu.
#[component]
pub fn MetadataEditorInline(file_path: String) -> Element {
    let mut tags: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let mut loading = use_signal(|| false);
    let mut status: Signal<Option<String>> = use_signal(|| None);

    // Load tags when the panel mounts
    use_effect({
        let path = file_path.clone();
        let mut tags = tags.clone();
        let mut loading = loading.clone();
        move || {
            #[cfg(feature = "server")]
            {
                *loading.write() = true;
                let map = app_core::read_metadata_tags(camino::Utf8Path::new(&path));
                let mut sorted: Vec<(String, String)> = map.into_iter().collect();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                *tags.write() = sorted;
                *loading.write() = false;
            }
        }
    });

    let save = {
        let path = file_path.clone();
        let tags = tags.clone();
        let mut status = status.clone();
        move |_| {
            let path2 = path.clone();
            let map: std::collections::HashMap<String, String> = tags.read().iter().cloned().collect();
            #[cfg(feature = "server")]
            {
                let (ok, err) = app_core::write_metadata_tags(camino::Utf8Path::new(&path2), &map);
                if ok {
                    *status.write() = Some("Saved.".to_string());
                } else {
                    *status.write() = Some(format!("Error: {}", err.unwrap_or_default()));
                }
            }
        }
    };

    rsx! {
        div { class: "metadata-editor",
            if *loading.read() {
                p { class: "text-muted", "Loading tags…" }
            } else if tags.read().is_empty() {
                p { class: "text-muted", "No container tags found." }
            } else {
                div { class: "tag-grid",
                    for (idx, (key, val)) in tags.read().iter().enumerate() {
                        div { class: "tag-row",
                            span { class: "tag-key text-muted", "{key}" }
                            input {
                                class: "input tag-value",
                                r#type: "text",
                                value: "{val}",
                                oninput: {
                                    let mut tags = tags.clone();
                                    move |e| {
                                        if let Some(t) = tags.write().get_mut(idx) {
                                            t.1 = e.value();
                                        }
                                    }
                                },
                            }
                        }
                    }
                }
                div { class: "metadata-actions",
                    button {
                        class: "btn btn-sm btn-primary",
                        onclick: save,
                        "Save tags"
                    }
                    if let Some(msg) = status.read().as_ref() {
                        span { class: "status-msg text-muted", "{msg}" }
                    }
                }
            }
        }
    }
}
