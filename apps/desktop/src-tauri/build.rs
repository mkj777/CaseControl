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
    "set_device_brightness",
    "set_master_brightness",
    "connect_all",
    "rescan",
    "set_device_hidden",
    "set_zone_hidden",
    "list_profiles",
    "save_profile",
    "apply_profile",
    "rename_profile",
    "delete_profile",
    "scan_conflicts",
    "startup_conflicts",
    "resolve_conflicts",
    "set_autostart",
    "set_dont_ask",
    "close_program",
    "get_settings",
    "set_settings",
    "set_default_color",
];

fn main() {
    let manifest = tauri_build::AppManifest::new().commands(COMMANDS);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("tauri-build failed");
}
