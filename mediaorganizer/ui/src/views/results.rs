//! Results view: duplicate clusters loaded from the SurrealDB graph.
//!
//! Each cluster is a set of files connected by duplicate_of edges.
//! The graph traversal (union-find over edges) is done in AppState::load_clusters.

use dioxus::prelude::*;
use urlencoding;
use crate::app::Route;
use crate::state::AppState;
use crate::state::app_state::{DuplicateCluster, ResultSort, ALL_CRITERIA};
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
    // 0 = All, 1 = Videos only, 2 = Images only
    let mut type_filter = use_signal(|| 0u8);
    // Similarity range filter [0.0, 1.0]
    let mut sim_min = use_signal(|| 0.0f32);

    // Client-side filters: path search, file-type, similarity threshold
    let filtered: Vec<&DuplicateCluster> = clusters.iter().filter(|c| {
        // Path search
        let q = search.read();
        if !q.is_empty() && !c.files.iter().any(|f| f.path.as_str().to_lowercase().contains(q.to_lowercase().as_str())) {
            return false;
        }
        // Similarity minimum
        let min = *sim_min.read();
        if min > 0.0 && c.max_similarity < min { return false; }
        // File type
        match *type_filter.read() {
            1 => c.files.iter().any(|f| !f.is_image()),    // Videos: keep groups with at least one video
            2 => c.files.iter().any(|f| f.is_image()),     // Images: keep groups with at least one image
            _ => true,
        }
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

                    // File type filter
                    select {
                        class: "select select-sm",
                        title: "Filter by file type",
                        onchange: move |e| {
                            *type_filter.write() = match e.value().as_str() {
                                "videos" => 1,
                                "images" => 2,
                                _ => 0,
                            };
                        },
                        option { value: "all",    selected: *type_filter.read() == 0, "All types" }
                        option { value: "videos", selected: *type_filter.read() == 1, "Videos only" }
                        option { value: "images", selected: *type_filter.read() == 2, "Images only" }
                    }

                    // Similarity minimum slider
                    span { class: "filter-label text-muted",
                        "Min sim: {(*sim_min.read() * 100.0) as u32}%"
                    }
                    input {
                        r#type: "range",
                        class: "filter-slider",
                        min: "0", max: "100", step: "1",
                        value: "{(*sim_min.read() * 100.0) as u32}",
                        title: "Minimum similarity threshold",
                        oninput: move |e| {
                            if let Ok(v) = e.value().parse::<f32>() {
                                *sim_min.write() = v / 100.0;
                            }
                        },
                    }
                }
                ResultsToolbar { app_state }
                AutoSelectBar { app_state }
                QualityOrderPanel { app_state }
                CustomSelectionPanel { app_state }
                SurrealSelectionPanel { app_state }
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
            span { class: "bar-label", "Auto-select duplicates to remove:" }

            // Select all but the best-quality file in each group (uses the ranker with user's criteria order)
            button {
                class: "btn btn-xs btn-outline",
                title: "In each group, keep the highest-quality file; select the rest. Use 'Quality order…' to change priorities.",
                onclick: move |_| {
                    #[cfg(feature = "server")]
                    {
                        let mut state = app_state.write();
                        let mut to_remove: Vec<String> = Vec::new();
                        let criteria = build_criteria_from_order(&state.criteria_order);
                        for cluster in &state.clusters {
                            if cluster.files.len() < 2 { continue; }
                            if let Some(keeper) = app_core::ranker::pick_keeper(&cluster.files, &criteria) {
                                for f in &cluster.files {
                                    if f.id != keeper.id {
                                        to_remove.push(f.id.clone());
                                    }
                                }
                            }
                        }
                        state.selected_for_action = to_remove;
                    }
                },
                "Best quality (keep)"
            }

            // Select all but the largest file in each group
            button {
                class: "btn btn-xs btn-outline",
                title: "In each group, keep the largest file; select smaller ones",
                onclick: move |_| {
                    let mut state = app_state.write();
                    let mut to_remove: Vec<String> = Vec::new();
                    for cluster in &state.clusters {
                        if cluster.files.len() < 2 { continue; }
                        let keeper_id = cluster.files.iter()
                            .max_by_key(|f| f.size_bytes)
                            .map(|f| f.id.clone());
                        if let Some(kid) = keeper_id {
                            for f in &cluster.files {
                                if f.id != kid { to_remove.push(f.id.clone()); }
                            }
                        }
                    }
                    state.selected_for_action = to_remove;
                },
                "Smallest files"
            }

            button {
                class: "btn btn-xs btn-outline",
                title: "Select all but one file per group where similarity is 100%",
                onclick: move |_| {
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

// ── Quality order panel ───────────────────────────────────────────────────────

/// Panel letting the user reorder the quality criteria used by "Best quality (keep)".
///
/// Port of QualityOrderDialog from VDF.GUI. Shown inline below the AutoSelectBar.
#[component]
fn QualityOrderPanel(mut app_state: Signal<AppState>) -> Element {
    let mut open = use_signal(|| false);

    if !*open.read() {
        return rsx! {
            button {
                class: "btn btn-xs btn-ghost",
                title: "Reorder quality criteria used by auto-select",
                onclick: move |_| open.set(true),
                "Quality order…"
            }
        };
    }

    // Local copy of the order for editing; changes are committed on "Apply"
    let mut local_order: Signal<Vec<String>> = use_signal({
        let order = app_state.read().criteria_order.clone();
        move || order.clone()
    });

    rsx! {
        div { class: "quality-order-panel panel",
            h3 { "Quality Criteria Order" }
            p { class: "text-muted",
                "Drag or use buttons to reorder. Highest priority first."
            }
            ul { class: "criteria-list",
                for (idx, name) in local_order.read().iter().enumerate() {
                    li { class: "criteria-row",
                        span { class: "criteria-name", "{name}" }
                        div { class: "criteria-btns",
                            button {
                                class: "btn btn-xs btn-ghost",
                                disabled: idx == 0,
                                onclick: {
                                    let mut lo = local_order.clone();
                                    move |_| {
                                        let mut v = lo.write();
                                        if idx > 0 { v.swap(idx, idx - 1); }
                                    }
                                },
                                "↑"
                            }
                            button {
                                class: "btn btn-xs btn-ghost",
                                disabled: idx + 1 >= local_order.read().len(),
                                onclick: {
                                    let mut lo = local_order.clone();
                                    move |_| {
                                        let mut v = lo.write();
                                        if idx + 1 < v.len() { v.swap(idx, idx + 1); }
                                    }
                                },
                                "↓"
                            }
                        }
                    }
                }
            }
            div { class: "panel-actions",
                button {
                    class: "btn btn-sm btn-primary",
                    onclick: {
                        let lo = local_order.clone();
                        move |_| {
                            app_state.write().criteria_order = lo.read().clone();
                            open.set(false);
                        }
                    },
                    "Apply"
                }
                button {
                    class: "btn btn-sm btn-ghost",
                    onclick: move |_| open.set(false),
                    "Cancel"
                }
                button {
                    class: "btn btn-sm btn-ghost",
                    title: "Reset to default order",
                    onclick: {
                        let mut lo = local_order.clone();
                        move |_| {
                            *lo.write() = ALL_CRITERIA.iter().map(|s| s.to_string()).collect();
                        }
                    },
                    "Reset"
                }
            }
        }
    }
}

// ── Custom selection panel ────────────────────────────────────────────────────

/// Parameters for the custom-selection filter.
///
/// Faithful port of `CustomSelectionData` from VDF.GUI/Data/CustomSelectionData.cs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CustomSelectionData {
    /// Skip groups that already have a file selected for removal.
    pub ignore_groups_with_checked: bool,
    /// 0 = All, 1 = Videos only, 2 = Images only
    pub file_type: u8,
    /// 0 = Any, 1 = Exact match, 2 = Except size, 3 = Not identical
    pub identical: u8,
    /// 0 = Ignore, 1 = Newest, 2 = Oldest
    pub datetime: u8,
    pub min_size_mb: u64,
    pub max_size_mb: u64,
    /// Glob patterns — path must match ALL of these
    pub path_contains: Vec<String>,
    /// Glob patterns — path must not match ANY of these
    pub path_not_contains: Vec<String>,
    pub similarity_from: u8,
    pub similarity_to: u8,
}

impl Default for CustomSelectionData {
    fn default() -> Self {
        Self {
            ignore_groups_with_checked: true,
            file_type: 0,
            identical: 0,
            datetime: 0,
            min_size_mb: 0,
            max_size_mb: 999_999_999,
            path_contains: Vec::new(),
            path_not_contains: Vec::new(),
            similarity_from: 0,
            similarity_to: 100,
        }
    }
}

/// Named preset for CustomSelectionData, saved to settings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CustomSelectionPreset {
    pub name: String,
    pub data: CustomSelectionData,
}

/// Panel for advanced custom-selection of duplicates to remove.
///
/// Port of CustomSelectionView.xaml + CustomSelectionVM.cs.
#[component]
fn CustomSelectionPanel(mut app_state: Signal<AppState>) -> Element {
    let mut open = use_signal(|| false);

    if !*open.read() {
        return rsx! {
            button {
                class: "btn btn-xs btn-ghost",
                title: "Advanced filter: select files by type, size, path, similarity",
                onclick: move |_| open.set(true),
                "Custom selection…"
            }
        };
    }

    let mut data: Signal<CustomSelectionData> = use_signal(CustomSelectionData::default);
    let mut presets: Signal<Vec<CustomSelectionPreset>> = use_signal(Vec::new);
    let mut path_contains_input = use_signal(String::new);
    let mut path_not_contains_input = use_signal(String::new);
    let mut status = use_signal(|| String::new());

    rsx! {
        div { class: "custom-selection-panel panel",
            h3 { "Custom Selection" }

            // ── Preset bar ────────────────────────────────────────────────
            div { class: "preset-bar",
                label { "Preset:" }
                select {
                    onchange: {
                        let presets = presets.clone();
                        let mut data = data.clone();
                        move |e| {
                            let name = e.value();
                            if let Some(p) = presets.read().iter().find(|p| p.name == name) {
                                *data.write() = p.data.clone();
                            }
                        }
                    },
                    option { value: "", "— select —" }
                    for p in presets.read().iter() {
                        option { value: "{p.name}", "{p.name}" }
                    }
                }
                button {
                    class: "btn btn-xs btn-outline",
                    onclick: {
                        let data = data.clone();
                        let mut presets = presets.clone();
                        let mut status = status.clone();
                        move |_| {
                            let new_data = data.read().clone();
                            // Prompt would normally show an InputBox; use timestamp as name for web
                            let name = format!("preset_{}", presets.read().len() + 1);
                            presets.write().push(CustomSelectionPreset { name, data: new_data });
                            *status.write() = "Preset saved.".to_string();
                        }
                    },
                    "Save preset"
                }
            }

            // ── Options ───────────────────────────────────────────────────
            div { class: "cs-options",
                label { class: "cs-row",
                    input {
                        r#type: "checkbox",
                        checked: data.read().ignore_groups_with_checked,
                        onchange: {
                            let mut data = data.clone();
                            move |e| { data.write().ignore_groups_with_checked = e.checked(); }
                        },
                    }
                    " Ignore groups with already-selected files"
                }

                label { class: "cs-label", "File type:" }
                select {
                    value: "{data.read().file_type}",
                    onchange: {
                        let mut data = data.clone();
                        move |e| { data.write().file_type = e.value().parse().unwrap_or(0); }
                    },
                    option { value: "0", selected: data.read().file_type == 0, "All" }
                    option { value: "1", selected: data.read().file_type == 1, "Videos only" }
                    option { value: "2", selected: data.read().file_type == 2, "Images only" }
                }

                label { class: "cs-label", "Identical files:" }
                select {
                    value: "{data.read().identical}",
                    onchange: {
                        let mut data = data.clone();
                        move |e| { data.write().identical = e.value().parse().unwrap_or(0); }
                    },
                    option { value: "0", selected: data.read().identical == 0, "Any" }
                    option { value: "1", selected: data.read().identical == 1, "Exact (all metadata)" }
                    option { value: "2", selected: data.read().identical == 2, "Except size" }
                    option { value: "3", selected: data.read().identical == 3, "Not identical" }
                }

                label { class: "cs-label", "Keep by date:" }
                select {
                    value: "{data.read().datetime}",
                    onchange: {
                        let mut data = data.clone();
                        move |e| { data.write().datetime = e.value().parse().unwrap_or(0); }
                    },
                    option { value: "0", selected: data.read().datetime == 0, "Ignore (unordered)" }
                    option { value: "1", selected: data.read().datetime == 1, "Newest (keep)" }
                    option { value: "2", selected: data.read().datetime == 2, "Oldest (keep)" }
                }

                div { class: "cs-row-inline",
                    label { "Size (MB): " }
                    input {
                        r#type: "number",
                        class: "input-sm",
                        min: "0",
                        value: "{data.read().min_size_mb}",
                        oninput: {
                            let mut data = data.clone();
                            move |e| { data.write().min_size_mb = e.value().parse().unwrap_or(0); }
                        },
                    }
                    span { " – " }
                    input {
                        r#type: "number",
                        class: "input-sm",
                        min: "0",
                        value: "{data.read().max_size_mb}",
                        oninput: {
                            let mut data = data.clone();
                            move |e| { data.write().max_size_mb = e.value().parse().unwrap_or(999_999_999); }
                        },
                    }
                }

                div { class: "cs-row-inline",
                    label { "Similarity (%): " }
                    input {
                        r#type: "number",
                        class: "input-sm",
                        min: "0", max: "100",
                        value: "{data.read().similarity_from}",
                        oninput: {
                            let mut data = data.clone();
                            move |e| { data.write().similarity_from = e.value().parse().unwrap_or(0); }
                        },
                    }
                    span { " – " }
                    input {
                        r#type: "number",
                        class: "input-sm",
                        min: "0", max: "100",
                        value: "{data.read().similarity_to}",
                        oninput: {
                            let mut data = data.clone();
                            move |e| { data.write().similarity_to = e.value().parse().unwrap_or(100); }
                        },
                    }
                }

                // Path contains list
                label { class: "cs-label", "Path must match (wildcard):" }
                div { class: "cs-list-block",
                    ul {
                        for (idx, entry) in data.read().path_contains.iter().enumerate() {
                            li {
                                span { "{entry}" }
                                button {
                                    class: "btn btn-xs btn-ghost",
                                    onclick: {
                                        let mut data = data.clone();
                                        move |_| { data.write().path_contains.remove(idx); }
                                    },
                                    "✕"
                                }
                            }
                        }
                    }
                    div { class: "cs-add-row",
                        input {
                            r#type: "text",
                            class: "input-sm",
                            placeholder: "/path/prefix*",
                            value: "{path_contains_input}",
                            oninput: move |e| path_contains_input.set(e.value()),
                        }
                        button {
                            class: "btn btn-xs btn-outline",
                            onclick: {
                                let mut data = data.clone();
                                let mut inp = path_contains_input.clone();
                                move |_| {
                                    let v = inp.read().trim().to_string();
                                    if !v.is_empty() && !data.read().path_contains.contains(&v) {
                                        data.write().path_contains.push(v);
                                        inp.set(String::new());
                                    }
                                }
                            },
                            "Add"
                        }
                    }
                }

                // Path NOT contains list
                label { class: "cs-label", "Path must NOT match (wildcard):" }
                div { class: "cs-list-block",
                    ul {
                        for (idx, entry) in data.read().path_not_contains.iter().enumerate() {
                            li {
                                span { "{entry}" }
                                button {
                                    class: "btn btn-xs btn-ghost",
                                    onclick: {
                                        let mut data = data.clone();
                                        move |_| { data.write().path_not_contains.remove(idx); }
                                    },
                                    "✕"
                                }
                            }
                        }
                    }
                    div { class: "cs-add-row",
                        input {
                            r#type: "text",
                            class: "input-sm",
                            placeholder: "/path/to/skip*",
                            value: "{path_not_contains_input}",
                            oninput: move |e| path_not_contains_input.set(e.value()),
                        }
                        button {
                            class: "btn btn-xs btn-outline",
                            onclick: {
                                let mut data = data.clone();
                                let mut inp = path_not_contains_input.clone();
                                move |_| {
                                    let v = inp.read().trim().to_string();
                                    if !v.is_empty() && !data.read().path_not_contains.contains(&v) {
                                        data.write().path_not_contains.push(v);
                                        inp.set(String::new());
                                    }
                                }
                            },
                            "Add"
                        }
                    }
                }
            }

            // ── Status / actions ──────────────────────────────────────────
            if !status.read().is_empty() {
                p { class: "cs-status text-muted", "{status}" }
            }

            div { class: "panel-actions",
                button {
                    class: "btn btn-sm btn-primary",
                    onclick: {
                        let d = data.clone();
                        let mut state = app_state.clone();
                        let mut st = status.clone();
                        move |_| {
                            run_custom_selection(&d.read(), &mut state);
                            *st.write() = format!(
                                "{} files selected for removal.",
                                state.read().selected_for_action.len()
                            );
                        }
                    },
                    "Select"
                }
                button {
                    class: "btn btn-sm btn-ghost",
                    onclick: move |_| open.set(false),
                    "Close"
                }
                button {
                    class: "btn btn-sm btn-ghost",
                    title: "Reset all options to defaults",
                    onclick: {
                        let mut data = data.clone();
                        move |_| { *data.write() = CustomSelectionData::default(); }
                    },
                    "Reset"
                }
            }
        }
    }
}

/// Execute custom selection: marks files for removal in `app_state.selected_for_action`.
///
/// Direct port of `MainWindowVM.RunCustomSelection()` from VDF.GUI.
fn run_custom_selection(data: &CustomSelectionData, app_state: &mut Signal<AppState>) {
    #[cfg(feature = "server")]
    {
        use std::collections::HashSet;

        let mut to_remove: Vec<String> = Vec::new();
        let mut processed_clusters: HashSet<usize> = HashSet::new();

        // Build a set of cluster indices that already have selected files (if ignore option is on)
        let already_checked: HashSet<usize> = if data.ignore_groups_with_checked {
            let selected = &app_state.read().selected_for_action;
            app_state.read().clusters.iter().enumerate()
                .filter(|(_, c)| c.files.iter().any(|f| selected.contains(&f.id)))
                .map(|(i, _)| i)
                .collect()
        } else {
            HashSet::new()
        };

        let state = app_state.read();
        let sim_lo = data.similarity_from as f32 / 100.0;
        let sim_hi = data.similarity_to as f32 / 100.0;

        for (cidx, cluster) in state.clusters.iter().enumerate() {
            if already_checked.contains(&cidx) { continue; }
            if processed_clusters.contains(&cidx) { continue; }

            let max_sim = cluster.max_similarity;
            if max_sim < sim_lo || max_sim > sim_hi { continue; }

            // Filter files in the cluster by type, size, path patterns
            let filtered: Vec<&app_core::db::FileRecord> = cluster.files.iter().filter(|f| {
                // File type filter
                match data.file_type {
                    1 if f.is_image() => return false,  // videos only
                    2 if !f.is_image() => return false, // images only
                    _ => {}
                }
                // Size filter (bytes → MB)
                let mb = f.size_bytes / 1_048_576;
                if mb < data.min_size_mb || mb > data.max_size_mb { return false; }
                // Path contains (all must match)
                for pat in &data.path_contains {
                    if !glob_match(pat, f.path.as_str()) { return false; }
                }
                // Path not contains (none must match)
                for pat in &data.path_not_contains {
                    if glob_match(pat, f.path.as_str()) { return false; }
                }
                true
            }).collect();

            if filtered.len() < 2 { continue; }

            // Sort by identical criterion and datetime preference
            let mut sorted: Vec<&app_core::db::FileRecord> = filtered.clone();
            match data.datetime {
                1 => {
                    // Keep newest: sort ascending by mtime so index 0 is oldest (removed)
                    sorted.sort_by(|a, b| a.scanned_at.cmp(&b.scanned_at));
                }
                2 => {
                    // Keep oldest: sort descending so index 0 is newest (removed)
                    sorted.sort_by(|a, b| b.scanned_at.cmp(&a.scanned_at));
                }
                _ => {}
            }

            // Index 0 is kept; rest are selected for removal
            let keeper_id = sorted.first().map(|f| &f.id);
            for f in &sorted[1..] {
                to_remove.push(f.id.clone());
            }
            let _ = keeper_id; // already kept by skipping it

            processed_clusters.insert(cidx);
        }

        drop(state);
        app_state.write().selected_for_action = to_remove;
    }
}

/// Simple glob pattern match (supports `*` as wildcard only).
///
/// Mirrors `FileSystemName.MatchesSimpleExpression` from C#.
fn glob_match(pattern: &str, path: &str) -> bool {
    if !pattern.contains('*') && !pattern.contains('?') {
        return path.contains(pattern);
    }
    // Convert glob pattern to a simple prefix/suffix/contains check
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.is_empty() { return true; }
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() { continue; }
        if i == 0 {
            if !path.starts_with(part) { return false; }
            pos = part.len();
        } else if i == parts.len() - 1 {
            if !path.ends_with(part) { return false; }
        } else {
            match path[pos..].find(part) {
                Some(idx) => pos += idx + part.len(),
                None => return false,
            }
        }
    }
    true
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
    let fa = cluster.files.first().map(|f| f.id.clone()).unwrap_or_default();
    let fb = cluster.files.get(1).map(|f| f.id.clone()).unwrap_or_default();

    // Collect unique methods across all edges for badge display
    let mut seen_methods = std::collections::HashSet::new();
    let all_method_badges: Vec<String> = cluster.edges.iter()
        .map(|e| method_label(e))
        .filter(|m| seen_methods.insert(m.clone()))
        .collect();

    // Any edge is_flipped → show flipped badge
    let any_flipped = cluster.edges.iter().any(|e| {
        #[cfg(feature = "server")] { e.is_flipped }
        #[cfg(not(feature = "server"))] { false }
    });

    // Build match explanation lines from all edges
    let explanations: Vec<String> = cluster.edges.iter()
        .map(|e| build_explanation(e))
        .collect();

    // File rows: pre-build display tuples (id, name, path, dur, w, h, size, is_image)
    let file_rows: Vec<(String, String, String, f64, u32, u32, u64, bool)> = cluster.files.iter().map(|f| {
        (
            f.id.clone(),
            f.name.clone(),
            f.path.to_string(),
            f.duration_secs(),
            f.width().unwrap_or(0),
            f.height().unwrap_or(0),
            f.size_bytes,
            f.is_image(),
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
                // One badge per distinct detection method found across all edges
                for method in all_method_badges {
                    MethodBadge { method }
                }
                if any_flipped {
                    span { class: "badge badge-red", "Flipped" }
                }
                span { class: "file-count", "{file_count} files" }
            }

            div { class: "cluster-files",
                for (fid, name, path, dur, w, h, size, is_img) in file_rows {
                    div { class: "file-row",
                        // Thumbnail: shown for videos and images
                        {
                            let encoded = urlencoding::encode(&path).into_owned();
                            // Position at 25% of duration for a representative frame
                            let pos = if dur > 0.0 { dur * 0.25 } else { 0.0 };
                            if is_img {
                                rsx! {
                                    img {
                                        class: "file-thumb",
                                        src: "/api/thumbnail?path={encoded}&pos=0&w=200",
                                        alt: "{name}",
                                        loading: "lazy",
                                    }
                                }
                            } else if dur > 0.0 {
                                rsx! {
                                    img {
                                        class: "file-thumb",
                                        src: "/api/thumbnail?path={encoded}&pos={pos:.2}&w=200",
                                        alt: "{name}",
                                        loading: "lazy",
                                    }
                                }
                            } else {
                                rsx! { div { class: "file-thumb file-thumb-empty" } }
                            }
                        }
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

            // Match explanation lines — one per edge, showing method + evidence details
            if !explanations.is_empty() {
                div { class: "evidence-rows",
                    for explanation in explanations {
                        div { class: "evidence-row",
                            span { class: "evidence-label", "↳" }
                            span { class: "evidence-text", "{explanation}" }
                        }
                    }
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

/// Build a human-readable explanation line for a single duplicate edge.
///
/// Mirrors the evidence text shown in VDF.GUI's DuplicateItemVM tooltip and
/// VDF.Web's match row detail column.
fn build_explanation(edge: &DuplicatePair) -> String {
    let method = method_label(edge);
    let sim_pct = (edge.similarity * 100.0).round() as u32;

    let method_display = match method.as_str() {
        "FrameSimilarity"     => "Frame similarity",
        "IframeTimeline"      => "I-frame timeline",
        "AudioFingerprint"    => "Audio fingerprint",
        "Mpeg7Signature"      => "MPEG-7 signature",
        "SsimVerified"        => "SSIM verified",
        "TemporalAverageHash" => "Temporal avg hash",
        other                 => other,
    };

    let mut parts: Vec<String> = vec![method_display.to_string()];
    parts.push(format!("{sim_pct}% match"));

    #[cfg(feature = "server")]
    {
        if let Some(offset) = edge.clip_offset_secs {
            parts.push(format!("clip at {}", format_duration(offset)));
        }
        if let Some(audio_offset) = edge.audio_offset_secs {
            parts.push(format!("audio offset {audio_offset:.1}s"));
        }
        if let Some(frames) = edge.consecutive_frames {
            if frames > 0 {
                parts.push(format!("{frames} consecutive frames"));
            }
        }
        if edge.is_flipped {
            parts.push("horizontally mirrored".to_string());
        }
    }

    parts.join(" · ")
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

// ── SurrealQL expression selection ───────────────────────────────────────────

/// SurrealQL-based expression selector — port of ExpressionBuilder.xaml.
///
/// C# used DynamicExpresso (C# runtime eval) against in-memory DuplicateItem objects.
/// The Rust equivalent runs a SurrealQL WHERE clause directly against the database,
/// which is more powerful: full graph traversal, vector operators, functions.
///
/// Reference panel lists all queryable file fields.
#[component]
fn SurrealSelectionPanel(mut app_state: Signal<AppState>) -> Element {
    let mut open = use_signal(|| false);

    if !*open.read() {
        return rsx! {
            button {
                class: "btn btn-xs btn-ghost",
                title: "Select files using a SurrealQL WHERE expression",
                onclick: move |_| open.set(true),
                "SurrealQL select…"
            }
        };
    }

    let mut expr = use_signal(String::new);
    let mut presets: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let mut status = use_signal(String::new);
    let mut preset_name = use_signal(String::new);

    rsx! {
        div { class: "surreal-panel panel",
            h3 { "SurrealQL File Selection" }

            // ── Reference ─────────────────────────────────────────────────
            details { class: "field-ref",
                summary { "Available fields (click to expand)" }
                pre { class: "field-list",
"file.path              string   — full file path
file.name              string   — filename
file.size_bytes        int      — file size in bytes
file.is_image          bool     — true for images
file.scanned_at        int      — Unix epoch seconds
file.sha256            string?  — SHA-256 hex digest
file.phashes           object   — pHash arrays
file.audio_fingerprint array    — Chromaprint u32 array
file.iframe_phashes    array    — I-frame pHash values

Examples:
  size_bytes > 5000000
  is_image = false AND size_bytes < 1000000
  name CONTAINS 'copy'
  path STARTSWITH '/home/user/Videos'
  string::length(path) > 80"
                }
            }

            // ── Expression input ──────────────────────────────────────────
            div { class: "cs-options",
                label { class: "cs-label", "WHERE clause:" }
                textarea {
                    class: "input surreal-input",
                    rows: "3",
                    placeholder: "size_bytes > 5000000 AND is_image = false",
                    value: "{expr}",
                    oninput: move |e| expr.set(e.value()),
                }

                // Presets bar
                div { class: "preset-bar",
                    label { "Preset:" }
                    select {
                        onchange: {
                            let presets = presets.clone();
                            let mut ex = expr.clone();
                            move |e| {
                                let name = e.value();
                                if let Some((_, exp)) = presets.read().iter().find(|(n, _)| n == &name) {
                                    ex.set(exp.clone());
                                }
                            }
                        },
                        option { value: "", "— select —" }
                        for (name, _) in presets.read().iter() {
                            option { value: "{name}", "{name}" }
                        }
                    }
                    input {
                        r#type: "text",
                        class: "input-sm",
                        placeholder: "preset name",
                        value: "{preset_name}",
                        oninput: move |e| preset_name.set(e.value()),
                    }
                    button {
                        class: "btn btn-xs btn-outline",
                        onclick: {
                            let expr = expr.clone();
                            let mut presets = presets.clone();
                            let mut pname = preset_name.clone();
                            move |_| {
                                let name = pname.read().trim().to_string();
                                let exp = expr.read().trim().to_string();
                                if name.is_empty() || exp.is_empty() { return; }
                                let mut p = presets.write();
                                if let Some(existing) = p.iter_mut().find(|(n, _)| n == &name) {
                                    existing.1 = exp;
                                } else {
                                    p.push((name.clone(), exp));
                                }
                                pname.set(String::new());
                            }
                        },
                        "Save preset"
                    }
                }
            }

            if !status.read().is_empty() {
                p { class: "cs-status text-muted", "{status}" }
            }

            div { class: "panel-actions",
                button {
                    class: "btn btn-sm btn-primary",
                    disabled: expr.read().trim().is_empty(),
                    onclick: {
                        let ex = expr.clone();
                        let mut state = app_state.clone();
                        let mut st = status.clone();
                        move |_| {
                            let where_clause = ex.read().trim().to_string();
                            spawn(async move {
                                #[cfg(feature = "server")]
                                match run_surreal_select(where_clause).await {
                                    Ok(ids) => {
                                        let n = ids.len();
                                        state.write().selected_for_action = ids;
                                        *st.write() = format!("{n} files selected.");
                                    }
                                    Err(e) => { *st.write() = format!("Query error: {e}"); }
                                }
                            });
                        }
                    },
                    "Select matching files"
                }
                button {
                    class: "btn btn-sm btn-ghost",
                    onclick: move |_| open.set(false),
                    "Close"
                }
            }
        }
    }
}

/// Execute a SurrealQL WHERE clause against the file table.
///
/// Returns the IDs of all matching files. The UI then marks these as
/// `selected_for_action`, exactly as the C# CheckCustomCommand did.
#[cfg(feature = "server")]
async fn run_surreal_select(where_clause: String) -> Result<Vec<String>, String> {
    use app_core::db::{Database, ScanDatabase};
    use tokio::task::spawn_blocking;

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf")
        .join("db");

    spawn_blocking(move || -> Result<Vec<String>, String> {
        let db = ScanDatabase::open(&db_path).map_err(|e| e.to_string())?;
        let ids = db.query_file_ids_where(&where_clause).map_err(|e| e.to_string())?;
        Ok(ids)
    }).await.map_err(|e| e.to_string())?
}

// ── Quality criteria builder ──────────────────────────────────────────────────

/// Build a `Vec<Criterion>` respecting the user's chosen priority order.
///
/// Mirrors `ResolveCriteria()` from `MainWindowVM_Utils.cs`.
/// Unrecognised names are ignored; any criterion missing from the list is
/// appended at the end so newly-added criteria still act as tiebreakers.
#[cfg(feature = "server")]
fn build_criteria_from_order(order: &[String]) -> Vec<app_core::ranker::Criterion> {
    use app_core::ranker::Criterion;
    use app_core::db::FileRecord;

    fn make(name: &str) -> Option<Criterion> {
        match name {
            "Duration" => Some(Criterion::new("duration",
                |r: &FileRecord| r.duration_secs(), true)),
            "Resolution" => Some(Criterion::new("frame_area",
                |r: &FileRecord| {
                    let w = r.width().unwrap_or(0) as f64;
                    let h = r.height().unwrap_or(0) as f64;
                    w * h
                }, false)),
            "FPS" => Some(Criterion::new("fps",
                |r: &FileRecord| r.frame_rate().unwrap_or(0.0) as f64, true)),
            "Bitrate" => Some(Criterion::new("video_bitrate",
                |r: &FileRecord| r.video_bitrate_kbps().unwrap_or(0) as f64, true)),
            "Audio Bitrate" => Some(Criterion::new("audio_bitrate",
                |r: &FileRecord| r.audio_bitrate_kbps().unwrap_or(0) as f64, true)),
            "Size" => Some(Criterion::new("size_smallest",
                |r: &FileRecord| -(r.size_bytes as f64), false)),
            _ => None,
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut result: Vec<Criterion> = Vec::new();
    for name in order {
        if seen.contains(name.as_str()) { continue; }
        if let Some(c) = make(name) {
            seen.insert(name.clone());
            result.push(c);
        }
    }
    // Append any criteria not in the user's list as final tiebreakers
    for &name in ALL_CRITERIA {
        if !seen.contains(name) {
            if let Some(c) = make(name) {
                result.push(c);
            }
        }
    }
    result
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
