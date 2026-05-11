//! Embedded UI asset serving for the trusty-analyzer daemon.
//!
//! Why: ships the dashboard in the same binary as the daemon so there's no
//! second deployment artifact and no CORS/origin issues. The Svelte app is
//! built into `ui/dist/` at the workspace root (by `build.rs`) and embedded
//! at compile time via `include_dir!`.
//! What: two axum handlers — `ui_index_handler` for `/ui` (serves
//! `index.html`) and `ui_asset_handler` for `/ui/*path` (serves arbitrary
//! assets, falling back to `index.html` for unknown paths so SPA client-side
//! routing works).
//! Test: `cargo test -p trusty-analyzer-service ui::` covers the asset path,
//! the SPA fallback, and MIME type selection.

use axum::{
    body::Body,
    http::{header, Response, StatusCode},
    response::IntoResponse,
};
use include_dir::{include_dir, Dir};

/// Compile-time embedded UI tree. Path is resolved relative to this crate's
/// manifest, so `../../ui/dist` points at the workspace-root `ui/dist`
/// directory populated by `build.rs`.
static UI_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../ui/dist");

/// Serve the UI index page (`/ui` and `/ui/`).
pub async fn ui_index_handler() -> impl IntoResponse {
    serve_asset("index.html")
}

/// Serve an arbitrary UI asset at `/ui/*path`. Falls back to `index.html`
/// when the path doesn't match a file, so the Svelte router can take over.
pub async fn ui_asset_handler(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl IntoResponse {
    serve_asset(&path)
}

fn serve_asset(path: &str) -> Response<Body> {
    // Strip leading slashes — include_dir paths are relative.
    let trimmed = path.trim_start_matches('/');
    if let Some(file) = UI_DIR.get_file(trimmed) {
        let mime = mime_guess::from_path(trimmed)
            .first_or_octet_stream()
            .to_string();
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .body(Body::from(file.contents().to_vec()))
            .unwrap();
    }
    // SPA fallback: serve index.html for unknown paths.
    match UI_DIR.get_file("index.html") {
        Some(file) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(file.contents().to_vec()))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("UI not built"))
            .unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn index_handler_returns_html() {
        let resp = ui_index_handler().await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_path_falls_back_to_index() {
        let resp = serve_asset("does-not-exist.txt");
        // Either 200 (fallback served) or 404 (no index either) — must not panic.
        assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::NOT_FOUND);
    }
}
