//! Side-by-side file comparison view.
//!
//! Reads the full FileRecord + DuplicatePair evidence for the selected pair
//! directly from the SurrealDB graph and renders a rich evidence panel.

use dioxus::prelude::*;
use core::db::{FileRecord, MatchMethod, ScanDatabase};

use crate::app::Route;

#[component]
pub fn CompareView(file_a: String, file_b: String) -> Element {
    // Load both FileRecords from the database
    let pair_data = use_resource(move || {
        let fa = file_a.clone();
        let fb = file_b.clone();
        async move { load_pair(fa, fb).await }
    });

    rsx! {
        div { class: "view compare-view",
            Link { to: Route::ResultsView {}, class: "back-link", "← Back to results" }

            match &*pair_data.read() {
                Some(Ok((rec_a, rec_b, edge))) => rsx! {
                    ComparePanel { rec_a: rec_a.clone(), rec_b: rec_b.clone(), edge: edge.clone() }
                },
                Some(Err(e)) => rsx! {
                    div { class: "error", "Failed to load: {e}" }
                },
                None => rsx! {
                    div { class: "loading", "Loading…" }
                },
            }
        }
    }
}

// ── Evidence panel ────────────────────────────────────────────────────────────

#[component]
fn ComparePanel(
    rec_a: FileRecord,
    rec_b: FileRecord,
    edge: EdgeData,
) -> Element {
    rsx! {
        div { class: "compare-panel",
            // File cards side by side
            div { class: "compare-files",
                FileCard { record: rec_a.clone(), label: "File A" }
                div { class: "compare-divider",
                    span { class: "similarity-circle",
                        "{(edge.similarity * 100.0).round() as u32}%"
                    }
                }
                FileCard { record: rec_b.clone(), label: "File B" }
            }

            // Match evidence
            div { class: "evidence-panel",
                h2 { "Match Evidence" }

                div { class: "evidence-grid",
                    EvidenceRow { label: "Method",     value: method_label(edge.method) }
                    EvidenceRow { label: "Similarity", value: format!("{:.2}%", edge.similarity * 100.0) }

                    if let Some(offset) = edge.clip_offset_secs {
                        EvidenceRow {
                            label: "Clip offset",
                            value: format!("{offset:.2}s — File B begins {offset:.2}s into File A"),
                        }
                    }
                    if let Some(frames) = edge.consecutive_frames {
                        EvidenceRow { label: "Consecutive matching frames", value: frames.to_string() }
                    }
                }

                // pHash scores per timestamp
                if let Some(ref scores) = edge.phash_scores {
                    div { class: "phash-scores",
                        h3 { "Per-frame pHash scores" }
                        div { class: "score-bars",
                            for (i, score) in scores.iter().enumerate() {
                                ScoreBar { index: i, value: *score }
                            }
                        }
                    }
                }

                // Duration comparison
                DurationComparison { rec_a: rec_a.clone(), rec_b: rec_b.clone() }
            }
        }
    }
}

#[component]
fn FileCard(record: FileRecord, label: &'static str) -> Element {
    rsx! {
        div { class: "file-card",
            div { class: "file-card-label", "{label}" }
            div { class: "file-name", "{record.name}" }
            div { class: "file-path text-muted", "{record.path}" }

            if let Some(ref info) = record.media_info {
                dl { class: "meta-list",
                    dt { "Duration" }  dd { "{format_duration(info.duration_secs)}" }
                    dt { "Resolution" } dd { "{info.width}×{info.height}" }
                    dt { "Codec" }     dd { "{info.video_codec}" }
                    dt { "Audio" }     dd { if info.has_audio { "Yes" } else { "No" } }
                    dt { "Size" }      dd { "{format_bytes(record.size_bytes)}" }
                }
            }
        }
    }
}

#[component]
fn EvidenceRow(label: &'static str, value: String) -> Element {
    rsx! {
        div { class: "evidence-row",
            span { class: "evidence-label", "{label}" }
            span { class: "evidence-value", "{value}" }
        }
    }
}

