//! Side-by-side file comparison view — ThumbnailComparerVM port.
//!
//! Implements the four compare modes from `ThumbnailComparerVM.cs`:
//!   Single, SideBySide, Swipe (slider overlay), Stacked (horizontal separator)
//! Plus independent frame-step controls for each file (ports StepA / StepB).

use dioxus::prelude::*;
use urlencoding;

use crate::app::Route;

// ── Match method ──────────────────────────────────────────────────────────────

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
            Self::FrameSimilarity     => "Frame pHash",
            Self::IframeTimeline      => "I-frame timeline",
            Self::AudioFingerprint    => "Chromaprint audio",
            Self::Mpeg7Signature      => "MPEG-7 signature",
            Self::SsimVerified        => "SSIM verified",
            Self::TemporalAverageHash => "Temporal average hash",
        }
    }
}

#[cfg(feature = "server")]
impl From<app_core::db::MatchMethod> for MatchMethodDisplay {
    fn from(m: app_core::db::MatchMethod) -> Self {
        use app_core::db::MatchMethod;
        match m {
            MatchMethod::FrameSimilarity     => Self::FrameSimilarity,
            MatchMethod::IframeTimeline      => Self::IframeTimeline,
            MatchMethod::AudioFingerprint    => Self::AudioFingerprint,
            MatchMethod::Mpeg7Signature      => Self::Mpeg7Signature,
            MatchMethod::SsimVerified        => Self::SsimVerified,
            MatchMethod::TemporalAverageHash => Self::TemporalAverageHash,
        }
    }
}

// ── Compare mode ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompareMode {
    #[default]
    SideBySide,
    Single,
    Swipe,
    Stacked,
    /// Pixel difference — bright pixels indicate per-channel differences.
    Diff,
}

impl CompareMode {
    const ALL: &'static [Self] = &[
        Self::SideBySide, Self::Single, Self::Swipe, Self::Stacked, Self::Diff,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::SideBySide => "Side by side",
            Self::Single     => "Single",
            Self::Swipe      => "Swipe",
            Self::Stacked    => "Stacked",
            Self::Diff       => "Pixel diff",
        }
    }
}

// ── DTO types — always serializable, populated by server fn ──────────────────

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FileInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub duration_secs: f64,
    pub fps: f32,
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    pub has_audio: bool,
    pub is_image: bool,
    /// Evenly-spaced thumbnail positions (secs) — 5 frames at 10%,27.5%,45%,62.5%,80% of duration.
    pub thumb_positions: Vec<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EdgeData {
    pub similarity: f32,
    pub method: String,
    pub clip_offset_secs: Option<f64>,
    pub phash_scores: Option<Vec<f32>>,
    pub consecutive_frames: Option<u32>,
}

