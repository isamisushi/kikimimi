//! architecture.md §8 (個人ビュー/ローカル): `guru agent`'s local web UI embeds
//! `web/dist` (the built SPA) into the binary via `rust-embed`
//! (`crates/cli/src/web.rs`). `rust-embed`'s derive macro walks that folder
//! *at compile time*, so it must exist even on a fresh checkout where
//! `cd web && npm run build` has never been run — otherwise `cargo build`
//! would hard-fail on a missing directory. That's explicitly not allowed
//! (task spec: "DO NOT make cargo build hard-fail when web/dist is absent").
//!
//! So: if `web/dist/index.html` is missing, write a minimal placeholder page
//! there instead of failing. `npm run build` (vite) empties `dist/` before
//! writing the real build, so this placeholder never lingers once someone
//! actually builds the SPA.

use std::fs;
use std::path::Path;

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    let index = dist.join("index.html");

    if !index.exists() {
        match fs::create_dir_all(&dist).and_then(|()| fs::write(&index, PLACEHOLDER_HTML)) {
            Ok(()) => println!(
                "cargo:warning=guru-cli: {} not found; embedding a placeholder page. \
                 Run `cd web && npm install && npm run build`, then rebuild guru-cli, \
                 to ship the real web UI.",
                index.display()
            ),
            Err(e) => println!(
                "cargo:warning=guru-cli: could not write placeholder {} ({e}); \
                 the local web UI will fail to compile unless web/dist exists.",
                index.display()
            ),
        }
    }

    // rust-embed's own derive-time directory walk already tracks this folder for
    // rebuilds; this is a belt-and-suspenders rerun trigger for the placeholder
    // logic above (cargo's default rerun heuristics don't cover a sibling crate's
    // directory like web/dist).
    println!("cargo:rerun-if-changed={}", dist.display());
}

const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>guru</title></head>
<body style="font-family: ui-monospace, monospace; white-space: pre-wrap; padding: 2rem;">
guru web UI is not built.

Run:

    cd web && npm install && npm run build

then rebuild guru (cargo build --release) to serve the real UI here.
</body>
</html>
"#;
