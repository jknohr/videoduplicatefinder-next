//! Server-side functions — compiled only when `--features web` is active.
//!
//! These run inside the Axum process. The Dioxus WASM client calls them via
//! generated fetch wrappers; the API is identical on both sides.
//!
//! Raw Axum routes (video streaming, HTTP 206, auth) are registered by
//! `register_axum_routes()` and appended to the Dioxus fullstack router.

pub mod api;
#[cfg(feature = "server")]
pub mod auth;
#[cfg(feature = "server")]
pub mod ffmpeg_setup;

/// Register raw Axum routes that can't be expressed as #[server] fns.
///
/// Call this from main.rs when building the Axum router for the web target.
#[cfg(feature = "web")]
pub fn register_axum_routes(router: axum::Router) -> axum::Router {
    use axum::routing::{get, post};
    use axum::middleware;
    use api::{thumbnail_handler, video_stream_handler, diff_frame_handler};
    #[cfg(feature = "server")]
    use auth::{auth_middleware, login_page, login_submit};

    // Initialise auth and verify FFmpeg on first call to register_axum_routes
    #[cfg(feature = "server")]
    {
        auth::init_auth();
        ffmpeg_setup::check_ffmpeg();
    }

    let router = router
        .route("/api/video", get(video_stream_handler))
        .route("/api/thumbnail", get(thumbnail_handler))
        .route("/api/diff_frame", get(diff_frame_handler));

    #[cfg(feature = "server")]
    let router = router
        .route("/login", get(login_page))
        .route("/auth/login", post(login_submit))
        .layer(middleware::from_fn(auth_middleware));

    router
}