// ── Root component ────────────────────────────────────────────────────────────

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
                    ThumbnailComparer {
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

// ── Top-level comparer (ports ThumbnailComparerVM) ────────────────────────────

const THUMB_COUNT: usize = 5;

#[component]
fn ThumbnailComparer(info_a: FileInfo, info_b: FileInfo, edge: EdgeData) -> Element {
    // ── mode & frame state ──────────────────────────────────────────────────
    let mut mode      = use_signal(|| CompareMode::default());
    let mut swipe_pct = use_signal(|| 50u32);   // 0-100 — position of the swipe separator
    let mut base_idx  = use_signal(|| 0usize);  // which thumbnail position we're viewing
    let mut step_a    = use_signal(|| 0i32);    // frame steps for A relative to base
    let mut step_b    = use_signal(|| 0i32);    // frame steps for B relative to base
    let mut zoom      = use_signal(|| 100u32);  // zoom level % (100 = fit)

    let max_base = info_a.thumb_positions.len().max(info_b.thumb_positions.len()).saturating_sub(1);
    let has_video = info_a.duration_secs > 0.0 || info_b.duration_secs > 0.0;

    // Compute the actual timestamp for a given file + base + step
    let pos_a = thumb_pos_with_step(&info_a, *base_idx.read(), *step_a.read());
    let pos_b = thumb_pos_with_step(&info_b, *base_idx.read(), *step_b.read());

    let zoom_style = format!("transform: scale({:.2}); transform-origin: top center;", *zoom.read() as f64 / 100.0);

    rsx! {
        div { class: "thumbnail-comparer",

            // ── toolbar ────────────────────────────────────────────────────
            div { class: "comparer-toolbar",
                // Mode selector
                div { class: "mode-select",
                    for m in CompareMode::ALL {
                        button {
                            class: if *mode.read() == *m { "btn btn-sm btn-primary" } else { "btn btn-sm btn-outline" },
                            onclick: {
                                let m = *m;
                                move |_| mode.set(m)
                            },
                            "{m.label()}"
                        }
                    }
                }

                // Zoom controls
                div { class: "zoom-controls",
                    button {
                        class: "btn btn-sm btn-ghost",
                        title: "Zoom out",
                        onclick: move |_| { let z = (*zoom.read()).saturating_sub(10).max(20); zoom.set(z); },
                        "−"
                    }
                    span { class: "zoom-label", "{zoom}%" }
                    button {
                        class: "btn btn-sm btn-ghost",
                        title: "Zoom in",
                        onclick: move |_| { let z = (*zoom.read() + 10).min(300); zoom.set(z); },
                        "+"
                    }
                    button {
                        class: "btn btn-sm btn-ghost",
                        title: "Fit to view",
                        onclick: move |_| zoom.set(100),
                        "Fit"
                    }
                }

                // Similarity badge
                div { class: "similarity-badge",
                    span { class: "sim-pct", "{(edge.similarity * 100.0).round() as u32}%" }
                    span { class: "sim-method text-muted", "{edge.method}" }
                }
            }

            // ── frame / thumbnail position controls ────────────────────────
            if has_video {
                div { class: "frame-controls",
                    // Base thumbnail position
                    if max_base > 0 {
                        div { class: "base-ctrl",
                            button {
                                class: "btn btn-xs btn-ghost",
                                disabled: *base_idx.read() == 0,
                                onclick: move |_| {
                                    let b = base_idx.read().saturating_sub(1);
                                    base_idx.set(b);
                                    step_a.set(0); step_b.set(0);
                                },
                                "◀"
                            }
                            span { class: "base-label",
                                "Position {*base_idx.read() + 1}/{max_base + 1}"
                            }
                            button {
                                class: "btn btn-xs btn-ghost",
                                disabled: *base_idx.read() >= max_base,
                                onclick: move |_| {
                                    let b = (*base_idx.read() + 1).min(max_base);
                                    base_idx.set(b);
                                    step_a.set(0); step_b.set(0);
                                },
                                "▶"
                            }
                        }
                    }

                    // Per-file frame steps
                    div { class: "step-ctrl",
                        span { class: "step-label", "A" }
                        button {
                            class: "btn btn-xs btn-ghost",
                            onclick: move |_| { let v = *step_a.read(); step_a.set(v - 1); },
                            "−"
                        }
                        span { class: "step-val",
                            {
                                let s = *step_a.read();
                                if s == 0 { "base".to_string() } else { format!("{s:+}") }
                            }
                        }
                        button {
                            class: "btn btn-xs btn-ghost",
                            onclick: move |_| { let v = *step_a.read(); step_a.set(v + 1); },
                            "+"
                        }

                        span { class: "step-sep" }

                        span { class: "step-label", "B" }
                        button {
                            class: "btn btn-xs btn-ghost",
                            onclick: move |_| { let v = *step_b.read(); step_b.set(v - 1); },
                            "−"
                        }
                        span { class: "step-val",
                            {
                                let s = *step_b.read();
                                if s == 0 { "base".to_string() } else { format!("{s:+}") }
                            }
                        }
                        button {
                            class: "btn btn-xs btn-ghost",
                            onclick: move |_| { let v = *step_b.read(); step_b.set(v + 1); },
                            "+"
                        }

                        button {
                            class: "btn btn-xs btn-ghost",
                            title: "Reset steps",
                            onclick: move |_| { step_a.set(0); step_b.set(0); },
                            "↺"
                        }
                    }
                }
            }

            // ── compare canvas ─────────────────────────────────────────────
            div { class: "comparer-canvas",
                div { style: "{zoom_style}",
                    match *mode.read() {
                        CompareMode::SideBySide => rsx! {
                            SideBySideView {
                                info_a: info_a.clone(),
                                info_b: info_b.clone(),
                                pos_a,
                                pos_b,
                            }
                        },
                        CompareMode::Single => rsx! {
                            SingleView {
                                info_a: info_a.clone(),
                                info_b: info_b.clone(),
                                pos_a,
                                pos_b,
                            }
                        },
                        CompareMode::Swipe => rsx! {
                            SwipeView {
                                info_a: info_a.clone(),
                                info_b: info_b.clone(),
                                pos_a,
                                pos_b,
                                swipe_pct: *swipe_pct.read(),
                                on_swipe: move |pct| swipe_pct.set(pct),
                            }
                        },
                        CompareMode::Stacked => rsx! {
                            StackedView {
                                info_a: info_a.clone(),
                                info_b: info_b.clone(),
                                pos_a,
                                pos_b,
                                split_pct: *swipe_pct.read(),
                                on_split: move |pct| swipe_pct.set(pct),
                            }
                        },
                        CompareMode::Diff => rsx! {
                            DiffView {
                                info_a: info_a.clone(),
                                info_b: info_b.clone(),
                                pos_a,
                                pos_b,
                            }
                        },
                    }
                }
            }

            // ── frame timestamp labels ─────────────────────────────────────
            if has_video {
                div { class: "frame-labels",
                    span { class: "frame-label-a",
                        "{info_a.name} — {format_timestamp(pos_a)}"
                        if *step_a.read() != 0 { " (step {step_a:+})" }
                    }
                    span { class: "frame-label-b",
                        "{info_b.name} — {format_timestamp(pos_b)}"
                        if *step_b.read() != 0 { " (step {step_b:+})" }
                    }
                }
            }

            // ── meta + evidence panels ─────────────────────────────────────
            div { class: "comparer-bottom",
                FileMetaCard { info: info_a.clone(), label: "File A" }
                EvidencePanel { edge: edge.clone(), info_a: info_a.clone(), info_b: info_b.clone() }
                FileMetaCard { info: info_b.clone(), label: "File B" }
            }
        }
    }
}

// ── Compare mode views ────────────────────────────────────────────────────────

#[component]
fn SideBySideView(info_a: FileInfo, info_b: FileInfo, pos_a: f64, pos_b: f64) -> Element {
    rsx! {
        div { class: "compare-side-by-side",
            CompareImage { info: info_a.clone(), pos: pos_a, label: "A" }
            CompareImage { info: info_b.clone(), pos: pos_b, label: "B" }
        }
    }
}

#[component]
fn SingleView(info_a: FileInfo, info_b: FileInfo, pos_a: f64, pos_b: f64) -> Element {
    let mut show_b = use_signal(|| false);
    rsx! {
        div { class: "compare-single",
            div { class: "single-toggle",
                button {
                    class: if !*show_b.read() { "btn btn-sm btn-primary" } else { "btn btn-sm btn-outline" },
                    onclick: move |_| show_b.set(false),
                    "A"
                }
                button {
                    class: if *show_b.read() { "btn btn-sm btn-primary" } else { "btn btn-sm btn-outline" },
                    onclick: move |_| show_b.set(true),
                    "B"
                }
            }
            if *show_b.read() {
                CompareImage { info: info_b.clone(), pos: pos_b, label: "B" }
            } else {
                CompareImage { info: info_a.clone(), pos: pos_a, label: "A" }
            }
        }
    }
}

/// Swipe mode — Image B is the base; Image A is clipped to [0, swipe_pct%] from the left.
/// A vertical separator line tracks the slider position.
#[component]
fn SwipeView(
    info_a: FileInfo,
    info_b: FileInfo,
    pos_a: f64,
    pos_b: f64,
    swipe_pct: u32,
    on_swipe: EventHandler<u32>,
) -> Element {
    let clip = format!("inset(0 {}% 0 0)", 100 - swipe_pct);
    rsx! {
        div { class: "compare-swipe",
            // Base image (B)
            div { class: "swipe-base",
                CompareImage { info: info_b.clone(), pos: pos_b, label: "B" }
            }
            // Overlay image (A) — clipped on the right
            div {
                class: "swipe-overlay",
                style: "clip-path: {clip};",
                CompareImage { info: info_a.clone(), pos: pos_a, label: "A" }
            }
            // Separator line
            div { class: "swipe-separator", style: "left: {swipe_pct}%;" }
            // Slider
            div { class: "swipe-slider-row",
                input {
                    r#type: "range",
                    min: "0", max: "100",
                    value: "{swipe_pct}",
                    class: "swipe-slider",
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<u32>() {
                            on_swipe.call(v);
                        }
                    }
                }
            }
        }
    }
}

