//! IPC commands callable from the frontend via `invoke`. Errors are returned as strings, which is what the
//! frontend receives as the rejected promise value.
//!
//! Commands that touch devices, files or the system run on Tauri's blocking pool (`blocking`) so a slow device
//! acknowledgement (up to 3 s) never stalls the window's event loop. The logic they share with the tray lives in
//! the `*_now` functions.

use std::collections::{BTreeMap, BTreeSet};

use cc_config::Profile;
use cc_core::{DeviceId, ModeState, Rgb};
use cc_engine::{
    ApplyReport, DetectFailure, DeviceStatus, DeviceView, EngineHandle, LightingView, ReleaseReport,
    mask_hidden_zones,
};
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

use crate::conflict_actions::{
    ActionReport, check_catalogue_id, conflict_view, known_devices, run_and_report, run_scan,
    scan_and_publish, set_dont_ask_in,
};
use crate::conflicts::ConflictScan;
use crate::events::{self, CONFLICTS_UPDATED, DEVICES_CHANGED, PROFILES_CHANGED};
use crate::hidden::{
    HiddenEntry, hidden_device_error, hidden_devices, hidden_view, hidden_zone_map, hidden_zones,
    is_device_hidden, zone_write,
};
use crate::prefs::{SettingsPatch, SettingsView, apply_patch, record_zone_size};
use crate::profiles::{ProfileList, duplicate_name, last_after_rename, next_profile_name, plan_delete};
use crate::startup::{
    CloseTarget, ResolveRequest, StartupConflicts, plan_autostart, plan_close, plan_resolve,
};
use crate::state::AppState;
use crate::updater::{AppUpdater, UpdateView};

/// Which Rust profile this binary was compiled with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildProfile {
    /// `debug_assertions` on (`cargo tauri dev`, `cargo build`).
    Debug,
    /// `debug_assertions` off (`cargo tauri build`).
    Release,
}

impl BuildProfile {
    /// The profile of the running binary.
    pub const fn current() -> Self {
        if cfg!(debug_assertions) {
            Self::Debug
        } else {
            Self::Release
        }
    }
}

/// Basic facts about the running app, shown on the Settings page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppInfo {
    /// Product name from `tauri.conf.json`.
    pub name: String,
    /// App version from `tauri.conf.json`.
    pub version: String,
    /// Build profile of this binary.
    pub profile: BuildProfile,
}

/// What the Devices page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceList {
    /// Controlled devices, sorted by kind, then name.
    pub devices: Vec<DeviceView>,
    /// Devices that were found but could not be brought up.
    pub failures: Vec<DetectFailure>,
    /// Why the engine is not running, if it is not.
    pub engine_error: Option<String>,
    /// What the user hid, by device id (also for devices that are not connected now).
    pub hidden: BTreeMap<DeviceId, HiddenEntry>,
    /// Master brightness (from the settings while the engine is not running).
    pub lighting: LightingView,
}

/// Runs `f` with the managed state on the blocking pool.
async fn blocking<R, T, F>(app: AppHandle<R>, f: F) -> Result<T, String>
where
    R: Runtime,
    T: Send + 'static,
    F: FnOnce(&AppHandle<R>, &AppState) -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || {
        let state = app
            .try_state::<AppState>()
            .ok_or_else(|| "the app is still starting".to_owned())?;
        f(&app, &state)
    })
    .await
    .map_err(|err| err.to_string())?
}

fn err_string(err: impl std::fmt::Display) -> String {
    err.to_string()
}

/// The Devices page's data.
pub fn device_list_now(state: &AppState) -> DeviceList {
    let (hidden, saved) = {
        let settings = state.settings();
        (
            hidden_view(&settings),
            LightingView {
                master_brightness: settings.master_brightness,
            },
        )
    };
    match state.engine() {
        Ok(engine) => DeviceList {
            devices: engine.devices(),
            failures: engine.failures(),
            engine_error: None,
            hidden,
            lighting: engine.lighting(),
        },
        Err(_) => DeviceList {
            devices: Vec::new(),
            failures: Vec::new(),
            engine_error: state.engine_error(),
            hidden,
            lighting: saved,
        },
    }
}

/// Fails if device `id` is hidden; otherwise returns the names of its hidden zones.
fn visible_device(state: &AppState, id: &DeviceId) -> Result<BTreeSet<String>, String> {
    let settings = state.settings();
    if is_device_hidden(&settings, id) {
        return Err(hidden_device_error(id));
    }
    Ok(hidden_zones(&settings, id))
}

/// The engine's view of device `id`, or an error for an unknown device.
fn describe(engine: &EngineHandle, id: &DeviceId) -> Result<DeviceView, String> {
    engine.device(id).ok_or_else(|| format!("unknown device {id}"))
}

/// Tells the frontend and the tray that the profile list or the active profile changed.
pub fn profiles_changed<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    match state.profile_list() {
        Ok(list) => {
            crate::tray::refresh(app, &list);
            events::emit(app, PROFILES_CHANGED, list);
        }
        Err(err) => tracing::warn!(%err, "cannot list profiles"),
    }
}

/// Switches to profile `name` (ADR 0001 #40): saves a pending change of the active profile, applies `name` with its
/// device brightness (connected devices it has nothing for fall back to their own mode, else the fallback colour) and
/// makes it active.
pub fn apply_profile_now<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    name: &str,
) -> Result<ApplyReport, String> {
    let lock = state.profile_lock();
    state.flush_profile_locked(&lock);
    let result = state.switch_profile_locked(&lock, name);
    drop(lock);
    profiles_changed(app, state);
    result
}