#[component]
fn ScoreBar(index: usize, value: f32) -> Element {
    let pct = (value * 100.0).clamp(0.0, 100.0);
    let class = if pct >= 95.0 { "bar-exact" } else if pct >= 80.0 { "bar-high" } else { "bar-low" };
    rsx! {
        div { class: "score-bar {class}",
            div { class: "bar-fill", style: "width: {pct:.1}%" }
            span { class: "bar-label", "#{index}  {pct:.0}%" }
        }
    }
}

#[component]
fn DurationComparison(rec_a: FileRecord, rec_b: FileRecord) -> Element {
    let (dur_a, dur_b) = match (&rec_a.media_info, &rec_b.media_info) {
        (Some(a), Some(b)) => (a.duration_secs, b.duration_secs),
        _ => return rsx! {},
    };

    let longer  = dur_a.max(dur_b);
    let pct_a   = (dur_a / longer * 100.0) as u32;
    let pct_b   = (dur_b / longer * 100.0) as u32;
    let diff    = (dur_a - dur_b).abs();

    rsx! {
        div { class: "duration-comparison",
            h3 { "Duration comparison" }
            div { class: "dur-bar-row",
                span { class: "dur-label", "A" }
                div { class: "dur-bar",
                    div { class: "dur-fill", style: "width: {pct_a}%" }
                }
                span { class: "dur-value", "{format_duration(dur_a)}" }
            }
            div { class: "dur-bar-row",
                span { class: "dur-label", "B" }
                div { class: "dur-bar",
                    div { class: "dur-fill", style: "width: {pct_b}%" }
                }
                span { class: "dur-value", "{format_duration(dur_b)}" }
            }
            p { class: "dur-diff text-muted", "Difference: {format_duration(diff)}" }
        }
    }
}

// ── Data loading ──────────────────────────────────────────────────────────────

/// Edge data read back from the duplicate_of RELATE record.
#[derive(Debug, Clone)]
pub struct EdgeData {
    pub similarity: f32,
    pub method: MatchMethod,
    pub clip_offset_secs: Option<f64>,
    pub phash_scores: Option<Vec<f32>>,
    pub consecutive_frames: Option<i64>,
}

async fn load_pair(
    file_a: String,
    file_b: String,
) -> Result<(FileRecord, FileRecord, EdgeData), String> {
    tokio::task::spawn_blocking(move || {
        let db_path = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("vdf")
            .join("db");

        let db = ScanDatabase::open(&db_path).map_err(|e| e.to_string())?;

        let rec_a = db.get_file(&file_a)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("file not found: {file_a}"))?;
        let rec_b = db.get_file(&file_b)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("file not found: {file_b}"))?;

        // Find the specific duplicate_of edge between these two files
        let all = db.all_duplicates().map_err(|e| e.to_string())?;
        let pair = all.into_iter().find(|p| {
            (p.file_a == file_a && p.file_b == file_b)
            || (p.file_a == file_b && p.file_b == file_a)
        });

        let edge = pair.map(|p| EdgeData {
            similarity: p.similarity,
            method: p.method,
            clip_offset_secs: p.clip_offset_secs,
            phash_scores: None,       // TODO: load from extended duplicate_of fields
            consecutive_frames: None, // TODO: load from extended duplicate_of fields
        }).unwrap_or(EdgeData {
            similarity: 0.0,
            method: MatchMethod::FrameSimilarity,
            clip_offset_secs: None,
            phash_scores: None,
            consecutive_frames: None,
        });

        Ok((rec_a, rec_b, edge))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn method_label(method: MatchMethod) -> String {
    match method {
        MatchMethod::FrameSimilarity    => "Frame pHash".into(),
        MatchMethod::IframeTimeline     => "I-frame timeline".into(),
        MatchMethod::AudioFingerprint   => "Chromaprint audio".into(),
        MatchMethod::Mpeg7Signature     => "MPEG-7 signature".into(),
        MatchMethod::SsimVerified       => "SSIM verified".into(),
        MatchMethod::TemporalAverageHash => "Temporal average hash".into(),
    }
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