/// Stacked mode — A is shown on top clipped to the top [split_pct%] of the canvas;
/// B fills the full canvas behind it. A horizontal separator line tracks the slider.
#[component]
fn StackedView(
    info_a: FileInfo,
    info_b: FileInfo,
    pos_a: f64,
    pos_b: f64,
    split_pct: u32,
    on_split: EventHandler<u32>,
) -> Element {
    let clip = format!("inset(0 0 {}% 0)", 100 - split_pct);
    rsx! {
        div { class: "compare-stacked",
            div { class: "stacked-base",
                CompareImage { info: info_b.clone(), pos: pos_b, label: "B" }
            }
            div {
                class: "stacked-overlay",
                style: "clip-path: {clip};",
                CompareImage { info: info_a.clone(), pos: pos_a, label: "A" }
            }
            div { class: "stacked-separator", style: "top: {split_pct}%;" }
            div { class: "swipe-slider-row stacked-slider-row",
                input {
                    r#type: "range",
                    min: "0", max: "100",
                    value: "{split_pct}",
                    class: "swipe-slider",
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<u32>() {
                            on_split.call(v);
                        }
                    }
                }
            }
        }
    }
}

// ── Image / video tile ────────────────────────────────────────────────────────

#[component]
fn CompareImage(info: FileInfo, pos: f64, label: &'static str) -> Element {
    let encoded = urlencoding::encode(&info.path).into_owned();
    let src = if info.is_image {
        format!("/api/video?path={encoded}")
    } else {
        format!("/api/thumbnail?path={encoded}&pos={pos:.3}&w=800")
    };

    rsx! {
        div { class: "compare-image-tile",
            div { class: "tile-label", "{label}" }
            img {
                class: "compare-img",
                src: "{src}",
                loading: "lazy",
                alt: "{info.name} at {pos:.1}s",
            }
        }
    }
}