/// Releases every device that is not faulted; the report counts the devices that were driven and are released now.
pub fn release_all_now(state: &AppState) -> Result<ReleaseReport, String> {
    let report = state.engine()?.release_all();
    tracing::info!(
        released = report.released,
        failed = report.failed.len(),
        "release all"
    );
    Ok(report)
}

/// Returns name, version and build profile of the running app.
#[tauri::command]
pub fn app_info<R: Runtime>(app: AppHandle<R>) -> AppInfo {
    let package = app.package_info();
    AppInfo {
        name: package.name.clone(),
        version: package.version.to_string(),
        profile: BuildProfile::current(),
    }
}

/// Shows, unminimizes and focuses the main window.
#[tauri::command]
pub fn show_window<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    crate::show_main_window(&app).map_err(err_string)
}

/// Hides the main window; the app keeps running in the tray.
#[tauri::command]
pub fn hide_to_tray<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    crate::hide_main_window(&app).map_err(err_string)
}

/// Shuts the app down (same path as Quit in the tray menu).
#[tauri::command]
pub fn quit<R: Runtime>(app: AppHandle<R>) {
    crate::shutdown(&app);
}

/// Every controlled device, detection failures and the engine start error.
#[tauri::command]
pub async fn list_devices<R: Runtime>(app: AppHandle<R>) -> Result<DeviceList, String> {
    blocking(app, |_, state| Ok(device_list_now(state))).await
}

/// Shows one colour per LED in direct mode (coalesced to the frame rate by the engine). Refused for a hidden
/// device; hidden zones are sent black.
#[tauri::command]
pub async fn set_direct<R: Runtime>(
    app: AppHandle<R>,
    id: DeviceId,
    mut colors: Vec<Rgb>,
) -> Result<(), String> {
    blocking(app, move |_, state| {
        let engine = state.engine()?;
        let hidden = visible_device(state, &id)?;
        if !hidden.is_empty() {
            mask_hidden_zones(&describe(engine, &id)?.description, &hidden, &mut colors);
        }
        engine.set_direct(&id, colors).map_err(err_string)?;
        state.profile_changed();
        Ok(())
    })
    .await
}

/// Sets the colours of one zone in direct mode. Refused for a hidden device or a hidden zone; the other hidden
/// zones are sent black.
#[tauri::command]
pub async fn set_zone<R: Runtime>(
    app: AppHandle<R>,
    id: DeviceId,
    zone: usize,
    colors: Vec<Rgb>,
) -> Result<(), String> {
    blocking(app, move |_, state| {
        let engine = state.engine()?;
        let hidden = visible_device(state, &id)?;
        if hidden.is_empty() {
            engine.set_zone(&id, zone, colors).map_err(err_string)?;
        } else {
            // The engine's buffer may still hold colours of a hidden zone (e.g. from before it was hidden), so send
            // the whole masked buffer instead of patching one zone into it.
            let view = describe(engine, &id)?;
            let buffer = zone_write(&view.description, &hidden, &view.colors, zone, &colors)?;
            engine.set_direct(&id, buffer).map_err(err_string)?;
        }
        state.profile_changed();
        Ok(())
    })
    .await
}

/// Switches a device to a hardware mode. Refused for a hidden device.
#[tauri::command]
pub async fn set_mode<R: Runtime>(app: AppHandle<R>, id: DeviceId, state: ModeState) -> Result<(), String> {
    blocking(app, move |_, st| {
        let engine = st.engine()?;
        visible_device(st, &id)?;
        engine.set_mode(&id, state).map_err(err_string)?;
        st.profile_changed();
        Ok(())
    })
    .await
}

/// Changes a resizable zone's LED count and remembers it by zone name in the settings. Returns the device with
/// its new layout.
#[tauri::command]
pub async fn resize_zone<R: Runtime>(
    app: AppHandle<R>,
    id: DeviceId,
    zone: usize,
    len: usize,
) -> Result<DeviceView, String> {
    blocking(app, move |_, state| {
        let engine = state.engine()?;
        visible_device(state, &id)?;
        let zone_name = engine
            .device(&id)
            .ok_or_else(|| format!("unknown device {id}"))?
            .description
            .zones
            .get(zone)
            .map(|z| z.name.clone())
            .ok_or_else(|| format!("no zone {zone}"))?;
        engine.resize_zone(&id, zone, len).map_err(err_string)?;
        state
            .update_settings(|s| {
                record_zone_size(s, &id, &zone_name, len);
                Ok(())
            })
            .map_err(|err| format!("The zone was resized, but the size could not be saved: {err}"))?;
        engine
            .device(&id)
            .ok_or_else(|| format!("device {id} disappeared"))
    })
    .await
}

/// Writes the device's current mode to its flash. Only on explicit user request.
#[tauri::command]
pub async fn save_to_hardware<R: Runtime>(app: AppHandle<R>, id: DeviceId) -> Result<(), String> {
    blocking(app, move |_, state| {
        let engine = state.engine()?;
        visible_device(state, &id)?;
        tracing::info!(%id, "saving to device flash on user request");
        engine.save_to_hardware(&id).map_err(err_string)
    })
    .await
}

/// Hands one device back to its hardware mode.
#[tauri::command]
pub async fn release_device<R: Runtime>(app: AppHandle<R>, id: DeviceId) -> Result<(), String> {
    blocking(app, move |_, state| {
        state.engine()?.release(&id).map_err(err_string)
    })
    .await
}

/// Hands every device back to its hardware mode. Returns how many devices were driven and are released now (for the
/// "N devices released" toast) and the releases that failed.
#[tauri::command]
pub async fn release_all<R: Runtime>(app: AppHandle<R>) -> Result<ReleaseReport, String> {
    blocking(app, |_, state| release_all_now(state)).await
}

