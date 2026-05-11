//! Live log view — streams log entries from scan_state, mirrors VDF.Web/Log.razor.

use dioxus::prelude::*;
use crate::state::ScanState;
use crate::state::scan_state::LogLevel;

#[component]
pub fn LogsView() -> Element {
    let scan_state = use_context::<Signal<ScanState>>();
    let mut auto_scroll = use_signal(|| true);
    let mut level_filter = use_signal(|| LevelFilter::All);

    let entries: Vec<_> = scan_state.read().log_entries.clone();
    let filtered: Vec<_> = entries.iter()
        .filter(|e| match *level_filter.read() {
            LevelFilter::All   => true,
            LevelFilter::Info  => e.level == LogLevel::Info,
            LevelFilter::Warn  => e.level == LogLevel::Warn,
            LevelFilter::Error => e.level == LogLevel::Error,
        })
        .cloned()
        .collect();

    rsx! {
        div { class: "view logs-view",
            div { class: "logs-toolbar",
                h1 { class: "view-title", "Logs" }
                span { class: "log-count text-muted", "{filtered.len()} entries" }

                // Level filter buttons
                div { class: "btn-group",
                    button {
                        class: if *level_filter.read() == LevelFilter::All { "btn btn-sm btn-primary" } else { "btn btn-sm btn-ghost" },
                        onclick: move |_| *level_filter.write() = LevelFilter::All,
                        "All"
                    }
                    button {
                        class: if *level_filter.read() == LevelFilter::Info { "btn btn-sm btn-primary" } else { "btn btn-sm btn-ghost" },
                        onclick: move |_| *level_filter.write() = LevelFilter::Info,
                        "Info"
                    }
                    button {
                        class: if *level_filter.read() == LevelFilter::Warn { "btn btn-sm btn-primary" } else { "btn btn-sm btn-ghost" },
                        onclick: move |_| *level_filter.write() = LevelFilter::Warn,
                        "Warn"
                    }
                    button {
                        class: if *level_filter.read() == LevelFilter::Error { "btn btn-sm btn-primary" } else { "btn btn-sm btn-ghost" },
                        onclick: move |_| *level_filter.write() = LevelFilter::Error,
                        "Error"
                    }
                }

                label { class: "checkbox-label",
                    input {
                        r#type: "checkbox",
                        checked: *auto_scroll.read(),
                        onchange: move |e| *auto_scroll.write() = e.checked(),
                    }
                    span { "Auto-scroll" }
                }

                button {
                    class: "btn btn-sm btn-ghost",
                    onclick: move |_| {
                        use_context::<Signal<ScanState>>().write().log_entries.clear();
                    },
                    "Clear"
                }
            }

            div { class: "log-body",
                if filtered.is_empty() {
                    p { class: "empty-state text-muted", "No log entries yet. Run a scan to see output here." }
                } else {
                    for entry in &filtered {
                        div { class: match entry.level {
                            LogLevel::Error => "log-line log-error",
                            LogLevel::Warn  => "log-line log-warn",
                            LogLevel::Info  => "log-line log-info",
                        },
                            span { class: "log-level-badge",
                                match entry.level {
                                    LogLevel::Error => "ERR",
                                    LogLevel::Warn  => "WRN",
                                    LogLevel::Info  => "INF",
                                }
                            }
                            span { class: "log-message", "{entry.message}" }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LevelFilter {
    All,
    Info,
    Warn,
    Error,
}
