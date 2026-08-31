//! Primary binary name. Thin wrapper: all logic lives in the crate's `lib.rs`
//! (`kikimimi_cli::run`), shared with the `kkmm` alias binary.

fn main() {
    kikimimi_cli::run();
}
