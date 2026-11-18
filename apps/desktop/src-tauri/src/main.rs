// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Elevated helper mode (`--elevated-actions <file>`): runs before Tauri, the single-instance guard or the engine.
    if let Some(code) = casecontrol_desktop_lib::elevated::helper_main(std::env::args_os()) {
        std::process::exit(code);
    }
    casecontrol_desktop_lib::run();
}
