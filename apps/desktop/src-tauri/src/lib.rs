//! CaseControl desktop shell (Tauri 2). Skeleton created by the orchestrator; the Phase 1 desktop
//! worker adds tray, hide-on-close, single instance, logging and the first IPC commands.

/// Builds and runs the Tauri application.
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running CaseControl");
}