// ── Pixel diff view ───────────────────────────────────────────────────────────

/// Overlay diff mode — shows absolute pixel difference between one frame from
/// each file. Bright pixels indicate large per-channel differences.
/// Uses the /api/diff_frame endpoint (FFmpeg blend=all_mode=difference filter).
/// Ports the ThumbnailComparerVM pixel-diff overlay from C# VDF.
#[component]
fn DiffView(info_a: FileInfo, info_b: FileInfo, pos_a: f64, pos_b: f64) -> Element {
    let enc_a = urlencoding::encode(&info_a.path).into_owned();
    let enc_b = urlencoding::encode(&info_b.path).into_owned();
    let src = format!(
        "/api/diff_frame?path_a={enc_a}&pos_a={pos_a:.3}&path_b={enc_b}&pos_b={pos_b:.3}&w=800"
    );

    rsx! {
        div { class: "compare-diff",
            div { class: "diff-legend",
                span { class: "diff-legend-label", "Pixel difference (black = identical, white = maximum difference)" }
            }
            div { class: "compare-image-tile",
                div { class: "tile-label", "A − B" }
                img {
                    class: "compare-img diff-img",
                    src: "{src}",
                    alt: "Pixel difference between A and B",
                    loading: "lazy",
                }
            }
            div { class: "diff-side-by-side",
                div { class: "diff-label-a", "A: {info_a.name} @ {format_timestamp(pos_a)}" }
                div { class: "diff-label-b", "B: {info_b.name} @ {format_timestamp(pos_b)}" }
            }
        }
    }
}

