// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Elevated helper mode (`--elevated-actions <file>`): runs before Tauri, the single-instance guard or the engine.
    if let Some(code) = casecontrol_desktop_lib::elevated::helper_main(std::env::args_os()) {
        std::process::exit(code);
    }
    // A command line that names a headless mode in another than its exact form starts nothing (fail closed).
    if let Some(code) = casecontrol_desktop_lib::boot::mode_guard_main(std::env::args_os()) {
        std::process::exit(code);
    }
    // The start-up task (`--apply-at-boot`, ADR 0001 #42): applies the colours headless and exits; no Tauri.
    if let Some(code) = casecontrol_desktop_lib::boot::boot_main(std::env::args_os()) {
        std::process::exit(code);
    }
    // The uninstaller's clean-up (`--uninstall-cleanup`): tasks and saved device state; no Tauri.
    if let Some(code) = casecontrol_desktop_lib::cleanup::cleanup_main(std::env::args_os()) {
        std::process::exit(code);
    }
    // The window never starts in session 0.
    if let Some(code) = casecontrol_desktop_lib::refuse_session_zero() {
        std::process::exit(code);
    }
    casecontrol_desktop_lib::run();
}
