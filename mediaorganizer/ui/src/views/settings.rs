//! Settings view — all core::config::Settings fields exposed as form controls.

use dioxus::prelude::*;
use core::config::{FolderMatchMode, Settings};

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
                        display: format!("{:.0}%", scan_state.read().settings.percent_duration_difference),
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

                // ── Skip start / end ──────────────────────────────────────
                section { class: "settings-section",
                    h2 { "Skip Start / End" }

                    NumberField {
                        label: "Skip start (seconds)",
                        value: scan_state.read().settings.skip_start_secs as f32,
                        min: 0.0,
                        onchange: move |v| scan_state.write().settings.skip_start_secs = v as f64,
                    }

                    NumberField {
                        label: "Skip end (seconds)",
                        value: scan_state.read().settings.skip_end_secs as f32,
                        min: 0.0,
                        onchange: move |v| scan_state.write().settings.skip_end_secs = v as f64,
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
                        onclick: move |_| scan_state.write().settings = Settings::default(),
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

fn save_settings(settings: &Settings) {
    let dir = dirs::config_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("vdf");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("settings.json");
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(path, json);
    }
}