/// Sets one device's brightness in percent (ADR 0001 #33). Refused for a hidden device. The value is saved to the
/// active profile (coalesced while a slider is dragged; ADR 0001 #40).
#[tauri::command]
pub async fn set_device_brightness<R: Runtime>(
    app: AppHandle<R>,
    id: DeviceId,
    percent: u8,
) -> Result<(), String> {
    blocking(app, move |_, state| {
        let engine = state.engine()?;
        visible_device(state, &id)?;
        engine.set_device_brightness(&id, percent).map_err(err_string)?;
        state.profile_changed();
        Ok(())
    })
    .await
}

/// Sets the master brightness in percent, multiplied into every device (ADR 0001 #33). While a slider is dragged the
/// page sends `persist = false`; the final value comes with `persist = true` and is saved in the settings (master
/// brightness is not part of a profile). Devices it takes over are saved to the active profile with what they show
/// (ADR 0001 #40). Returns the master brightness.
#[tauri::command]
pub async fn set_master_brightness<R: Runtime>(
    app: AppHandle<R>,
    percent: u8,
    persist: bool,
) -> Result<LightingView, String> {
    blocking(app, move |_, state| {
        let engine = state.engine()?;
        let percent = percent.min(cc_config::FULL_BRIGHTNESS);
        engine.set_master_brightness(percent);
        state.profile_changed();
        if persist {
            state
                .update_settings(|s| {
                    s.master_brightness = percent;
                    Ok(())
                })
                .map_err(|err| format!("The brightness was changed, but it could not be saved: {err}"))?;
        }
        Ok(engine.lighting())
    })
    .await
}

/// Connect all, the counterpart of Release all: takes control of every released device that is neither hidden nor
/// faulted (its last setting, else the mode it shows on its own, else the default colour from the settings; ADR 0001
/// #36). What the devices show then is saved to the active profile (ADR 0001 #40). Returns the master brightness.
#[tauri::command]
pub async fn connect_all<R: Runtime>(app: AppHandle<R>) -> Result<LightingView, String> {
    blocking(app, |_, state| {
        let engine = state.engine()?;
        engine.connect_all();
        state.profile_changed();
        tracing::info!("connect all");
        Ok(engine.lighting())
    })
    .await
}

/// Sets the colour a takeover shows on a device with neither a setting nor a readback (ADR 0001 #36). While the
/// picker is dragged the page sends `persist = false`: the engine uses the colour, nothing is saved. The final value
/// comes with `persist = true` and is saved; if that fails the engine goes back to the saved colour. Devices already
/// driven keep what they show. Returns the colour now in effect.
#[tauri::command]
pub async fn set_default_color<R: Runtime>(
    app: AppHandle<R>,
    color: Rgb,
    persist: bool,
) -> Result<Rgb, String> {
    blocking(app, move |_, state| {
        if persist
            && let Err(err) = state.update_settings(|s| {
                s.default_color = color;
                Ok(())
            })
        {
            if let Ok(engine) = state.engine() {
                engine.set_default_color(state.settings().default_color);
            }
            return Err(err);
        }
        if let Ok(engine) = state.engine() {
            engine.set_default_color(color);
        }
        Ok(color)
    })
    .await
}

/// Hides or unhides a device (ADR 0001 #26) and saves that. Hiding releases the device to its own hardware mode;
/// unhiding changes nothing on the hardware. Either way the engine learns which devices brightness changes must
/// not take over. Returns the new device list.
#[tauri::command]
pub async fn set_device_hidden<R: Runtime>(
    app: AppHandle<R>,
    id: DeviceId,
    hidden: bool,
) -> Result<DeviceList, String> {
    blocking(app, move |_, state| {
        state.update_settings(|s| {
            crate::hidden::set_device_hidden(s, &id, hidden);
            Ok(())
        })?;
        tracing::info!(%id, hidden, "device visibility changed");
        if let Ok(engine) = state.engine() {
            engine.set_excluded(hidden_devices(&state.settings()));
            if hidden
                && engine
                    .device(&id)
                    .is_some_and(|v| v.status == DeviceStatus::Active)
            {
                engine
                    .release(&id)
                    .map_err(|err| format!("The device is hidden, but it could not be released: {err}"))?;
            }
        }
        Ok(device_list_now(state))
    })
    .await
}

/// Hides or unhides zone `zone_name` of a device (ADR 0001 #26) and saves that. The engine learns the hidden zones
/// first (as its dark zones) and keeps them black in every direct output from then on: if the device shows direct
/// colours, hiding a zone switches it off right away; unhiding leaves it off until the next colour change. Returns the
/// new device list.
#[tauri::command]
pub async fn set_zone_hidden<R: Runtime>(
    app: AppHandle<R>,
    id: DeviceId,
    zone_name: String,
    hidden: bool,
) -> Result<DeviceList, String> {
    blocking(app, move |_, state| {
        let engine = state.engine()?;
        let view = describe(engine, &id)?;
        if !view.description.zones.iter().any(|z| z.name == zone_name) {
            return Err(format!("{} has no zone {zone_name}", view.description.info.name));
        }
        // The engine gets the new zones under the settings lock, before the save, so a concurrent change of another
        // zone cannot hand it an older set; it re-masks a device it drives with direct colours itself.
        let saved = state.update_settings(|s| {
            crate::hidden::set_zone_hidden(s, &id, &zone_name, hidden);
            engine.set_dark_zones(hidden_zone_map(s));
            Ok(())
        });
        if let Err(err) = saved {
            // Not saved: the engine goes back to the zones that are still in the settings.
            engine.set_dark_zones(hidden_zone_map(&state.settings()));
            return Err(err);
        }
        tracing::info!(%id, zone = %zone_name, hidden, "zone visibility changed");
        Ok(device_list_now(state))
    })
    .await
}

