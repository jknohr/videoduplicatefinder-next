//! Results view: duplicate clusters loaded from the SurrealDB graph.
//!
//! Each cluster is a set of files connected by duplicate_of edges.
//! The graph traversal (union-find over edges) is done in AppState::load_clusters.

use dioxus::prelude::*;
use vdf_core::db::MatchMethod;

use crate::app::Route;
use crate::state::AppState;
use crate::state::app_state::{DuplicateCluster, ResultSort};

#[component]
pub fn ResultsView() -> Element {
    let mut app_state = use_context::<Signal<AppState>>();
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
                    for (idx, cluster) in clusters.iter().enumerate() {
                        ClusterCard { key: "{idx}", cluster: cluster.clone(), app_state }
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

#[component]
fn ClusterCard(cluster: DuplicateCluster, mut app_state: Signal<AppState>) -> Element {
    let method_filter = app_state.read().method_filter.clone();
    let best_edge = cluster.edges.iter()
        .max_by(|a, b| a.similarity.partial_cmp(&b.similarity).unwrap_or(std::cmp::Ordering::Equal))
        .cloned();

    let Some(edge) = best_edge else { return rsx! {} };

    // Skip if method filter is active and doesn't match
    if let Some(ref filter) = method_filter {
        if format!("{:?}", edge.method) != *filter {
            return rsx! {};
        }
    }

    rsx! {
        div { class: "cluster-card",
            // Header: similarity badge + method badge + file count
            div { class: "cluster-header",
                SimilarityBadge { value: cluster.max_similarity }
                MethodBadge { method: edge.method }
                span { class: "file-count", "{cluster.files.len()} files" }
            }

            // File list
            div { class: "cluster-files",
                for file in cluster.files.iter() {
                    div { class: "file-row",
                        div { class: "file-name", "{file.name}" }
                        div { class: "file-meta",
                            if let Some(ref info) = file.media_info {
                                span { class: "tag", "{format_duration(info.duration_secs)}" }
                                span { class: "tag", "{info.width}×{info.height}" }
                            }
                            span { class: "tag", "{format_bytes(file.size_bytes)}" }
                        }
                        div { class: "file-path text-muted", "{file.path}" }
                    }
                }
            }

            // Evidence from the duplicate_of edge
            if let Some(offset) = edge.clip_offset_secs {
                div { class: "evidence-row",
                    span { class: "evidence-label", "Clip offset:" }
                    span { "{offset:.1}s into the longer file" }
                }
            }

            // Actions
            div { class: "cluster-actions",
                if cluster.files.len() == 2 {
                    button {
                        class: "btn btn-sm btn-outline",
                        onclick: {
                            let fa = cluster.files[0].id.clone();
                            let fb = cluster.files[1].id.clone();
                            move |_| { app_state.write().selected_pair = Some((fa.clone(), fb.clone())); }
                        },
                        Link {
                            to: Route::CompareView {
                                file_a: cluster.files[0].id.clone(),
                                file_b: cluster.files[1].id.clone(),
                            },
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
fn MethodBadge(method: MatchMethod) -> Element {
    let (label, class) = match method {
        MatchMethod::FrameSimilarity    => ("Frame hash",         "badge badge-blue"),
        MatchMethod::IframeTimeline     => ("I-frame timeline",   "badge badge-purple"),
        MatchMethod::AudioFingerprint   => ("Audio fingerprint",  "badge badge-green"),
        MatchMethod::Mpeg7Signature     => ("MPEG-7",             "badge badge-orange"),
        MatchMethod::SsimVerified       => ("SSIM verified",      "badge badge-teal"),
        MatchMethod::TemporalAverageHash => ("Temporal avg",      "badge badge-gray"),
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
