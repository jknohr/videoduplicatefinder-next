//! Side-by-side file comparison view.
//!
//! Reads the full FileRecord + DuplicatePair evidence for the selected pair
//! directly from the SurrealDB graph and renders a rich evidence panel.

use dioxus::prelude::*;
use urlencoding;

use crate::app::Route;

// ── Match method enum — always needed for display ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MatchMethodDisplay {
    #[default]
    FrameSimilarity,
    IframeTimeline,
    AudioFingerprint,
    Mpeg7Signature,
    SsimVerified,
    TemporalAverageHash,
}

impl MatchMethodDisplay {
    pub fn label(self) -> &'static str {
        match self {
            Self::FrameSimilarity    => "Frame pHash",
            Self::IframeTimeline     => "I-frame timeline",
            Self::AudioFingerprint   => "Chromaprint audio",
            Self::Mpeg7Signature     => "MPEG-7 signature",
            Self::SsimVerified       => "SSIM verified",
            Self::TemporalAverageHash => "Temporal average hash",
        }
    }
}

#[cfg(feature = "server")]
impl From<app_core::db::MatchMethod> for MatchMethodDisplay {
    fn from(m: app_core::db::MatchMethod) -> Self {
        use app_core::db::MatchMethod;
        match m {
            MatchMethod::FrameSimilarity    => Self::FrameSimilarity,
            MatchMethod::IframeTimeline     => Self::IframeTimeline,
            MatchMethod::AudioFingerprint   => Self::AudioFingerprint,
            MatchMethod::Mpeg7Signature     => Self::Mpeg7Signature,
            MatchMethod::SsimVerified       => Self::SsimVerified,
            MatchMethod::TemporalAverageHash => Self::TemporalAverageHash,
        }
    }
}

// ── View data types — always available, populated by server fn ────────────────

/// File metadata for display — serializable DTO that doesn't depend on core.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FileInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    pub has_audio: bool,
    pub is_image: bool,
}

/// Edge evidence data read from the duplicate_of RELATE record.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EdgeData {
    pub similarity: f32,
    pub method: String,
    pub clip_offset_secs: Option<f64>,
    pub phash_scores: Option<Vec<f32>>,
    pub consecutive_frames: Option<u32>,
}

// ── Component ─────────────────────────────────────────────────────────────────