/// Runs detection again; devices already under control are left alone.
#[tauri::command]
pub async fn rescan<R: Runtime>(app: AppHandle<R>) -> Result<DeviceList, String> {
    blocking(app, |_, state| {
        if let Ok(engine) = state.engine() {
            engine.rescan();
        }
        Ok(device_list_now(state))
    })
    .await
}

/// Profile names and the active one.
#[tauri::command]
pub async fn list_profiles<R: Runtime>(app: AppHandle<R>) -> Result<ProfileList, String> {
    blocking(app, |_, state| state.profile_list()).await
}

/// Switches to profile `name` (see [`apply_profile_now`]).
#[tauri::command]
pub async fn apply_profile<R: Runtime>(app: AppHandle<R>, name: String) -> Result<ApplyReport, String> {
    blocking(app, move |app, state| apply_profile_now(app, state, &name)).await
}

/// Saves `profile` as a new profile and switches to it, under the profile lock.
fn create_and_switch<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    make: impl FnOnce(&[String]) -> Result<Profile, String>,
) -> Result<ProfileList, String> {
    let lock = state.profile_lock();
    state.flush_profile_locked(&lock);
    let storage = state.storage()?;
    let names = storage.profiles.list().map_err(err_string)?;
    let profile = make(&names)?;
    storage.profiles.save(&profile).map_err(err_string)?;
    tracing::info!(profile = %profile.name, devices = profile.devices.len(), "profile created");
    let switched = state.switch_profile_locked(&lock, &profile.name);
    drop(lock);
    profiles_changed(app, state);
    switched?;
    state.profile_list()
}

/// Creates an empty profile "Profile N" (lowest free N) and switches to it: connected devices fall back to their own
/// mode, else the fallback colour, at full device brightness (ADR 0001 #40).
#[tauri::command]
pub async fn create_profile<R: Runtime>(app: AppHandle<R>) -> Result<ProfileList, String> {
    blocking(app, |app, state| {
        create_and_switch(app, state, |names| {
            Ok(Profile {
                name: next_profile_name(names),
                devices: BTreeMap::new(),
                brightness: BTreeMap::new(),
            })
        })
    })
    .await
}

/// Copies profile `name` as `"<name> copy"` (then `"<name> copy 2"`, ...) and switches to the copy (ADR 0001 #40).
#[tauri::command]
pub async fn duplicate_profile<R: Runtime>(app: AppHandle<R>, name: String) -> Result<ProfileList, String> {
    blocking(app, move |app, state| {
        create_and_switch(app, state, |names| {
            let source = state.storage()?.profiles.load(&name).map_err(err_string)?;
            Ok(Profile {
                name: duplicate_name(&source.name, names),
                ..source
            })
        })
    })
    .await
}

/// Renames profile `from` to `to`; the active profile follows.
#[tauri::command]
pub async fn rename_profile<R: Runtime>(
    app: AppHandle<R>,
    from: String,
    to: String,
) -> Result<ProfileList, String> {
    blocking(app, move |app, state| {
        let lock = state.profile_lock();
        state.flush_profile_locked(&lock);
        state.storage()?.profiles.rename(&from, &to).map_err(err_string)?;
        let next = last_after_rename(state.settings().last_profile.as_deref(), &from, &to);
        let saved = match next {
            Some(next) => state.set_active_profile(&next),
            None => Ok(()),
        };
        drop(lock);
        profiles_changed(app, state);
        saved?;
        state.profile_list()
    })
    .await
}

/// Deletes profile `name`. Refused for the last profile (there is always one, ADR 0001 #40). Deleting the active
/// profile switches to the first remaining one.
#[tauri::command]
pub async fn delete_profile<R: Runtime>(app: AppHandle<R>, name: String) -> Result<ProfileList, String> {
    blocking(app, move |app, state| {
        let lock = state.profile_lock();
        state.flush_profile_locked(&lock);
        let storage = state.storage()?;
        let names = storage.profiles.list().map_err(err_string)?;
        let active = state.settings().last_profile.clone();
        let next = plan_delete(&names, active.as_deref(), &name).map_err(|refused| refused.to_string())?;
        storage.profiles.delete(&name).map_err(err_string)?;
        tracing::info!(profile = %name.trim(), "profile deleted");
        let switched = match next {
            Some(next) => state.switch_profile_locked(&lock, &next).map(|_| ()),
            None => Ok(()),
        };
        drop(lock);
        profiles_changed(app, state);
        switched?;
        state.profile_list()
    })
    .await
}

/// Scans for conflicting software and emits `conflicts-updated`.
#[tauri::command]
pub async fn scan_conflicts<R: Runtime>(app: AppHandle<R>) -> Result<ConflictScan, String> {
    blocking(app, |app, state| scan_and_publish(app, state)).await
}

/// The start dialog model from a fresh scan (also emits `conflicts-updated`). Empty when there is nothing to ask.
#[tauri::command]
pub async fn startup_conflicts<R: Runtime>(app: AppHandle<R>) -> Result<StartupConflicts, String> {
    blocking(app, |app, state| Ok(scan_and_publish(app, state)?.startup)).await
}

/// The start dialog's Proceed: per program close it and/or turn its autostart off. Everything that needs
/// administrator rights runs behind one UAC prompt. Returns per-action results and the scan afterwards (also emitted
/// as `conflicts-updated`).
#[tauri::command]
pub async fn resolve_conflicts<R: Runtime>(
    app: AppHandle<R>,
    request: ResolveRequest,
) -> Result<ActionReport, String> {
    blocking(app, move |app, state| {
        let report = run_scan(state)?;
        let (_, visible) = known_devices(state);
        let plan = plan_resolve(&request, &report, &visible, cc_conflicts::current_session_id());
        tracing::info!(
            programs = request.items.len(),
            actions = plan.actions.len(),
            refused = plan.refused.len(),
            "resolving start-up conflicts on user request"
        );
        run_and_report(app, state, &report, &plan.actions, plan.refused)
    })
    .await
}

