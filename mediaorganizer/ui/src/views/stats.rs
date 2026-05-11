//! Stats view: summary of scanned files and duplicate groups.
//!
//! All statistics are derived from AppState::clusters which is loaded from
//! SurrealDB after a scan completes. No additional DB queries are needed.

use dioxus::prelude::*;
use crate::app::Route;
use crate::state::AppState;

#[component]
pub fn StatsView() -> Element {
    let app_state = use_context::<Signal<AppState>>();
    let clusters = app_state.read().clusters.clone();

    let group_count = clusters.len();
    let dup_file_count: usize = clusters.iter().map(|c| c.files.len()).sum();
    let dup_size_bytes: u64 = clusters
        .iter()
        .flat_map(|c| c.files.iter())
        .map(|f| f.size_bytes)
        .sum();

    // Wasted space = total bytes in clusters minus one "kept" copy per cluster
    // (the largest file in each cluster is assumed the keeper)
    let wasted_bytes: u64 = clusters.iter().map(|c| {
        let max = c.files.iter().map(|f| f.size_bytes).max().unwrap_or(0);
        let total: u64 = c.files.iter().map(|f| f.size_bytes).sum();
        total.saturating_sub(max)
    }).sum();

    let avg_sim = if group_count > 0 {
        clusters.iter().map(|c| c.max_similarity as f64).sum::<f64>() / group_count as f64
    } else {
        0.0
    };

    rsx! {
        div { class: "view stats-view",
            header { class: "stats-header",
                h1 { "Statistics" }
            }

            if clusters.is_empty() {
                div { class: "empty-state",
                    p { "No scan results yet." }
                    Link { to: Route::ScanView {},
                        button { class: "btn btn-primary", "Start a scan" }
                    }
                }
            } else {
                div { class: "stats-grid",
                    StatCard {
                        label: "Duplicate groups",
                        value: group_count.to_string(),
                        subtitle: "clusters of similar files"
                    }
                    StatCard {
                        label: "Duplicate files",
                        value: dup_file_count.to_string(),
                        subtitle: "files involved in duplicates"
                    }
                    StatCard {
                        label: "Duplicate storage",
                        value: format_bytes(dup_size_bytes),
                        subtitle: "total size of all duplicate files"
                    }
                    StatCard {
                        label: "Reclaimable space",
                        value: format_bytes(wasted_bytes),
                        subtitle: "estimated savings if duplicates removed"
                    }
                    StatCard {
                        label: "Avg. similarity",
                        value: format!("{:.1}%", avg_sim * 100.0),
                        subtitle: "average best match per group"
                    }
                }

                // Method breakdown
                section { class: "method-breakdown",
                    h2 { "By detection method" }
                    {method_breakdown(&clusters)}
                }

                div { class: "stats-actions",
                    Link { to: Route::ResultsView {},
                        button { class: "btn btn-primary", "View duplicates →" }
                    }
                }
            }
        }
    }
}

#[component]
fn StatCard(label: String, value: String, subtitle: String) -> Element {
    rsx! {
        div { class: "stat-card",
            div { class: "stat-value", "{value}" }
            div { class: "stat-label", "{label}" }
            div { class: "stat-subtitle text-muted", "{subtitle}" }
        }
    }
}

fn method_breakdown(clusters: &[crate::state::app_state::DuplicateCluster]) -> Element {
    use std::collections::HashMap;

    #[cfg(feature = "server")]
    let method_label = |method: &app_core::db::MatchMethod| format!("{:?}", method);

    let mut counts: HashMap<String, (usize, u64)> = HashMap::new();

    for cluster in clusters {
        for edge in &cluster.edges {
            #[cfg(feature = "server")]
            let label = method_label(&edge.method);
            #[cfg(not(feature = "server"))]
            let label = edge.method_str.clone();

            let size: u64 = cluster.files.iter().map(|f| f.size_bytes).sum();
            let entry = counts.entry(label).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += size;
        }
    }

    let mut rows: Vec<(String, usize, u64)> = counts
        .into_iter()
        .map(|(m, (c, s))| (m, c, s))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));

    rsx! {
        table { class: "method-table",
            thead {
                tr {
                    th { "Method" }
                    th { "Pairs" }
                    th { "Cluster size" }
                }
            }
            tbody {
                for (method, count, size) in rows {
                    tr {
                        td { "{method}" }
                        td { "{count}" }
                        td { "{format_bytes(size)}" }
                    }
                }
            }
        }
    }
}

fn format_bytes(bytes: u64) -> String {
    const MB: u64 = 1_048_576;
    const GB: u64 = 1_073_741_824;
    if bytes >= GB { format!("{:.1} GB", bytes as f64 / GB as f64) }
    else if bytes >= MB { format!("{:.0} MB", bytes as f64 / MB as f64) }
    else { format!("{} KB", bytes / 1024) }
}
