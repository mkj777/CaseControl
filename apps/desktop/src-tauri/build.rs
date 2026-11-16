/// Every app command. Declaring them generates one `allow-<command>` permission per command, so the capability
/// file grants exactly the commands the frontend uses instead of all of them.
const COMMANDS: &[&str] = &[
    "app_info",
    "show_window",
    "hide_to_tray",
    "quit",
    "list_devices",
    "set_direct",
    "set_zone",
    "set_mode",
    "resize_zone",
    "save_to_hardware",
    "release_device",
    "release_all",
    "rescan",
    "set_device_hidden",
    "set_zone_hidden",
    "list_profiles",
    "save_profile",
    "apply_profile",
    "rename_profile",
    "delete_profile",
    "scan_conflicts",
    "terminate_process",
    "stop_service",
    "get_settings",
    "set_settings",
];

fn main() {
    let manifest = tauri_build::AppManifest::new().commands(COMMANDS);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("tauri-build failed");
}