/// The Conflicts page's Turn off (`enabled = false`) / Re-enable (`true`) of a finding's autostart. Works for every
/// finding, vendor tools included.
#[tauri::command]
pub async fn set_autostart<R: Runtime>(
    app: AppHandle<R>,
    entry: String,
    enabled: bool,
) -> Result<ActionReport, String> {
    blocking(app, move |app, state| {
        let report = run_scan(state)?;
        let remembered = state.settings().conflicts.previous_start_types.clone();
        let actions = plan_autostart(&entry, enabled, &report, &remembered)?;
        tracing::info!(entry = %entry, enabled, actions = actions.len(), "changing autostart on user request");
        run_and_report(app, state, &report, &actions, Vec::new())
    })
    .await
}

/// Sets or clears "Don't ask again" for catalogue entry `entry`. Returns the view of the last scan with the new
/// setting (also emitted as `conflicts-updated`).
#[tauri::command]
pub async fn set_dont_ask<R: Runtime>(
    app: AppHandle<R>,
    entry: String,
    dont_ask: bool,
) -> Result<ConflictScan, String> {
    blocking(app, move |app, state| {
        check_catalogue_id(&entry)?;
        let changed = state.settings().conflicts.dont_ask.contains(&entry) != dont_ask;
        if changed {
            state.update_settings(|s| {
                set_dont_ask_in(&mut s.conflicts.dont_ask, &entry, dont_ask);
                Ok(())
            })?;
        }
        let view = match state.with_last_scan(|r| Ok(r.clone())) {
            Ok(report) => conflict_view(state, &report),
            Err(_) => return scan_and_publish(app, state),
        };
        events::emit(app, CONFLICTS_UPDATED, view.clone());
        Ok(view)
    })
    .await
}

/// The Conflicts page's End process / Stop service: closes `targets` of finding `entry` (any class). The targets
/// are re-checked against a fresh scan; a service stop (and a process this user may not end) runs in the elevated
/// helper behind one UAC prompt. Returns per-action results and the scan afterwards.
#[tauri::command]
pub async fn close_program<R: Runtime>(
    app: AppHandle<R>,
    entry: String,
    targets: Vec<CloseTarget>,
) -> Result<ActionReport, String> {
    blocking(app, move |app, state| {
        let report = run_scan(state)?;
        let actions = plan_close(&entry, &targets, &report, cc_conflicts::current_session_id())?;
        tracing::info!(entry = %entry, actions = actions.len(), "closing a program on user request");
        run_and_report(app, state, &report, &actions, Vec::new())
    })
    .await
}

/// The Settings page's view of the current settings, with the line under "Save colours to the devices".
pub fn settings_view(state: &AppState) -> SettingsView {
    SettingsView {
        save_to_devices_note: state.save_note(),
        ..SettingsView::of(&state.settings(), state.settings_error())
    }
}

/// The Settings page's data.
#[tauri::command]
pub async fn get_settings<R: Runtime>(app: AppHandle<R>) -> Result<SettingsView, String> {
    blocking(app, |_, state| Ok(settings_view(state))).await
}

/// Applies a partial settings change, saves it, and carries it over to the running app: frame rate, detector switches
/// and the takeover default colour go to the engine (the colour is used from the next takeover on; devices already
/// driven keep what they show), autostart to the logon task (release builds only, see [`crate::autostart`]). "Keep
/// colours when signing out" is read at exit. "Apply colours before sign-in" and "Save colours to the devices" have
/// their own commands ([`set_apply_before_sign_in`], [`set_save_to_devices`]), because they ask for a UAC prompt or
/// write to devices and must not hold the settings lock meanwhile.
#[tauri::command]
pub async fn set_settings<R: Runtime>(
    app: AppHandle<R>,
    patch: SettingsPatch,
) -> Result<SettingsView, String> {
    blocking(app, move |_, state| {
        // The start-up autostart check must not act on a setting this patch changes meanwhile.
        let _autostart = patch.autostart.is_some().then(|| state.autostart_lock());
        let effects = state.update_settings(|s| {
            let effects = apply_patch(s, &patch)?;
            if let Some(on) = effects.autostart {
                crate::autostart::apply(on)?;
            }
            Ok(effects)
        })?;
        if let Ok(engine) = state.engine() {
            if let Some(fps) = effects.frame_rate {
                engine.set_frame_rate(fps);
            }
            if effects.detectors_changed {
                engine.set_disabled_families(state.settings().disabled_families());
            }
            if effects.family_enabled {
                engine.rescan();
            }
            if let Some(color) = effects.default_color {
                engine.set_default_color(color);
            }
        }
        Ok(settings_view(state))
    })
    .await
}

/// Shown when "Apply colours before sign-in" is switched in a debug build.
pub const BOOT_TASK_DEBUG_REFUSAL: &str =
    "Apply colours before sign-in is only available in installed builds.";

