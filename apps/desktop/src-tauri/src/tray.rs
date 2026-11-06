//! System tray icon and its menu: Show, Apply profile, Release devices, Quit (ADR 0001 #14).
//! "Apply profile" and "Release devices" are disabled placeholders until Phase 2/3 wire them up.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Runtime};

/// Id of the tray icon.
pub const TRAY_ID: &str = "main";

/// The actions behind the tray menu entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Show and focus the main window.
    Show,
    /// Apply a profile (placeholder, disabled).
    ApplyProfile,
    /// Release all devices to firmware control (placeholder, disabled).
    ReleaseDevices,
    /// Shut down and exit.
    Quit,
}

impl TrayAction {
    /// All actions in menu order.
    pub const ALL: [Self; 4] = [Self::Show, Self::ApplyProfile, Self::ReleaseDevices, Self::Quit];

    /// Menu item id.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Show => "tray-show",
            Self::ApplyProfile => "tray-apply-profile",
            Self::ReleaseDevices => "tray-release-devices",
            Self::Quit => "tray-quit",
        }
    }

    /// Menu item label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Show => "Show",
            Self::ApplyProfile => "Apply profile",
            Self::ReleaseDevices => "Release devices",
            Self::Quit => "Quit",
        }
    }

    /// Whether the menu item is clickable in this phase.
    pub const fn enabled(self) -> bool {
        matches!(self, Self::Show | Self::Quit)
    }

    /// Maps a menu item id back to its action.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.id() == id)
    }
}

/// Builds the tray icon with its menu and registers the event handlers.
pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<TrayIcon<R>> {
    let item = |action: TrayAction| {
        MenuItem::with_id(app, action.id(), action.label(), action.enabled(), None::<&str>)
    };
    let menu = Menu::with_items(
        app,
        &[
            &item(TrayAction::Show)?,
            &item(TrayAction::ApplyProfile)?,
            &item(TrayAction::ReleaseDevices)?,
            &PredefinedMenuItem::separator(app)?,
            &item(TrayAction::Quit)?,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("CaseControl")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(on_tray_icon_event);
    match app.default_window_icon() {
        Some(icon) => builder = builder.icon(icon.clone()),
        None => tracing::warn!("no default window icon; tray icon has no image"),
    }
    builder.build(app)
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let Some(action) = TrayAction::from_id(event.id().as_ref()) else {
        tracing::warn!(id = event.id().as_ref(), "unknown tray menu id");
        return;
    };
    tracing::info!(?action, "tray menu action");
    match action {
        TrayAction::Show => {
            if let Err(err) = crate::show_main_window(app) {
                tracing::error!(%err, "cannot show main window");
            }
        }
        // Disabled in Phase 1; the click cannot happen, but keep the match exhaustive.
        TrayAction::ApplyProfile | TrayAction::ReleaseDevices => {}
        TrayAction::Quit => crate::shutdown(app),
    }
}

fn on_tray_icon_event<R: Runtime>(tray: &TrayIcon<R>, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        tracing::info!("tray icon left click");
        if let Err(err) = crate::show_main_window(tray.app_handle()) {
            tracing::error!(%err, "cannot show main window");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for action in TrayAction::ALL {
            assert_eq!(TrayAction::from_id(action.id()), Some(action));
        }
        assert_eq!(TrayAction::from_id("nope"), None);
    }

    #[test]
    fn menu_labels_and_state_match_phase_one() {
        let labels: Vec<_> = TrayAction::ALL.iter().map(|a| a.label()).collect();
        assert_eq!(labels, ["Show", "Apply profile", "Release devices", "Quit"]);
        let enabled: Vec<_> = TrayAction::ALL.iter().map(|a| a.enabled()).collect();
        assert_eq!(enabled, [true, false, false, true]);
    }
}
