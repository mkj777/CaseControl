//! IPC commands callable from the frontend via `invoke`. Errors are returned as strings, which is what the
//! frontend receives as the rejected promise value.
//!
//! Commands that touch devices, files or the system run on Tauri's blocking pool (`blocking`) so a slow device
//! acknowledgement (up to 3 s) never stalls the window's event loop. The logic they share with the tray lives in
//! the `*_now` functions.

use std::collections::{BTreeMap, BTreeSet};

use cc_config::Profile;
use cc_core::{DeviceId, ModeState, Rgb};
use cc_engine::{ApplyReport, DetectFailure, DeviceSetting, DeviceStatus, DeviceView, EngineHandle};
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_autostart::ManagerExt as _;

use crate::conflict_actions::{
    ActionReport, check_catalogue_id, conflict_view, known_devices, run_and_report, run_scan,
    scan_and_publish, set_dont_ask_in,
};
use crate::conflicts::ConflictScan;
use crate::events::{self, CONFLICTS_UPDATED, DEVICES_CHANGED, PROFILES_CHANGED};
use crate::hidden::{
    HiddenEntry, hidden_device_error, hidden_view, hidden_zones, is_device_hidden, mask_hidden_zones,
    zone_write,
};
use crate::prefs::{SettingsPatch, SettingsView, apply_patch, record_zone_size};
use crate::profiles::{clears_last, last_after_rename};
use crate::startup::{
    CloseTarget, ResolveRequest, StartupConflicts, plan_autostart, plan_close, plan_resolve,
};
use crate::state::AppState;

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
}

/// Outcome of releasing one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseResult {
    /// The device.
    pub id: DeviceId,
    /// Why the release failed, if it did.
    pub error: Option<String>,
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
    let hidden = hidden_view(&state.settings());
    match state.engine() {
        Ok(engine) => DeviceList {
            devices: engine.devices(),
            failures: engine.failures(),
            engine_error: None,
            hidden,
        },
        Err(_) => DeviceList {
            devices: Vec::new(),
            failures: Vec::new(),
            engine_error: state.engine_error(),
            hidden,
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

/// Applies profile `name` and records it as `last_profile`.
pub fn apply_profile_now<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    name: &str,
) -> Result<ApplyReport, String> {
    let engine = state.engine()?;
    let profile = state.storage()?.profiles.load(name).map_err(err_string)?;
    let report = state.apply_visible(engine, &profile);
    tracing::info!(
        profile = %profile.name,
        applied = report.applied.len(),
        missing = report.missing.len(),
        failed = report.failed.len(),
        "profile applied"
    );
    let saved = state.update_settings(|s| {
        s.last_profile = Some(profile.name.clone());
        Ok(())
    });
    profiles_changed(app, state);
    saved.map_err(|err| format!("The profile was applied, but it could not be remembered: {err}"))?;
    Ok(report)
}

/// Releases every device that is not faulted.
pub fn release_all_now(state: &AppState) -> Result<Vec<ReleaseResult>, String> {
    Ok(state
        .engine()?
        .release_all()
        .into_iter()
        .map(|(id, r)| ReleaseResult {
            id,
            error: r.err().map(|e| e.to_string()),
        })
        .collect())
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
        engine.set_direct(&id, colors).map_err(err_string)
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
            return engine.set_zone(&id, zone, colors).map_err(err_string);
        }
        // The engine's buffer may still hold colours of a hidden zone (e.g. from before it was hidden), so send the
        // whole masked buffer instead of patching one zone into it.
        let view = describe(engine, &id)?;
        let buffer = zone_write(&view.description, &hidden, &view.colors, zone, &colors)?;
        engine.set_direct(&id, buffer).map_err(err_string)
    })
    .await
}

/// Switches a device to a hardware mode. Refused for a hidden device.
#[tauri::command]
pub async fn set_mode<R: Runtime>(app: AppHandle<R>, id: DeviceId, state: ModeState) -> Result<(), String> {
    blocking(app, move |_, st| {
        let engine = st.engine()?;
        visible_device(st, &id)?;
        engine.set_mode(&id, state).map_err(err_string)
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

/// Hands every device back to its hardware mode.
#[tauri::command]
pub async fn release_all<R: Runtime>(app: AppHandle<R>) -> Result<Vec<ReleaseResult>, String> {
    blocking(app, |_, state| release_all_now(state)).await
}

/// Hides or unhides a device (ADR 0001 #26) and saves that. Hiding releases the device to its own hardware mode;
/// unhiding changes nothing on the hardware. Returns the new device list.
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
        if hidden
            && let Ok(engine) = state.engine()
            && engine
                .device(&id)
                .is_some_and(|v| v.status == DeviceStatus::Active)
        {
            engine
                .release(&id)
                .map_err(|err| format!("The device is hidden, but it could not be released: {err}"))?;
        }
        Ok(device_list_now(state))
    })
    .await
}

