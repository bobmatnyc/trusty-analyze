//! Build script: ensures `ui/dist/` is present so the service crate can embed
//! it via `include_dir!`.
//!
//! Why: trusty-analyzer-ui is a vanilla JS + D3 dashboard with no build step.
//! The files in `ui/dist/` are authored directly and embedded into the Rust
//! binary at compile time. This build script's only job is to verify the
//! directory exists (and write a minimal placeholder if it doesn't, so that
//! `include_dir!` always has a tree to embed).
//! What: checks `ui/dist/index.html`; if missing, writes a placeholder.
//! Test: delete `ui/dist/` then run `cargo check` — build succeeds and a
//! placeholder `index.html` is written.

use std::{env, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist_dir = manifest.join("ui").join("dist");

    println!("cargo:rerun-if-changed=ui/dist");

    ensure_dist_placeholder(&dist_dir);
}

fn ensure_dist_placeholder(dist_dir: &PathBuf) {
    std::fs::create_dir_all(dist_dir).ok();
    let index = dist_dir.join("index.html");
    if !index.exists() {
        std::fs::write(
            &index,
            "<!DOCTYPE html><html><body><p>UI assets missing. Restore <code>ui/dist/index.html</code>.</p></body></html>",
        )
        .ok();
    }
}
