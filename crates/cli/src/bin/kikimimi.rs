//! Primary binary name. Thin wrapper: all logic lives in the crate's `lib.rs`
//! (`kikimimi_cli::run`), shared with the `kkmm` / `k2m2` alias binaries.

fn main() {
    kikimimi_cli::run();
}