// ── File metadata card (bottom panels) ───────────────────────────────────────

#[component]
fn FileMetaCard(info: FileInfo, label: &'static str) -> Element {
    let encoded = urlencoding::encode(&info.path).into_owned();
    let video_url = format!("/api/video?path={encoded}");

    rsx! {
        div { class: "file-meta-card",
            div { class: "file-card-label", "{label}" }
            div { class: "file-name", "{info.name}" }
            div { class: "file-path text-muted", "{info.path}" }

            if info.is_image {
                img { class: "file-preview-image", src: "{video_url}", alt: "{info.name}" }
            } else if info.duration_secs > 0.0 {
                video {
                    class: "file-preview-video",
                    controls: true, preload: "metadata",
                    src: "{video_url}",
                }
            }

            dl { class: "meta-list",
                if info.duration_secs > 0.0 {
                    dt { "Duration" } dd { "{format_duration(info.duration_secs)}" }
                }
                if info.fps > 0.0 {
                    dt { "FPS" } dd { "{info.fps:.3}" }
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
                    title: "Open file",
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

// ── Evidence panel ────────────────────────────────────────────────────────────

#[component]
fn EvidencePanel(edge: EdgeData, info_a: FileInfo, info_b: FileInfo) -> Element {
    rsx! {
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
            QualityDiff { info_a, info_b }
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

// ── Quality diff panel (ports MainWindowVM_HoverDiff.cs) ──────────────────────

/// Shows a table of quality metric comparisons between the two files,
/// marking the winner with ✓ and computing % differences.
#[component]
fn QualityDiff(info_a: FileInfo, info_b: FileInfo) -> Element {
    // Resolution (total pixels)
    let px_a = info_a.width as u64 * info_a.height as u64;
    let px_b = info_b.width as u64 * info_b.height as u64;

    rsx! {
        div { class: "quality-diff",
            h3 { "Quality comparison" }
            table { class: "quality-table",
                thead {
                    tr {
                        th { "Metric" }
                        th { "File A" }
                        th { "File B" }
                        th { "Diff" }
                    }
                }
                tbody {
                    if info_a.duration_secs > 0.0 || info_b.duration_secs > 0.0 {
                        QualityRow {
                            metric: "Duration",
                            val_a: info_a.duration_secs,
                            val_b: info_b.duration_secs,
                            fmt_fn: |v| format_duration(v),
                            higher_is_better: true,
                        }
                    }
                    if px_a > 0 || px_b > 0 {
                        QualityRow {
                            metric: "Resolution",
                            val_a: px_a as f64,
                            val_b: px_b as f64,
                            fmt_fn: |_| String::new(),
                            higher_is_better: true,
                            // Override display — show "WxH" text
                        }
                    }
                    if info_a.fps > 0.0 || info_b.fps > 0.0 {
                        QualityRow {
                            metric: "FPS",
                            val_a: info_a.fps as f64,
                            val_b: info_b.fps as f64,
                            fmt_fn: |v| format!("{v:.3}"),
                            higher_is_better: true,
                        }
                    }
                    QualityRow {
                        metric: "File size",
                        val_a: info_a.size_bytes as f64,
                        val_b: info_b.size_bytes as f64,
                        fmt_fn: |v| format_bytes(v as u64),
                        higher_is_better: false,
                    }
                }
            }
        }
    }
}

#[component]
fn QualityRow(
    metric: &'static str,
    val_a: f64,
    val_b: f64,
    fmt_fn: fn(f64) -> String,
    higher_is_better: bool,
) -> Element {
    let a_wins = if higher_is_better { val_a >= val_b } else { val_a <= val_b };
    let b_wins = if higher_is_better { val_b >= val_a } else { val_b <= val_a };
    let pct_diff = if val_a != 0.0 && val_b != 0.0 {
        let best = if higher_is_better { val_a.max(val_b) } else { val_a.min(val_b) };
        if best > 0.0 {
            format_pct_diff((val_b - val_a) / val_a * 100.0)
        } else {
            "=".to_string()
        }
    } else {
        "n/a".to_string()
    };

    let display_a = if metric == "Resolution" {
        String::new() // overridden — caller sets this empty, we'll use special rendering
    } else {
        fmt_fn(val_a)
    };
    let display_b = if metric == "Resolution" {
        String::new()
    } else {
        fmt_fn(val_b)
    };

    rsx! {
        tr { class: "quality-row",
            td { class: "qmetric", "{metric}" }
            td { class: if a_wins { "qval winner" } else { "qval" },
                if metric == "Resolution" {
                    "{val_a as u64}"
                } else {
                    "{display_a}"
                }
                if a_wins && !b_wins { span { class: "win-mark", " ✓" } }
            }
            td { class: if b_wins { "qval winner" } else { "qval" },
                if metric == "Resolution" {
                    "{val_b as u64}"
                } else {
                    "{display_b}"
                }
                if b_wins && !a_wins { span { class: "win-mark", " ✓" } }
            }
            td { class: "qdiff text-muted", "{pct_diff}" }
        }
    }
}

// ── Duration comparison bar chart ─────────────────────────────────────────────

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
            h3 { "Duration" }
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

            let to_info = |r: app_core::db::FileRecord| {
                let dur = r.duration_secs();
                let thumb_positions = if dur > 0.0 {
                    (0..THUMB_COUNT)
                        .map(|i| dur * (0.10 + i as f64 * 0.175))
                        .collect()
                } else {
                    vec![]
                };
                let fps = r.video_streams.first()
                    .and_then(|s| s.fps.or(s.avg_fps))
                    .unwrap_or(0.0);

                FileInfo {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    path: r.path.to_string(),
                    size_bytes: r.size_bytes,
                    duration_secs: dur,
                    fps,
                    width: r.width().unwrap_or(0),
                    height: r.height().unwrap_or(0),
                    video_codec: r.video_streams.first()
                        .map(|s| s.codec_name.clone())
                        .unwrap_or_default(),
                    has_audio: r.has_audio(),
                    is_image: r.is_image(),
                    thumb_positions,
                }
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

/// Compute thumbnail position for a given file, base index, and frame step.
/// step is measured in frames at the file's fps. Clamps to [0, duration].
fn thumb_pos_with_step(info: &FileInfo, base_idx: usize, step: i32) -> f64 {
    if info.thumb_positions.is_empty() {
        return 0.0;
    }
    let base = info.thumb_positions.get(base_idx).copied()
        .unwrap_or(*info.thumb_positions.last().unwrap());

    if step == 0 || info.fps <= 0.0 {
        return base;
    }

    let pos = base + step as f64 / info.fps as f64;
    pos.clamp(0.0, info.duration_secs.max(base))
}

fn format_duration(secs: f64) -> String {
    let h = (secs / 3600.0) as u64;
    let m = ((secs % 3600.0) / 60.0) as u64;
    let s = (secs % 60.0) as u64;
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

fn format_timestamp(secs: f64) -> String {
    let h = (secs / 3600.0) as u64;
    let m = ((secs % 3600.0) / 60.0) as u64;
    let s = secs % 60.0;
    if h > 0 { format!("{h}:{m:02}:{s:05.2}") } else { format!("{m}:{s:05.2}") }
}

fn format_bytes(bytes: u64) -> String {
    const MB: u64 = 1_048_576;
    const GB: u64 = 1_073_741_824;
    if bytes >= GB { format!("{:.1} GB", bytes as f64 / GB as f64) }
    else if bytes >= MB { format!("{:.0} MB", bytes as f64 / MB as f64) }
    else { format!("{} KB", bytes / 1024) }
}

/// Format a signed percentage difference: "=", "+12%", "-5%"
fn format_pct_diff(pct: f64) -> String {
    if pct.abs() < 0.5 { "=".to_string() }
    else if pct > 0.0  { format!("+{:.0}%", pct) }
    else               { format!("{:.0}%", pct) }
}
