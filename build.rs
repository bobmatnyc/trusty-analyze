//! Build script: compiles the Svelte UI into `ui/dist/` so the service crate
//! can embed it via `include_dir!`.
//!
//! Why: keeps the daemon fully self-contained — `cargo build` produces a
//! single binary with the UI baked in. No separate build step required for
//! the common case. Set `SKIP_UI_BUILD=1` during Rust-only development or CI
//! steps that don't need the UI rebuilt.
//! What: runs `pnpm install` (if `node_modules` is missing) and `pnpm build`
//! inside `ui/`. On failure (missing pnpm, missing package.json, build error)
//! writes a placeholder `ui/dist/index.html` so `include_dir!` still has a
//! tree to embed.
//! Test: run `SKIP_UI_BUILD=1 cargo check` from the workspace root — should
//! succeed even without pnpm installed, producing the placeholder.

use std::{env, path::PathBuf, process::Command};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ui_dir = manifest.join("ui");
    let dist_dir = ui_dir.join("dist");

    println!("cargo:rerun-if-changed=ui/src");
    println!("cargo:rerun-if-changed=ui/package.json");
    println!("cargo:rerun-if-changed=ui/vite.config.js");
    println!("cargo:rerun-if-changed=ui/index.html");
    println!("cargo:rerun-if-env-changed=SKIP_UI_BUILD");

    if env::var("SKIP_UI_BUILD").as_deref() == Ok("1") {
        ensure_dist_placeholder(&dist_dir);
        return;
    }

    if !ui_dir.join("package.json").exists() {
        ensure_dist_placeholder(&dist_dir);
        return;
    }

    // Run pnpm install if node_modules missing.
    if !ui_dir.join("node_modules").exists() {
        let status = Command::new("pnpm")
            .args(["install", "--frozen-lockfile"])
            .current_dir(&ui_dir)
            .status();
        if status.map(|s| !s.success()).unwrap_or(true) {
            ensure_dist_placeholder(&dist_dir);
            return;
        }
    }

    // Run pnpm build.
    let status = Command::new("pnpm")
        .arg("build")
        .current_dir(&ui_dir)
        .status();

    if status.map(|s| !s.success()).unwrap_or(true) {
        ensure_dist_placeholder(&dist_dir);
    }
}

fn ensure_dist_placeholder(dist_dir: &PathBuf) {
    std::fs::create_dir_all(dist_dir).ok();
    let index = dist_dir.join("index.html");
    if !index.exists() {
        std::fs::write(
            &index,
            "<!DOCTYPE html><html><body><p>UI not built. Run <code>pnpm build</code> in ui/.</p></body></html>",
        )
        .ok();
    }
}
