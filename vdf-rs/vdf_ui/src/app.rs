//! App root: global state providers + router.

use dioxus::prelude::*;

use crate::state::{AppState, ScanState};
use crate::views::{
    compare::CompareView, results::ResultsView, scan::ScanView, settings::SettingsView,
};

/// Top-level routes.
#[derive(Routable, Clone, PartialEq)]
pub enum Route {
    #[route("/")]
    ScanView {},
    #[route("/results")]
    ResultsView {},
    #[route("/compare/:file_a/:file_b")]
    CompareView { file_a: String, file_b: String },
    #[route("/settings")]
    SettingsView {},
}

/// App root: initialise global stores and mount the router.
#[component]
pub fn App() -> Element {
    // Provide global reactive state — components call use_context::<Signal<T>>() to read/write.
    use_context_provider(|| Signal::new(ScanState::default()));
    use_context_provider(|| Signal::new(AppState::default()));

    rsx! {
        Router::<Route> {}
    }
}

/// Persistent sidebar shown on every route.
#[component]
pub fn Sidebar() -> Element {
    rsx! {
        nav { class: "sidebar",
            NavLink { to: Route::ScanView {}, "Scan" }
            NavLink { to: Route::ResultsView {}, "Results" }
            NavLink { to: Route::SettingsView {}, "Settings" }
        }
    }
}

/// Thin wrapper that styles the active link.
#[component]
fn NavLink(to: Route, children: Element) -> Element {
    rsx! {
        Link { to, class: "nav-link", { children } }
    }
}