/// Hides or unhides zone `zone_name` of a device (ADR 0001 #26) and saves that. If the device shows direct colours,
/// hiding a zone switches it off right away; unhiding leaves it off until the next colour change. Returns the new
/// device list.
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
        state.update_settings(|s| {
            crate::hidden::set_zone_hidden(s, &id, &zone_name, hidden);
            Ok(())
        })?;
        tracing::info!(%id, zone = %zone_name, hidden, "zone visibility changed");
        let (device_hidden, zones) = {
            let settings = state.settings();
            (is_device_hidden(&settings, &id), hidden_zones(&settings, &id))
        };
        if hidden
            && !device_hidden
            && view.status == DeviceStatus::Active
            && matches!(view.setting, Some(DeviceSetting::Direct { .. }))
        {
            let mut colors = view.colors;
            mask_hidden_zones(&view.description, &zones, &mut colors);
            engine
                .set_direct(&id, colors)
                .map_err(|err| format!("The zone is hidden, but it could not be switched off: {err}"))?;
        }
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
pub async fn list_profiles<R: Runtime>(app: AppHandle<R>) -> Result<crate::profiles::ProfileList, String> {
    blocking(app, |_, state| state.profile_list()).await
}

/// Saves what the devices show now as profile `name` (replacing a profile of the same name).
#[tauri::command]
pub async fn save_profile<R: Runtime>(
    app: AppHandle<R>,
    name: String,
) -> Result<crate::profiles::ProfileList, String> {
    blocking(app, move |app, state| {
        let profile = Profile {
            name: name.trim().to_owned(),
            devices: state.engine()?.current_settings(),
        };
        state.storage()?.profiles.save(&profile).map_err(err_string)?;
        tracing::info!(profile = %profile.name, devices = profile.devices.len(), "profile saved");
        profiles_changed(app, state);
        state.profile_list()
    })
    .await
}

/// Applies profile `name` and remembers it for the next start.
#[tauri::command]
pub async fn apply_profile<R: Runtime>(app: AppHandle<R>, name: String) -> Result<ApplyReport, String> {
    blocking(app, move |app, state| apply_profile_now(app, state, &name)).await
}

/// Renames profile `from` to `to`; `last_profile` follows.
#[tauri::command]
pub async fn rename_profile<R: Runtime>(
    app: AppHandle<R>,
    from: String,
    to: String,
) -> Result<crate::profiles::ProfileList, String> {
    blocking(app, move |app, state| {
        state.storage()?.profiles.rename(&from, &to).map_err(err_string)?;
        let saved = state.update_settings(|s| {
            if let Some(next) = last_after_rename(s.last_profile.as_deref(), &from, &to) {
                s.last_profile = Some(next);
            }
            Ok(())
        });
        profiles_changed(app, state);
        saved?;
        state.profile_list()
    })
    .await
}

/// Deletes profile `name`; clears `last_profile` if it was that one.
#[tauri::command]
pub async fn delete_profile<R: Runtime>(
    app: AppHandle<R>,
    name: String,
) -> Result<crate::profiles::ProfileList, String> {
    blocking(app, move |app, state| {
        state.storage()?.profiles.delete(&name).map_err(err_string)?;
        let saved = state.update_settings(|s| {
            if clears_last(s.last_profile.as_deref(), &name) {
                s.last_profile = None;
            }
            Ok(())
        });
        profiles_changed(app, state);
        saved?;
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

/// The Settings page's data.
#[tauri::command]
pub async fn get_settings<R: Runtime>(app: AppHandle<R>) -> Result<SettingsView, String> {
    blocking(app, |_, state| {
        Ok(SettingsView::of(&state.settings(), state.settings_error()))
    })
    .await
}

/// Applies a partial settings change, saves it, and carries it over to the running app: frame rate and detector
/// switches go to the engine, autostart to the HKCU Run key.
#[tauri::command]
pub async fn set_settings<R: Runtime>(
    app: AppHandle<R>,
    patch: SettingsPatch,
) -> Result<SettingsView, String> {
    blocking(app, move |app, state| {
        let effects = state.update_settings(|s| {
            let effects = apply_patch(s, &patch)?;
            if let Some(on) = effects.autostart {
                let autolaunch = app.autolaunch();
                let result = if on {
                    autolaunch.enable()
                } else {
                    autolaunch.disable()
                };
                result.map_err(|err| format!("cannot change autostart: {err}"))?;
                tracing::info!(enabled = on, "autostart changed");
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
        }
        Ok(SettingsView::of(&state.settings(), state.settings_error()))
    })
    .await
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
                },
                Zone {
                    name: "JRAINBOW1".into(),
                    kind: ZoneKind::Linear,
                    start: 1,
                    len: 1,
                    resizable: Some(0..=120),
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
                family: "msi_mystic_light".into(),
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
        assert_eq!(d["family"], serde_json::json!("msi_mystic_light"));
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
    fn release_result_serializes() {
        let r = ReleaseResult {
            id: DeviceId::hid(0x31E3, 0x1310, None),
            error: None,
        };
        assert_eq!(
            serde_json::to_value(r).expect("serializes"),
            serde_json::json!({ "id": "hid:31e3:1310", "error": null })
        );
    }
}
