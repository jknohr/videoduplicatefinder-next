//! Server-side functions — compiled only when `--features web` is active.
//!
//! These run inside the Axum process. The Dioxus WASM client calls them via
//! generated fetch wrappers; the API is identical on both sides.
//!
//! Raw Axum routes (video streaming, HTTP 206) are registered by
//! `register_axum_routes()` and appended to the Dioxus fullstack router.

pub mod api;

/// Register raw Axum routes that can't be expressed as #[server] fns.
///
/// The video stream handler needs HTTP 206 / Range semantics that the JSON-
/// encoded #[server] RPC model doesn't support. Call this from main.rs when
/// building the Axum router for the web target.
#[cfg(feature = "web")]
pub fn register_axum_routes(router: axum::Router) -> axum::Router {
    use axum::routing::get;
    use api::video_stream_handler;

    router.route("/api/video", get(video_stream_handler))
}