/// "Apply colours before sign-in" (ADR 0001 #42): creates (`on`) or removes the start-up task through the elevated
/// helper (one UAC prompt, `RegisterBootTask` / `RemoveBootTask`). The setting changes only when the helper reports
/// success; a declined prompt or a failure leaves it as it was and rejects with the reason ([`boot_task_error_text`]:
/// the helper's complete sentences are shown as they are). When the outcome is unknown (the helper still ran at the
/// timeout) or the setting could not be saved, the task is read again and the switch follows what is there
/// ([`align_apply_before_sign_in`]). Refused in debug builds (no task for a debug executable) and while settings cannot
/// be saved. Runs outside the settings lock, so lighting keeps working while the prompt is open; holds the start-up
/// task lock, so the start-up check does not run meanwhile.
#[tauri::command]
pub async fn set_apply_before_sign_in<R: Runtime>(
    app: AppHandle<R>,
    on: bool,
) -> Result<SettingsView, String> {
    blocking(app, move |app, state| {
        if cfg!(debug_assertions) {
            tracing::info!(on, "debug build: start-up task not changed");
            return Err(BOOT_TASK_DEBUG_REFUSAL.to_owned());
        }
        if let Some(err) = state.settings_error() {
            return Err(err);
        }
        let _serial = state.boot_task_lock();
        let action = if on {
            cc_conflicts::Action::RegisterBootTask {}
        } else {
            cc_conflicts::Action::RemoveBootTask {}
        };
        tracing::info!(on, "apply colours before sign-in: asking the elevated helper");
        let planned = cc_conflicts::PlannedAction {
            entry_id: cc_conflicts::BOOT_TASK_ENTRY.to_owned(),
            action,
        };
        let outcomes = crate::elevated::run_admin_batch(
            &[planned],
            cc_conflicts::current_session_id(),
            crate::conflict_actions::owner_window(app),
        );
        match outcomes.into_iter().next() {
            Some(outcome) if outcome.ok => {
                tracing::info!(on, "start-up task changed");
                if let Err(err) = state.update_settings(|s| {
                    s.apply_before_sign_in = on;
                    Ok(())
                }) {
                    tracing::warn!(%err, "the start-up task changed, but the setting could not be saved");
                    align_apply_before_sign_in(state, read_boot_task_state());
                    return Err(format!(
                        "The start-up task was {}, but this could not be saved: {err}",
                        if on { "created" } else { "removed" }
                    ));
                }
                Ok(settings_view(state))
            }
            Some(outcome) => {
                let reason = outcome
                    .error
                    .unwrap_or_else(|| "the elevated helper reported no reason".to_owned());
                tracing::warn!(on, %reason, unknown = outcome.unknown, "start-up task not changed");
                if outcome.unknown {
                    align_apply_before_sign_in(state, read_boot_task_state());
                }
                Err(boot_task_error_text(on, &reason))
            }
            None => Err("The elevated helper returned no result.".to_owned()),
        }
    })
    .await
}

/// The text the Settings page shows when the start-up task was not changed: the elevated helper's refusals are
/// complete sentences meant to be shown as they are (a prompt approved with another account, a request owned by
/// Administrators, a task under our name that is not ours) and name no direction, so they fit switching on and off
/// alike; a short reason gets a lead that says what failed ("not created" / "not removed").
pub fn boot_task_error_text(on: bool, reason: &str) -> String {
    let reason = reason.trim();
    let sentence = reason.starts_with(|c: char| c.is_uppercase()) && reason.ends_with('.');
    if sentence {
        reason.to_owned()
    } else {
        format!(
            "The start-up task was not {}: {reason}",
            if on { "created" } else { "removed" }
        )
    }
}

/// Whether "Apply colours before sign-in" should be on, from a read of the start-up task: off when there is none or
/// it is not ours, on when it is ours (also when it differs or is disabled; turning the switch off removes it).
/// `None` when the task could not be read (it may need administrator rights); the setting stays then.
pub fn boot_flag_from(read: &Result<cc_system::TaskState, String>) -> Option<bool> {
    use cc_system::TaskState;
    match read {
        Ok(TaskState::Missing | TaskState::Foreign(_)) => Some(false),
        Ok(TaskState::Matches | TaskState::Differs(_) | TaskState::Disabled) => Some(true),
        Err(_) => None,
    }
}

/// Reads the start-up task of the current user for this executable.
pub fn read_boot_task_state() -> Result<cc_system::TaskState, String> {
    #[cfg(windows)]
    {
        let sid = cc_system::current_user_sid().map_err(|e| e.to_string())?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let spec = cc_system::TaskSpec::boot(&sid, &exe).map_err(|e| e.to_string())?;
        cc_system::task_state(&cc_system::WindowsTaskStore, &spec).map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        Err("not supported on this platform".to_owned())
    }
}

/// Sets "Apply colours before sign-in" from `read` (see [`boot_flag_from`]) when it differs, logged. The caller holds
/// the start-up task lock. When the setting cannot be saved it still changes for this session, so Settings shows
/// what is there.
pub fn align_apply_before_sign_in(state: &AppState, read: Result<cc_system::TaskState, String>) {
    let Some(exists) = boot_flag_from(&read) else {
        if let Err(err) = read {
            tracing::info!(%err, "cannot read the start-up task; apply colours before sign-in left as it is");
        }
        return;
    };
    if state.settings().apply_before_sign_in == exists {
        return;
    }
    tracing::info!(task = ?read, setting = exists, "apply colours before sign-in follows the start-up task");
    let saved = state.update_settings(|s| {
        s.apply_before_sign_in = exists;
        Ok(())
    });
    if let Err(err) = saved {
        tracing::warn!(%err, "cannot save apply colours before sign-in; changed for this session only");
        state.settings().apply_before_sign_in = exists;
    }
}

/// "Save colours to the devices" (ADR 0001 #42): saves the switch, then saves the colours to every capable device
/// (on) or writes their original saved state back (off). Resolves with the settings view, whose
/// `save_to_devices_note` names the devices that were not saved or restored. Never called on live edits.
#[tauri::command]
pub async fn set_save_to_devices<R: Runtime>(app: AppHandle<R>, on: bool) -> Result<SettingsView, String> {
    blocking(app, move |_, state| {
        state.set_save_to_devices(on)?;
        Ok(settings_view(state))
    })
    .await
}

