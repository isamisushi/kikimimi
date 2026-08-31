//! WEB API CONTRACT ("Serve the SPA"): guru-cloud embeds `web/dist` (the
//! built SPA) into the binary via `rust-embed` (`crates/cloud/src/web.rs`),
//! same approach as `guru agent`'s local web UI
//! (`crates/cli/src/web.rs` / `crates/cli/build.rs`, which this file
//! mirrors — no shared crate to hang one copy of this off, see that file's
//! doc comment). `rust-embed`'s derive macro walks that folder *at compile
//! time*, so it must exist even on a fresh checkout where
//! `cd web && npm run build` has never been run — otherwise `cargo build`
//! would hard-fail on a missing directory, same gotcha `crates/cli/build.rs`
//! documents.
//!
//! So: if `web/dist/index.html` is missing, write a minimal placeholder page
//! there instead of failing. `npm run build` (vite) empties `dist/` before
//! writing the real build, so this placeholder never lingers once someone
//! actually builds the SPA. Idempotent with `crates/cli/build.rs`'s own copy
//! of this logic (both point at the same `web/dist`, both only write when
//! `index.html` is missing) -- running both crates' build scripts in the
//! same `cargo build` is safe regardless of ordering.

use std::fs;
use std::path::Path;

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    let index = dist.join("index.html");

    if !index.exists() {
        match fs::create_dir_all(&dist).and_then(|()| fs::write(&index, PLACEHOLDER_HTML)) {
            Ok(()) => println!(
                "cargo:warning=guru-cloud: {} not found; embedding a placeholder page. \
                 Run `cd web && npm install && npm run build`, then rebuild guru-cloud, \
                 to ship the real web UI.",
                index.display()
            ),
            Err(e) => println!(
                "cargo:warning=guru-cloud: could not write placeholder {} ({e}); \
                 the hosted web UI will fail to compile unless web/dist exists.",
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

then rebuild guru-cloud (cargo build --release) to serve the real UI here.
</body>
</html>
"#;
