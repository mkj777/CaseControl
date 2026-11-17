//! System tray icon and its menu: Show, Apply profile (one entry per profile, the last applied one checked),
//! Release devices, Quit (ADR 0001 #14). The menu is rebuilt whenever the profiles change.

use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::profiles::{ProfileList, same_name};
use crate::state::AppState;

/// Id of the tray icon.
pub const TRAY_ID: &str = "main";

const SHOW_ID: &str = "tray-show";
const APPLY_MENU_ID: &str = "tray-apply-profile";
const NO_PROFILES_ID: &str = "tray-no-profiles";
const PROFILE_PREFIX: &str = "tray-profile:";
const RELEASE_ID: &str = "tray-release-devices";
const QUIT_ID: &str = "tray-quit";

/// The actions behind the tray menu entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    /// Show and focus the main window.
    Show,
    /// Apply the named profile.
    ApplyProfile(String),
    /// Release all devices to their hardware mode.
    ReleaseDevices,
    /// Shut down and exit.
    Quit,
}

impl TrayAction {
    /// Menu item id.
    pub fn id(&self) -> String {
        match self {
            Self::Show => SHOW_ID.to_owned(),
            Self::ApplyProfile(name) => format!("{PROFILE_PREFIX}{name}"),
            Self::ReleaseDevices => RELEASE_ID.to_owned(),
            Self::Quit => QUIT_ID.to_owned(),
        }
    }

    /// Maps a menu item id back to its action.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            SHOW_ID => Some(Self::Show),
            RELEASE_ID => Some(Self::ReleaseDevices),
            QUIT_ID => Some(Self::Quit),
            _ => id
                .strip_prefix(PROFILE_PREFIX)
                .map(|name| Self::ApplyProfile(name.to_owned())),
        }
    }
}

/// One entry of the "Apply profile" submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileItem {
    /// Menu item id.
    pub id: String,
    /// Label (the profile name).
    pub label: String,
    /// Whether this is the last applied profile.
    pub checked: bool,
}

/// The "Apply profile" submenu entries for `list`.
pub fn profile_items(list: &ProfileList) -> Vec<ProfileItem> {
    list.names
        .iter()
        .map(|name| ProfileItem {
            id: TrayAction::ApplyProfile(name.clone()).id(),
            // `&` marks a mnemonic in menu labels; double it to show it literally.
            label: name.replace('&', "&&"),
            checked: list.active.as_deref().is_some_and(|a| same_name(a, name)),
        })
        .collect()
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, list: &ProfileList) -> tauri::Result<Menu<R>> {
    let items = profile_items(list);
    let checks = items
        .iter()
        .map(|p| CheckMenuItem::with_id(app, &p.id, &p.label, true, p.checked, None::<&str>))
        .collect::<tauri::Result<Vec<_>>>()?;
    let empty = MenuItem::with_id(app, NO_PROFILES_ID, "No profiles", false, None::<&str>)?;
    let entries: Vec<&dyn IsMenuItem<R>> = if checks.is_empty() {
        vec![&empty]
    } else {
        checks.iter().map(|c| c as &dyn IsMenuItem<R>).collect()
    };
    let apply = Submenu::with_id_and_items(app, APPLY_MENU_ID, "Apply profile", true, &entries)?;
    Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, SHOW_ID, "Show", true, None::<&str>)?,
            &apply,
            &MenuItem::with_id(app, RELEASE_ID, "Release devices", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, QUIT_ID, "Quit", true, None::<&str>)?,
        ],
    )
}

/// Builds the tray icon with its menu and registers the event handlers.
pub fn create<R: Runtime>(app: &AppHandle<R>, list: &ProfileList) -> tauri::Result<TrayIcon<R>> {
    let menu = build_menu(app, list)?;
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

/// Rebuilds the tray menu for `list`.
pub fn refresh<R: Runtime>(app: &AppHandle<R>, list: &ProfileList) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let result = build_menu(app, list).and_then(|menu| tray.set_menu(Some(menu)));
    if let Err(err) = result {
        tracing::error!(%err, "cannot rebuild the tray menu");
    }
}

/// Runs `f` with the managed state off the event-loop thread (device calls can take seconds).
fn in_background<R: Runtime>(app: &AppHandle<R>, f: impl FnOnce(&AppHandle<R>, &AppState) + Send + 'static) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(state) = app.try_state::<AppState>() {
            f(&app, &state);
        }
    });
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
        TrayAction::ApplyProfile(name) => in_background(app, move |app, state| {
            if let Err(err) = crate::commands::apply_profile_now(app, state, &name) {
                tracing::error!(profile = %name, %err, "tray: cannot apply profile");
                // A check item toggles itself on click; put the marks back where they belong.
                crate::commands::profiles_changed(app, state);
            }
            crate::commands::devices_changed(app);
        }),
        TrayAction::ReleaseDevices => in_background(app, |app, state| {
            match crate::commands::release_all_now(state) {
                Ok(report) => {
                    for f in &report.failed {
                        tracing::warn!(id = %f.id, error = %f.error, "tray: release failed");
                    }
                }
                Err(err) => tracing::error!(%err, "tray: cannot release devices"),
            }
            crate::commands::devices_changed(app);
        }),
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
        for action in [
            TrayAction::Show,
            TrayAction::ApplyProfile("Evening: warm".into()),
            TrayAction::ReleaseDevices,
            TrayAction::Quit,
        ] {
            assert_eq!(TrayAction::from_id(&action.id()), Some(action));
        }
        assert_eq!(TrayAction::from_id("nope"), None);
        assert_eq!(
            TrayAction::from_id(APPLY_MENU_ID),
            None,
            "the submenu itself is not an action"
        );
    }

    #[test]
    fn profile_items_check_the_active_profile() {
        let list = ProfileList {
            names: vec!["Day".into(), "R&B".into()],
            active: Some("r&b".into()),
        };
        let items = profile_items(&list);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "Day");
        assert!(!items[0].checked);
        assert_eq!(items[1].label, "R&&B");
        assert!(items[1].checked);
        assert_eq!(
            TrayAction::from_id(&items[1].id),
            Some(TrayAction::ApplyProfile("R&B".into()))
        );
    }

    #[test]
    fn no_active_profile_checks_nothing() {
        let list = ProfileList {
            names: vec!["Day".into()],
            active: None,
        };
        assert!(profile_items(&list).iter().all(|p| !p.checked));
    }
}
