fn main() {
    // Declaring the app commands generates one `allow-<command>` permission per command, so the
    // capability file grants exactly the commands the frontend uses instead of all of them.
    let manifest =
        tauri_build::AppManifest::new().commands(&["app_info", "show_window", "hide_to_tray", "quit"]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("tauri-build failed");
}
