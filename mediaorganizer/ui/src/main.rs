//! Platform entry points for the UI crate.
//!
//! Feature flags select which runtime is linked:
//!   --features desktop  →  Dioxus Blitz GPU renderer (Wayland/Win/Mac)
//!   --features web      →  Dioxus WASM + Axum fullstack server
//!   --features mobile   →  Dioxus mobile (iOS / Android)
//!
//! All three targets compile the same component tree from app.rs / views/*.

mod app;
mod notify;
mod settings;
mod state;
mod views;

#[cfg(feature = "web")]
mod server;

#[allow(unused_imports)]
use app::App;

#[cfg(feature = "desktop")]
fn main() {
    init_logging();
    dioxus::LaunchBuilder::desktop().launch(App);
}

#[cfg(feature = "web")]
fn main() {
    // DX build tool creates two outputs from this single entry point:
    //   - WASM client bundle (server feature OFF — tokio not linked)
    //   - Axum server binary (server feature ON) that serves the bundle + #[server] fns
    #[cfg(feature = "server")]
    init_logging();
    dioxus::LaunchBuilder::new().launch(App);
}

#[cfg(feature = "mobile")]
fn main() {
    init_logging();
    dioxus::LaunchBuilder::mobile().launch(App);
}

// Prevent a confusing linker error when no feature is selected.
#[cfg(not(any(feature = "desktop", feature = "web", feature = "mobile")))]
fn main() {
    eprintln!("No target feature selected.");
    eprintln!("Build with one of: --features desktop | web | mobile");
    std::process::exit(1);
}

#[allow(dead_code)]
fn init_logging() {
    use tracing_subscriber::{EnvFilter, fmt};
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}
