//! Settings view — all UiSettings fields exposed as form controls.

use dioxus::prelude::*;
use crate::settings::{FolderMatchMode, HardwareAccel, UiSettings};
use crate::state::ScanState;

#[component]
pub fn SettingsView() -> Element {
    let mut scan_state = use_context::<Signal<ScanState>>();

    rsx! {
        div { class: "view settings-view",
            h1 { "Settings" }

            form { class: "settings-form",
                // ── Similarity ────────────────────────────────────────────
                section { class: "settings-section",
                    h2 { "Matching" }

                    SliderField {
                        label: "Minimum similarity",
                        min: 0.5, max: 1.0, step: 0.01,
                        value: scan_state.read().settings.min_similarity,
                        display: format!("{:.0}%", scan_state.read().settings.min_similarity * 100.0),
                        onchange: move |v| scan_state.write().settings.min_similarity = v,
                    }

                    SliderField {
                        label: "Duration tolerance (%)",
                        min: 0.0, max: 100.0, step: 1.0,
                        value: scan_state.read().settings.percent_duration_difference as f32,
                        display: format!("{:.0}%", scan_state.read().settings.percent_duration_difference as f32),
                        onchange: move |v| scan_state.write().settings.percent_duration_difference = v as f64,
                    }

                    NumberField {
                        label: "Duration tolerance min (seconds)",
                        value: scan_state.read().settings.duration_diff_min_secs as f32,
                        min: 0.0,
                        onchange: move |v| scan_state.write().settings.duration_diff_min_secs = v as f64,
                    }

                    NumberField {
                        label: "Duration tolerance max (seconds, 0 = unlimited)",
                        value: scan_state.read().settings.duration_diff_max_secs as f32,
                        min: 0.0,
                        onchange: move |v| scan_state.write().settings.duration_diff_max_secs = v as f64,
                    }
                }

                // ── Fingerprinting ────────────────────────────────────────
                section { class: "settings-section",
                    h2 { "Fingerprinting" }

                    NumberField {
                        label: "Thumbnail frames per video",
                        value: scan_state.read().settings.thumbnail_count as f32,
                        min: 1.0,
                        onchange: move |v| scan_state.write().settings.thumbnail_count = v as usize,
                    }

                    CheckboxField {
                        label: "I-frame timeline fingerprint",
                        hint: "Detects partial clips and re-encodings. Slower.",
                        checked: scan_state.read().settings.iframe_fingerprint,
                        onchange: move |v| scan_state.write().settings.iframe_fingerprint = v,
                    }

                    CheckboxField {
                        label: "Audio fingerprint (Chromaprint)",
                        hint: "Matches by audio content regardless of video re-encoding.",
                        checked: scan_state.read().settings.partial_clip_detection,
                        onchange: move |v| scan_state.write().settings.partial_clip_detection = v,
                    }

                    if scan_state.read().settings.partial_clip_detection {
                        SliderField {
                            label: "Audio fingerprint minimum similarity",
                            min: 0.5, max: 1.0, step: 0.01,
                            value: scan_state.read().settings.partial_clip_min_similarity,
                            display: format!("{:.0}%", scan_state.read().settings.partial_clip_min_similarity * 100.0),
                            onchange: move |v| scan_state.write().settings.partial_clip_min_similarity = v,
                        }
                    }
                }

                // ── Scan scope ────────────────────────────────────────────
                section { class: "settings-section",
                    h2 { "Scan Scope" }

                    CheckboxField {
                        label: "Include images",
                        hint: "Also scan jpg, png, webp, gif, bmp, tiff files.",
                        checked: scan_state.read().settings.include_images,
                        onchange: move |v| scan_state.write().settings.include_images = v,
                    }

                    CheckboxField {
                        label: "Include sub-directories",
                        hint: "Recursively scan inside each folder.",
                        checked: scan_state.read().settings.include_sub_directories,
                        onchange: move |v| scan_state.write().settings.include_sub_directories = v,
                    }

                    label { class: "field-label", "Folder match mode" }
                    select {
                        class: "select",
                        onchange: move |e| {
                            scan_state.write().settings.folder_match_mode = match e.value().as_str() {
                                "same"      => FolderMatchMode::SameFolderOnly,
                                "different" => FolderMatchMode::DifferentFolderOnly,
                                _           => FolderMatchMode::None,
                            };
                        },
                        option {
                            value: "none",
                            selected: scan_state.read().settings.folder_match_mode == FolderMatchMode::None,
                            "All folders"
                        }
                        option {
                            value: "same",
                            selected: scan_state.read().settings.folder_match_mode == FolderMatchMode::SameFolderOnly,
                            "Same folder only"
                        }
                        option {
                            value: "different",
                            selected: scan_state.read().settings.folder_match_mode == FolderMatchMode::DifferentFolderOnly,
                            "Different folders only"
                        }
                    }
                }

                // ── MPEG-7 / SSIM ─────────────────────────────────────────
                section { class: "settings-section",
                    h2 { "Advanced Matching" }

                    CheckboxField {
                        label: "MPEG-7 video signature",
                        hint: "Low-level content signature — very accurate, requires FFmpeg mpeg7 build.",
                        checked: scan_state.read().settings.mpeg7_signature,
                        onchange: move |v| scan_state.write().settings.mpeg7_signature = v,
                    }

                    CheckboxField {
                        label: "SSIM second-pass verification",
                        hint: "Run structural similarity check on borderline pHash matches to reduce false positives.",
                        checked: scan_state.read().settings.ssim_verification,
                        onchange: move |v| scan_state.write().settings.ssim_verification = v,
                    }

                    if scan_state.read().settings.ssim_verification {
                        SliderField {
                            label: "SSIM re-check lower bound (sim ≥ this → verify)",
                            min: 0.5, max: 1.0, step: 0.01,
                            value: scan_state.read().settings.ssim_verify_min_sim,
                            display: format!("{:.0}%", scan_state.read().settings.ssim_verify_min_sim * 100.0),
                            onchange: move |v| scan_state.write().settings.ssim_verify_min_sim = v,
                        }
                        SliderField {
                            label: "SSIM re-check upper bound (sim ≤ this → verify)",
                            min: 0.5, max: 1.0, step: 0.01,
                            value: scan_state.read().settings.ssim_verify_max_sim,
                            display: format!("{:.0}%", scan_state.read().settings.ssim_verify_max_sim * 100.0),
                            onchange: move |v| scan_state.write().settings.ssim_verify_max_sim = v,
                        }
                        SliderField {
                            label: "SSIM reject threshold (below this → discard match)",
                            min: 0.0, max: 1.0, step: 0.01,
                            value: scan_state.read().settings.ssim_reject_threshold,
                            display: format!("{:.0}%", scan_state.read().settings.ssim_reject_threshold * 100.0),
                            onchange: move |v| scan_state.write().settings.ssim_reject_threshold = v,
                        }
                        NumberField {
                            label: "SSIM sample window (seconds)",
                            value: scan_state.read().settings.ssim_window_secs as f32,
                            min: 1.0,
                            onchange: move |v| scan_state.write().settings.ssim_window_secs = v as f64,
                        }
                    }
                }

                // ── Hardware acceleration ─────────────────────────────────
                section { class: "settings-section",
                    h2 { "Hardware Acceleration" }
                    p { class: "field-hint text-muted",
                        "Offloads video decoding to GPU. Requires appropriate FFmpeg build and drivers."
                    }
                    label { class: "field-label", "Decoder" }
                    select {
                        class: "select",
                        onchange: move |e| {
                            scan_state.write().settings.hardware_accel = match e.value().as_str() {
                                "vaapi"        => HardwareAccel::Vaapi,
                                "cuda"         => HardwareAccel::Cuda,
                                "videotoolbox" => HardwareAccel::VideoToolbox,
                                "d3d11va"      => HardwareAccel::D3d11va,
                                _              => HardwareAccel::None,
                            };
                        },
                        option { value: "none",         selected: scan_state.read().settings.hardware_accel == HardwareAccel::None,         "None (CPU)" }
                        option { value: "vaapi",        selected: scan_state.read().settings.hardware_accel == HardwareAccel::Vaapi,        "VA-API (Linux Intel/AMD)" }
                        option { value: "cuda",         selected: scan_state.read().settings.hardware_accel == HardwareAccel::Cuda,         "NVDEC / CUDA (NVIDIA)" }
                        option { value: "videotoolbox", selected: scan_state.read().settings.hardware_accel == HardwareAccel::VideoToolbox, "VideoToolbox (macOS)" }
                        option { value: "d3d11va",      selected: scan_state.read().settings.hardware_accel == HardwareAccel::D3d11va,      "D3D11VA (Windows)" }
                    }
                }

                // ── Skip start / end ──────────────────────────────────────
                section { class: "settings-section",
                    h2 { "Skip Start / End" }
                    p { class: "field-hint text-muted",
                        "Effective skip = max(seconds, duration × percent ÷ 100). \
                         Set either to 0 to ignore that dimension."
                    }

                    NumberField {
                        label: "Skip start (seconds)",
                        value: scan_state.read().settings.skip_start_secs as f32,
                        min: 0.0,
                        onchange: move |v| scan_state.write().settings.skip_start_secs = v as f64,
                    }

                    SliderField {
                        label: "Skip start (% of duration)",
                        min: 0.0, max: 50.0, step: 0.5,
                        value: scan_state.read().settings.skip_start_percent,
                        display: format!("{:.1}%", scan_state.read().settings.skip_start_percent),
                        onchange: move |v| scan_state.write().settings.skip_start_percent = v,
                    }

                    NumberField {
                        label: "Skip end (seconds)",
                        value: scan_state.read().settings.skip_end_secs as f32,
                        min: 0.0,
                        onchange: move |v| scan_state.write().settings.skip_end_secs = v as f64,
                    }

                    SliderField {
                        label: "Skip end (% of duration)",
                        min: 0.0, max: 50.0, step: 0.5,
                        value: scan_state.read().settings.skip_end_percent,
                        display: format!("{:.1}%", scan_state.read().settings.skip_end_percent),
                        onchange: move |v| scan_state.write().settings.skip_end_percent = v,
                    }
                }

                // ── Save / reset ──────────────────────────────────────────
                div { class: "settings-actions",
                    button {
                        class: "btn btn-primary",
                        r#type: "button",
                        onclick: move |_| save_settings(&scan_state.read().settings),
                        "Save"
                    }
                    button {
                        class: "btn btn-ghost",
                        r#type: "button",
                        onclick: move |_| scan_state.write().settings = UiSettings::default(),
                        "Reset to defaults"
                    }
                }
            }
        }
    }
}