#[component]
pub fn CompareView(file_a: String, file_b: String) -> Element {
    let pair_data = use_resource(move || {
        let fa = file_a.clone();
        let fb = file_b.clone();
        async move { load_pair(fa, fb).await }
    });

    rsx! {
        div { class: "view compare-view",
            Link { to: Route::ResultsView {}, class: "back-link", "← Back to results" }

            match &*pair_data.read() {
                Some(Ok((info_a, info_b, edge))) => rsx! {
                    ComparePanel {
                        info_a: info_a.clone(),
                        info_b: info_b.clone(),
                        edge: edge.clone(),
                    }
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
fn ComparePanel(info_a: FileInfo, info_b: FileInfo, edge: EdgeData) -> Element {
    rsx! {
        div { class: "compare-panel",
            div { class: "compare-files",
                FileCard { info: info_a.clone(), label: "File A" }
                div { class: "compare-divider",
                    span { class: "similarity-circle",
                        "{(edge.similarity * 100.0).round() as u32}%"
                    }
                }
                FileCard { info: info_b.clone(), label: "File B" }
            }

            div { class: "evidence-panel",
                h2 { "Match Evidence" }

                div { class: "evidence-grid",
                    EvidenceRow { label: "Method",     value: edge.method.clone() }
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

                DurationComparison { info_a: info_a.clone(), info_b: info_b.clone() }
            }
        }
    }
}

#[component]
fn FileCard(info: FileInfo, label: &'static str) -> Element {
    // Build URL for in-browser playback via the /api/video Axum handler.
    // For desktop the native renderer will handle local paths; for web
    // the HTTP server streams bytes with Range support.
    let encoded_path = urlencoding::encode(&info.path).into_owned();
    let video_url = format!("/api/video?path={encoded_path}");

    rsx! {
        div { class: "file-card",
            div { class: "file-card-label", "{label}" }
            div { class: "file-name", "{info.name}" }
            div { class: "file-path text-muted", "{info.path}" }

            // In-browser video/image preview
            if info.is_image {
                img {
                    class: "file-preview-image",
                    src: "{video_url}",
                    alt: "{info.name}",
                }
            } else if info.duration_secs > 0.0 {
                video {
                    class: "file-preview-video",
                    controls: true,
                    preload: "metadata",
                    src: "{video_url}",
                }
                // Multi-thumbnail strip: 5 evenly-spaced frames (port of ThumbnailComparer strips)
                ThumbnailStrip { path: info.path.clone(), duration_secs: info.duration_secs }
            }

            dl { class: "meta-list",
                if info.duration_secs > 0.0 {
                    dt { "Duration" } dd { "{format_duration(info.duration_secs)}" }
                }
                if info.width > 0 {
                    dt { "Resolution" } dd { "{info.width}×{info.height}" }
                }
                if !info.video_codec.is_empty() {
                    dt { "Codec" } dd { "{info.video_codec}" }
                }
                dt { "Audio" } dd { if info.has_audio { "Yes" } else { "No" } }
                dt { "Size" } dd { "{format_bytes(info.size_bytes)}" }
            }

            div { class: "file-card-actions",
                button {
                    class: "btn btn-xs btn-ghost",
                    title: "Open file with default application",
                    onclick: {
                        let path = info.path.clone();
                        move |_| crate::shell::open_path(&path)
                    },
                    "Open"
                }
                button {
                    class: "btn btn-xs btn-ghost",
                    title: "Reveal in file manager",
                    onclick: {
                        let path = info.path.clone();
                        move |_| crate::shell::reveal_in_folder(&path)
                    },
                    "Reveal"
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
fn DurationComparison(info_a: FileInfo, info_b: FileInfo) -> Element {
    let dur_a = info_a.duration_secs;
    let dur_b = info_b.duration_secs;
    if dur_a <= 0.0 || dur_b <= 0.0 {
        return rsx! {};
    }

    let longer = dur_a.max(dur_b);
    let pct_a  = (dur_a / longer * 100.0) as u32;
    let pct_b  = (dur_b / longer * 100.0) as u32;
    let diff   = (dur_a - dur_b).abs();

    rsx! {
        div { class: "duration-comparison",
            h3 { "Duration comparison" }
            div { class: "dur-bar-row",
                span { class: "dur-label", "A" }
                div { class: "dur-bar", div { class: "dur-fill", style: "width: {pct_a}%" } }
                span { class: "dur-value", "{format_duration(dur_a)}" }
            }
            div { class: "dur-bar-row",
                span { class: "dur-label", "B" }
                div { class: "dur-bar", div { class: "dur-fill", style: "width: {pct_b}%" } }
                span { class: "dur-value", "{format_duration(dur_b)}" }
            }
            p { class: "dur-diff text-muted", "Difference: {format_duration(diff)}" }
        }
    }
}

// ── Thumbnail strip ───────────────────────────────────────────────────────────

/// Renders N evenly-spaced thumbnails extracted from a video.
///
/// Mirrors the thumbnail carousel in ThumbnailComparer.xaml.
/// Uses the /api/thumbnail endpoint with different `pos` values.
const THUMB_COUNT: usize = 5;

#[component]
fn ThumbnailStrip(path: String, duration_secs: f64) -> Element {
    let encoded = urlencoding::encode(&path).into_owned();
    // Positions: 10%, 27.5%, 45%, 62.5%, 80% of duration
    let positions: Vec<f64> = (0..THUMB_COUNT)
        .map(|i| duration_secs * (0.10 + i as f64 * 0.175))
        .collect();

    rsx! {
        div { class: "thumb-strip",
            for pos in positions {
                img {
                    class: "thumb-strip-img",
                    src: "/api/thumbnail?path={encoded}&pos={pos:.2}&w=160",
                    loading: "lazy",
                    alt: "{pos:.1}s",
                    title: "{pos:.1}s",
                }
            }
        }
    }
}

// ── Data loading ──────────────────────────────────────────────────────────────

async fn load_pair(
    file_a: String,
    file_b: String,
) -> Result<(FileInfo, FileInfo, EdgeData), String> {
    #[cfg(not(feature = "server"))]
    {
        let _ = (file_a, file_b);
        Err("server feature required".to_string())
    }

    #[cfg(feature = "server")]
    {
        use app_core::db::{Database, ScanDatabase};

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

            let to_info = |r: app_core::db::FileRecord| FileInfo {
                id: r.id.clone(),
                name: r.name.clone(),
                path: r.path.to_string(),
                size_bytes: r.size_bytes,
                duration_secs: r.duration_secs(),
                width: r.width().unwrap_or(0),
                height: r.height().unwrap_or(0),
                video_codec: r.video_streams.first()
                    .map(|s| s.codec_name.clone())
                    .unwrap_or_default(),
                has_audio: r.has_audio(),
                is_image: r.is_image(),
            };

            let info_a = to_info(rec_a);
            let info_b = to_info(rec_b);

            let all = db.all_duplicates().map_err(|e| e.to_string())?;
            let pair = all.into_iter().find(|p| {
                (p.file_a == file_a && p.file_b == file_b)
                || (p.file_a == file_b && p.file_b == file_a)
            });

            let edge = pair.map(|p| {
                let method = MatchMethodDisplay::from(p.method);
                EdgeData {
                    similarity: p.similarity,
                    method: method.label().to_string(),
                    clip_offset_secs: p.clip_offset_secs,
                    phash_scores: if p.phash_scores.is_empty() { None } else { Some(p.phash_scores) },
                    consecutive_frames: p.consecutive_frames,
                }
            }).unwrap_or_default();

            Ok((info_a, info_b, edge))
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

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
