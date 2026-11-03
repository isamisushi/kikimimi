fn main() {
    println!("cargo:rerun-if-env-changed=KIKIMIMI_UPDATER_PUBLIC_KEY");
    println!("cargo:rerun-if-env-changed=VITE_ENABLE_ROUTER");
    tauri_build::build();
}
