//! Server-side functions — compiled only when `--features web` is active.
//!
//! These run inside the Axum process. The Dioxus WASM client calls them via
//! generated fetch wrappers; the API is identical on both sides.

pub mod api;