/// The managed updater, or an error while the app is still starting.
fn app_updater<R: Runtime>(app: &AppHandle<R>) -> Result<AppUpdater<R>, String> {
    app.try_state::<AppUpdater<R>>()
        .map(|u| u.inner().clone())
        .ok_or_else(|| "the app is still starting".to_owned())
}

/// The updater's status for the Settings page (ADR 0001 #41); changes arrive as `update-status`.
#[tauri::command]
pub fn update_status<R: Runtime>(app: AppHandle<R>) -> Result<UpdateView, String> {
    Ok(app_updater(&app)?.view())
}

/// "Check for updates": checks now and downloads a newer signed release like the background check (same code path,
/// same pending store). Resolves when the check has finished, with the status then; progress arrives as
/// `update-status`. A check that is already running is not started again; this then resolves at once with the
/// current status. Refused in builds without updates.
#[tauri::command]
pub async fn check_for_updates<R: Runtime>(app: AppHandle<R>) -> Result<UpdateView, String> {
    let updater = app_updater(&app)?;
    if !updater.enabled() {
        return Err(crate::UPDATES_DISABLED.to_owned());
    }
    tracing::info!("check for updates requested");
    tauri::async_runtime::spawn_blocking(move || {
        updater.check_once();
        updater.view()
    })
    .await
    .map_err(err_string)
}

/// "Update now": releases the devices, installs the pending update and restarts the app (see
/// [`crate::install_update_now`]). It runs on the main thread like tray Quit and the `quit` command; this command
/// only waits for it. Resolves only when nothing was installed, with the reason as the error.
#[tauri::command]
pub async fn install_update<R: Runtime>(app: AppHandle<R>) -> Result<UpdateView, String> {
    let updater = app_updater(&app)?;
    let (tx, rx) = std::sync::mpsc::channel();
    let main_app = app.clone();
    app.run_on_main_thread(move || {
        // Nobody may be waiting any more (the page went away); the result is logged by `install_update_now`.
        let _ = tx.send(crate::install_update_now(&main_app, &updater));
    })
    .map_err(err_string)?;
    tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(err_string)?
        .map_err(|_| "Update now ended without a result.".to_owned())?
}