// ── Field components ──────────────────────────────────────────────────────────

#[component]
fn SliderField(
    label: &'static str,
    min: f32,
    max: f32,
    step: f32,
    value: f32,
    display: String,
    onchange: EventHandler<f32>,
) -> Element {
    rsx! {
        div { class: "field",
            div { class: "field-header",
                label { class: "field-label", "{label}" }
                span { class: "field-value", "{display}" }
            }
            input {
                r#type: "range",
                min: "{min}", max: "{max}", step: "{step}",
                value: "{value}",
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<f32>() { onchange.call(v); }
                },
            }
        }
    }
}

#[component]
fn NumberField(
    label: &'static str,
    value: f32,
    min: f32,
    onchange: EventHandler<f32>,
) -> Element {
    rsx! {
        div { class: "field",
            label { class: "field-label", "{label}" }
            input {
                r#type: "number",
                min: "{min}",
                value: "{value}",
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<f32>() { onchange.call(v); }
                },
            }
        }
    }
}

#[component]
fn CheckboxField(
    label: &'static str,
    hint: &'static str,
    checked: bool,
    onchange: EventHandler<bool>,
) -> Element {
    rsx! {
        div { class: "field field-checkbox",
            label { class: "checkbox-label",
                input {
                    r#type: "checkbox",
                    checked,
                    onchange: move |e| onchange.call(e.checked()),
                }
                span { "{label}" }
            }
            if !hint.is_empty() {
                p { class: "field-hint text-muted", "{hint}" }
            }
        }
    }
}

// ── Persistence ───────────────────────────────────────────────────────────────

fn save_settings(settings: &UiSettings) {
    let dir = dirs::config_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("settings.json");
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(path, json);
    }
}
