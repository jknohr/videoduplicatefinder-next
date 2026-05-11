//! Results view: duplicate clusters loaded from the SurrealDB graph.
//!
//! Each cluster is a set of files connected by duplicate_of edges.
//! The graph traversal (union-find over edges) is done in AppState::load_clusters.

use dioxus::prelude::*;
use crate::app::Route;
use crate::state::AppState;
use crate::state::app_state::{DuplicateCluster, ResultSort};

#[cfg(feature = "server")]
use app_core::db::DuplicatePair;
#[cfg(not(feature = "server"))]
use crate::state::app_state::stubs::DuplicatePair;

#[component]
pub fn ResultsView() -> Element {
    let app_state = use_context::<Signal<AppState>>();
    let clusters = app_state.read().clusters.clone();

    rsx! {
        div { class: "view results-view",
            header { class: "results-header",
                h1 { "Duplicate Groups" }
                p { class: "subtitle",
                    "{clusters.len()} groups across {total_files(&clusters)} files"
                }
                ResultsToolbar { app_state }
            }

            if clusters.is_empty() {
                EmptyState {}
            } else {
                div { class: "cluster-list",
                    for cluster in clusters.iter() {
                        {cluster_card(cluster, app_state)}
                    }
                }
            }
        }
    }
}

fn total_files(clusters: &[DuplicateCluster]) -> usize {
    clusters.iter().map(|c| c.files.len()).sum()
}

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

    // File rows: pre-build display tuples
    let file_rows: Vec<(String, String, f64, u32, u32, u64)> = cluster.files.iter().map(|f| {
        (
            f.name.clone(),
            f.path.to_string(),
            f.duration_secs(),
            f.width().unwrap_or(0),
            f.height().unwrap_or(0),
            f.size_bytes,
        )
    }).collect();

    rsx! {
        div { class: "cluster-card",
            div { class: "cluster-header",
                SimilarityBadge { value: max_similarity }
                MethodBadge { method: method_str.clone() }
                span { class: "file-count", "{file_count} files" }
            }

            div { class: "cluster-files",
                for (name, path, dur, w, h, size) in file_rows {
                    div { class: "file-row",
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
                            "Compare side-by-side →"
                        }
                    }
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