/// Emits `devices-changed` (used after tray actions the frontend did not start).
pub fn devices_changed<R: Runtime>(app: &AppHandle<R>) {
    events::emit(app, DEVICES_CHANGED, ());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_matches_compilation() {
        let expected = if cfg!(debug_assertions) {
            BuildProfile::Debug
        } else {
            BuildProfile::Release
        };
        assert_eq!(BuildProfile::current(), expected);
    }

    #[test]
    fn app_info_serializes_to_frontend_shape() {
        let info = AppInfo {
            name: "CaseControl".into(),
            version: "0.1.0".into(),
            profile: BuildProfile::Release,
        };
        let json = serde_json::to_value(&info).expect("AppInfo serializes");
        assert_eq!(
            json,
            serde_json::json!({ "name": "CaseControl", "version": "0.1.0", "profile": "release" })
        );
        let debug = serde_json::to_value(BuildProfile::Debug).expect("BuildProfile serializes");
        assert_eq!(debug, serde_json::json!("debug"));
    }

    /// The JSON the frontend's `ipc.ts` types describe. If this changes, update `src/ipc.ts`.
    #[test]
    fn device_list_json_shape() {
        use cc_core::{
            Bus, ColorMode, DeviceDescription, DeviceError, DeviceInfo, DeviceKind, Led, Mode, ModeCaps,
            Zone, ZoneKind,
        };
        use cc_engine::{DeviceSetting, Takeover};
        let red = Rgb::new(0xFF, 0, 0);
        let description = DeviceDescription {
            info: DeviceInfo {
                id: DeviceId::hid(0x1462, 0x7D75, None),
                name: "Board".into(),
                vendor: "MSI".into(),
                kind: DeviceKind::Motherboard,
                bus: Bus::Hid,
                location: "path".into(),
                serial: None,
                firmware: Some("1.0".into()),
            },
            zones: vec![
                Zone {
                    name: "Grid".into(),
                    kind: ZoneKind::Matrix {
                        width: 2,
                        height: 1,
                        map: vec![Some(0), None],
                    },
                    start: 0,
                    len: 1,
                    resizable: None,
                    brightness_only: false,
                },
                Zone {
                    name: "JRAINBOW1".into(),
                    kind: ZoneKind::Linear,
                    start: 1,
                    len: 1,
                    resizable: Some(0..=120),
                    brightness_only: false,
                },
            ],
            leds: vec![Led { name: "a".into() }, Led { name: "b".into() }],
            modes: vec![Mode {
                id: 1,
                name: "Breathing".into(),
                caps: ModeCaps::SPEED | ModeCaps::MODE_SPECIFIC_COLOR,
                color_mode: ColorMode::ModeSpecific,
                speed: Some(0..=2),
                brightness: None,
                colors: Some(1..=1),
            }],
            direct_mode: None,
            keepalive: None,
        };
        let list = DeviceList {
            devices: vec![DeviceView {
                description,
                status: DeviceStatus::Faulted { error: "x".into() },
                setting: Some(DeviceSetting::Direct {
                    colors: vec![red, red],
                }),
                colors: vec![red, Rgb::BLACK],
                hardware: Some(ModeState {
                    mode: 0,
                    speed: Some(1),
                    brightness: None,
                    direction: None,
                    colors: vec![],
                }),
                family: "msi_mystic_light".into(),
                brightness: 55,
                takeover: Some(Takeover { mode_id: Some(1) }),
            }],
            failures: vec![DetectFailure {
                family: "wooting".into(),
                location: "p".into(),
                error: DeviceError::Transport("busy".into()),
            }],
            engine_error: None,
            hidden: BTreeMap::from([(
                DeviceId::hid(0x1462, 0x7D75, None),
                HiddenEntry {
                    device: false,
                    zones: vec!["JRAINBOW1".into()],
                },
            )]),
            lighting: LightingView {
                master_brightness: 80,
            },
        };
        let json = serde_json::to_value(&list).expect("serializes");
        assert_eq!(
            json["hidden"],
            serde_json::json!({ "hid:1462:7d75": { "device": false, "zones": ["JRAINBOW1"] } })
        );
        let d = &json["devices"][0];
        assert_eq!(d["description"]["info"]["kind"], serde_json::json!("Motherboard"));
        assert_eq!(d["description"]["info"]["bus"], serde_json::json!("Hid"));
        assert_eq!(
            d["description"]["zones"][0]["kind"],
            serde_json::json!({ "Matrix": { "width": 2, "height": 1, "map": [0, null] } })
        );
        assert_eq!(d["description"]["zones"][1]["kind"], serde_json::json!("Linear"));
        assert_eq!(
            d["description"]["zones"][1]["resizable"],
            serde_json::json!({ "start": 0, "end": 120 })
        );
        let mode = &d["description"]["modes"][0];
        assert_eq!(mode["caps"], serde_json::json!("SPEED | MODE_SPECIFIC_COLOR"));
        assert_eq!(mode["color_mode"], serde_json::json!("ModeSpecific"));
        assert_eq!(mode["speed"], serde_json::json!({ "start": 0, "end": 2 }));
        assert_eq!(d["description"]["direct_mode"], serde_json::Value::Null);
        assert_eq!(d["description"]["keepalive"], serde_json::Value::Null);
        assert_eq!(
            d["status"],
            serde_json::json!({ "state": "faulted", "error": "x" })
        );
        assert_eq!(
            d["setting"],
            serde_json::json!({ "kind": "direct", "colors": ["#FF0000", "#FF0000"] })
        );
        assert_eq!(d["colors"], serde_json::json!(["#FF0000", "#000000"]));
        assert_eq!(
            d["hardware"],
            serde_json::json!({ "mode": 0, "speed": 1, "brightness": null, "direction": null, "colors": [] })
        );
        assert_eq!(d["family"], serde_json::json!("msi_mystic_light"));
        assert_eq!(d["brightness"], serde_json::json!(55));
        assert_eq!(d["takeover"], serde_json::json!({ "mode_id": 1 }));
        assert_eq!(json["lighting"], serde_json::json!({ "master_brightness": 80 }));
        assert_eq!(
            json["failures"][0]["error"],
            serde_json::json!({ "Transport": "busy" })
        );
        assert_eq!(
            serde_json::to_value(DeviceStatus::Active).expect("serializes"),
            serde_json::json!({ "state": "active" })
        );
    }

    #[test]
    fn release_report_serializes() {
        let r = ReleaseReport {
            released: 2,
            failed: vec![cc_engine::ApplyFailure {
                id: DeviceId::hid(0x31E3, 0x1310, None),
                error: "busy".into(),
            }],
        };
        assert_eq!(
            serde_json::to_value(r).expect("serializes"),
            serde_json::json!({ "released": 2, "failed": [{ "id": "hid:31e3:1310", "error": "busy" }] })
        );
    }

    #[test]
    fn helper_sentences_are_shown_as_given() {
        let other =
            cc_conflicts::boot_task::other_account_message("S-1-5-21-1-2-3-500", "S-1-5-21-1-2-3-1001");
        let admin = cc_conflicts::boot_task::ADMIN_OWNED_REQUEST_MESSAGE;
        // The helper refuses both directions with the same sentences: switching on and switching off show them as
        // they are, and they never name the wrong action ("set up" when removing, "removed" when creating).
        for on in [true, false] {
            for sentence in [other.as_str(), admin] {
                let text = boot_task_error_text(on, sentence);
                assert_eq!(text, sentence, "on = {on}");
                for wrong in ["not set up", "not created", "not removed"] {
                    assert!(!text.contains(wrong), "on = {on}: {text}");
                }
            }
        }
        let not_ours =
            cc_system::not_ours_message("CaseControl Start-up S-1-5-21-1-2-3-1001", "it runs as x");
        assert_eq!(boot_task_error_text(false, &not_ours), not_ours);
        // A short reason gets a lead.
        assert_eq!(
            boot_task_error_text(true, "the Windows permission prompt was declined"),
            "The start-up task was not created: the Windows permission prompt was declined"
        );
        assert_eq!(
            boot_task_error_text(false, "access denied"),
            "The start-up task was not removed: access denied"
        );
    }

    #[test]
    fn apply_before_sign_in_follows_the_start_up_task() {
        use cc_system::TaskState;
        assert_eq!(boot_flag_from(&Ok(TaskState::Missing)), Some(false));
        assert_eq!(boot_flag_from(&Ok(TaskState::Foreign("x".into()))), Some(false));
        assert_eq!(boot_flag_from(&Ok(TaskState::Matches)), Some(true));
        assert_eq!(
            boot_flag_from(&Ok(TaskState::Differs(vec!["path".into()]))),
            Some(true)
        );
        assert_eq!(boot_flag_from(&Ok(TaskState::Disabled)), Some(true));
        assert_eq!(
            boot_flag_from(&Err("access denied".into())),
            None,
            "unreadable: left as it is"
        );
    }
}
