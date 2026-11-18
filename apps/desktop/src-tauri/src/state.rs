//! The one managed [`AppState`]: storage locations, settings, the engine handle, the active profile's autosave, the
//! last conflict scan and the shutdown latch.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cc_config::{HardwareState, Paths, Profile, ProfileStore, Settings};
use cc_conflicts::ScanReport;
use cc_engine::{ApplyReport, EngineHandle};
use parking_lot::{Mutex, MutexGuard};

use crate::autosave::{AUTOSAVE_INTERVAL, Autosave};
use crate::prefs::{clear_brightness_overrides, engine_config, start_brightness};
use crate::profiles::{ProfileList, StartPlan, merge_snapshot, profile_list, start_plan};

/// Time the engine gets to release every device on exit.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// Where settings and profiles live.
#[derive(Debug)]
pub struct Storage {
    /// Directories.
    pub paths: Paths,
    /// Profile files.
    pub profiles: ProfileStore,
}

/// Everything the commands, the tray and the exit paths share.
#[derive(Debug)]
pub struct AppState {
    storage: Result<Storage, String>,
    settings: Mutex<Settings>,
    /// Set when settings must not be saved this session (unreadable file or no storage).
    settings_error: Option<String>,
    engine: Result<EngineHandle, String>,
    last_scan: Mutex<Option<ScanReport>>,
    shut_down: AtomicBool,
    /// Marks changes of the active profile and times their writes (ADR 0001 #40).
    autosave: Autosave,
    /// Held by every profile operation (switch, create, duplicate, rename, delete) and every autosave write, so a
    /// write always goes to the profile that was active when the change was made.
    profile_lock: Mutex<()>,
}

impl AppState {
    /// Resolves and creates the storage folders, loads the settings and the machine state (`hardware-state.toml`)
    /// and starts the engine with every compiled-in backend. Never fails: a storage problem leaves the defaults in
    /// place (nothing is saved), an engine start failure is kept as a message for the Devices page.
    pub fn init() -> Self {
        let storage = match Paths::from_env() {
            Some(paths) => match paths.ensure() {
                Ok(()) => Ok(Storage {
                    profiles: ProfileStore::new(&paths),
                    paths,
                }),
                Err(err) => Err(format!("cannot create the settings folders: {err}")),
            },
            None => Err("cannot determine the user's AppData folders".to_owned()),
        };
        let (settings, settings_error) = match &storage {
            Ok(s) => match Settings::load(&s.paths) {
                Ok(settings) => (settings, None),
                Err(err) => {
                    // The file exists but cannot be read: never overwrite it with defaults this session.
                    tracing::error!(%err, "cannot read settings; using defaults and not saving settings this session");
                    (
                        Settings::default(),
                        Some(format!(
                            "The settings file could not be read ({err}). CaseControl runs with default settings                              and will not save settings until it is restarted."
                        )),
                    )
                }
            },
            Err(err) => {
                tracing::error!(%err, "no storage; settings and profiles will not be saved");
                (
                    Settings::default(),
                    Some(format!("Settings cannot be saved: {err}")),
                )
            }
        };
        // The machine state (last good own lighting per device, ADR 0001 #38) lives next to the settings; without
        // storage it is kept in memory for this session only.
        let hardware_state = match &storage {
            Ok(s) => HardwareState::load(&s.paths),
            Err(_) => HardwareState::in_memory(),
        };
        let engine = cc_engine::start(
            engine_config(&settings, hardware_state),
            cc_drivers::all_backends(),
        )
        .map_err(|err| {
            tracing::error!(%err, "engine failed to start");
            format!("The device engine could not start: {err}")
        });
        Self {
            storage,
            settings: Mutex::new(settings),
            settings_error,
            engine,
            last_scan: Mutex::new(None),
            shut_down: AtomicBool::new(false),
            autosave: Autosave::new(AUTOSAVE_INTERVAL),
            profile_lock: Mutex::new(()),
        }
    }

    /// Makes sure exactly one profile is active and applies it (ADR 0001 #24, #40). Without any profile, "Profile 1"
    /// is created from what the devices show now (what saving a profile used to write; often empty) and becomes
    /// active. With profiles but no valid `last_profile`, the first in list order becomes active. The active profile
    /// is applied with its device brightness; brightness changes an older version kept in the settings are laid over
    /// it, saved into it and cleared. With master brightness 0 saved, the engine outputs every device dark, so the
    /// profile is applied underneath without a coloured frame.
    pub fn start_profiles(&self) {
        let Ok(storage) = &self.storage else {
            return;
        };
        let names = match storage.profiles.list() {
            Ok(names) => names,
            Err(err) => {
                tracing::error!(%err, "cannot list profiles; no profile applied");
                return;
            }
        };
        let last = self.settings().last_profile.clone();
        let _lock = self.profile_lock();
        match start_plan(&names, last.as_deref()) {
            StartPlan::Create(name) => {
                let (devices, brightness) = match &self.engine {
                    Ok(engine) => (engine.current_settings(), engine.brightness()),
                    Err(_) => (BTreeMap::new(), start_brightness(None, &self.settings())),
                };
                let profile = Profile {
                    name,
                    devices,
                    brightness,
                };
                if let Err(err) = storage.profiles.save(&profile) {
                    tracing::error!(%err, profile = %profile.name, "cannot create the first profile");
                    return;
                }
                tracing::info!(profile = %profile.name, devices = profile.devices.len(), "first profile created");
                if let Err(err) = self.set_active_profile(&profile.name) {
                    tracing::warn!(%err, "the first profile is active, but this could not be saved");
                }
                self.clear_legacy_brightness();
            }
            StartPlan::Apply { name, remember } => {
                let profile = match storage.profiles.load(&name) {
                    Ok(profile) => profile,
                    Err(err) => {
                        tracing::warn!(profile = %name, %err, "active profile not applied");
                        return;
                    }
                };
                if remember {
                    tracing::info!(profile = %profile.name, "no valid last profile; activating the first one");
                    if let Err(err) = self.set_active_profile(&profile.name) {
                        tracing::warn!(%err, "the profile is active, but this could not be saved");
                    }
                }
                let Ok(engine) = &self.engine else {
                    return;
                };
                let settings = self.settings().clone();
                let legacy = settings.devices.values().any(|ov| ov.brightness.is_some());
                engine.replace_brightness(start_brightness(Some(&profile), &settings));
                let report = self.apply_visible(engine, &profile);
                tracing::info!(
                    profile = %profile.name,
                    applied = report.applied.len(),
                    missing = report.missing.len(),
                    failed = report.failed.len(),
                    "active profile applied on start"
                );
                if legacy {
                    match self.write_profile_snapshot(&profile.name) {
                        Ok(()) => self.clear_legacy_brightness(),
                        Err(err) => {
                            tracing::warn!(%err, "cannot save the kept device brightness into the profile")
                        }
                    }
                }
            }
        }
    }

    /// Clears device brightness an older version kept in the settings (it is part of the active profile now).
    fn clear_legacy_brightness(&self) {
        if !self.settings().devices.values().any(|ov| ov.brightness.is_some()) {
            return;
        }
        let cleared = self.update_settings(|s| {
            clear_brightness_overrides(s);
            Ok(())
        });
        if let Err(err) = cleared {
            tracing::warn!(%err, "cannot clear the kept device brightness");
        }
    }

    /// Makes `name` the active profile (`last_profile`). It is always set in memory, so autosave writes go to it; the
    /// error says that it could not be saved for the next start.
    pub fn set_active_profile(&self, name: &str) -> Result<(), String> {
        let name = name.trim().to_owned();
        let saved = self.update_settings(|s| {
            s.last_profile = Some(name.clone());
            Ok(())
        });
        if saved.is_err() {
            self.settings.lock().last_profile = Some(name);
        }
        saved
    }

    /// Takes the profile lock; hold it across a profile operation and pass it to the `*_locked` methods.
    pub fn profile_lock(&self) -> MutexGuard<'_, ()> {
        self.profile_lock.lock()
    }

    /// Records that what a device shows changed; the active profile is saved shortly (ADR 0001 #40).
    pub fn profile_changed(&self) {
        self.autosave.mark();
    }

    /// Writes a pending change of the active profile now. The caller holds the profile lock.
    pub fn flush_profile_locked(&self, _lock: &MutexGuard<'_, ()>) {
        self.autosave.flush(|| {
            let active = self.settings().last_profile.clone();
            if let Some(name) = active
                && let Err(err) = self.write_profile_snapshot(&name)
            {
                tracing::error!(profile = %name, %err, "cannot save the active profile");
            }
        });
    }

    /// Writes a pending change of the active profile now.
    pub fn flush_profile(&self) {
        let lock = self.profile_lock();
        self.flush_profile_locked(&lock);
    }

    /// The autosave writer: writes the active profile whenever a change is due, until shutdown. Runs on its own
    /// thread.
    pub fn run_autosave(&self) {
        while self.autosave.wait_due() {
            self.flush_profile();
        }
        tracing::debug!("autosave stopped");
    }

    /// Saves what the devices show and their brightness into profile `name` (merged, see [`merge_snapshot`]);
    /// recreates the profile if its file is gone. Writes nothing when nothing changed.
    fn write_profile_snapshot(&self, name: &str) -> Result<(), String> {
        let storage = self.storage()?;
        let engine = self.engine()?;
        let mut profile = match storage.profiles.load(name) {
            Ok(profile) => profile,
            Err(err) => {
                tracing::warn!(profile = %name, %err, "active profile not found; writing it anew");
                Profile {
                    name: name.to_owned(),
                    devices: BTreeMap::new(),
                    brightness: BTreeMap::new(),
                }
            }
        };
        let before = profile.clone();
        merge_snapshot(&mut profile, engine.current_settings(), engine.brightness());
        if profile == before {
            return Ok(());
        }
        storage.profiles.save(&profile).map_err(|err| err.to_string())?;
        tracing::debug!(profile = %profile.name, "active profile saved");
        Ok(())
    }

    /// Applies profile `name` with its device brightness and makes it active (ADR 0001 #40). The caller holds the
    /// profile lock and has flushed the previous profile's pending change. Without an engine only the active profile
    /// changes.
    pub fn switch_profile_locked(
        &self,
        _lock: &MutexGuard<'_, ()>,
        name: &str,
    ) -> Result<ApplyReport, String> {
        let profile = self
            .storage()?
            .profiles
            .load(name)
            .map_err(|err| err.to_string())?;
        let report = match &self.engine {
            Ok(engine) => {
                engine.replace_brightness(profile.brightness.clone());
                self.apply_visible(engine, &profile)
            }
            Err(_) => ApplyReport::default(),
        };
        tracing::info!(
            profile = %profile.name,
            applied = report.applied.len(),
            missing = report.missing.len(),
            failed = report.failed.len(),
            "profile applied"
        );
        self.set_active_profile(&profile.name)
            .map_err(|err| format!("The profile was applied, but it could not be remembered: {err}"))?;
        Ok(report)
    }

    /// Applies `profile` without its hidden devices and with its hidden zones black (ADR 0001 #26). Connected
    /// devices it has no entry for fall back to their own mode, else the fallback colour (ADR 0001 #40).
    pub fn apply_visible(&self, engine: &EngineHandle, profile: &Profile) -> ApplyReport {
        let (devices, skipped) = crate::hidden::visible_profile(&self.settings(), &profile.devices, |id| {
            engine.device(id).map(|v| v.description)
        });
        if !skipped.is_empty() {
            tracing::info!(profile = %profile.name, hidden = skipped.len(), "hidden devices skipped");
        }
        engine.apply_replacing(&devices)
    }

    /// The running engine, or why there is none.
    pub fn engine(&self) -> Result<&EngineHandle, String> {
        self.engine.as_ref().map_err(Clone::clone)
    }

    /// Why the engine did not start, if it did not.
    pub fn engine_error(&self) -> Option<String> {
        self.engine.as_ref().err().cloned()
    }

    /// Settings and profile storage, or why it is unavailable.
    pub fn storage(&self) -> Result<&Storage, String> {
        self.storage.as_ref().map_err(Clone::clone)
    }

    /// The current settings.
    pub fn settings(&self) -> MutexGuard<'_, Settings> {
        self.settings.lock()
    }

    /// Why settings are not saved this session, if they are not.
    pub fn settings_error(&self) -> Option<String> {
        self.settings_error.clone()
    }

    /// Runs `change` on a copy of the settings and saves the copy; the in-memory settings only change if both
    /// succeed. Holds the settings lock throughout, so concurrent updates do not lose each other's changes. Refused
    /// while [`settings_error`](Self::settings_error) is set, so an unreadable config is never overwritten.
    pub fn update_settings<T>(
        &self,
        change: impl FnOnce(&mut Settings) -> Result<T, String>,
    ) -> Result<T, String> {
        if let Some(err) = &self.settings_error {
            return Err(err.clone());
        }
        let storage = self.storage()?;
        let mut guard = self.settings.lock();
        let mut next = guard.clone();
        let out = change(&mut next)?;
        next.save(&storage.paths)
            .map_err(|err| format!("cannot save settings: {err}"))?;
        *guard = next;
        Ok(out)
    }

    /// The profile names with the active one.
    pub fn profile_list(&self) -> Result<ProfileList, String> {
        let names = self.storage()?.profiles.list().map_err(|err| err.to_string())?;
        Ok(profile_list(names, self.settings().last_profile.as_deref()))
    }

    /// Stores the latest conflict scan (actions are checked against it).
    pub fn set_last_scan(&self, report: ScanReport) {
        *self.last_scan.lock() = Some(report);
    }

    /// Runs `f` on the latest conflict scan.
    pub fn with_last_scan<T>(&self, f: impl FnOnce(&ScanReport) -> Result<T, String>) -> Result<T, String> {
        match self.last_scan.lock().as_ref() {
            Some(report) => f(report),
            None => Err("no conflict scan yet; scan first".to_owned()),
        }
    }

    /// Writes a pending change of the active profile, then releases every device and stops the engine, exactly
    /// once; later calls return immediately.
    pub fn shutdown_engine(&self) {
        if self.shut_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.autosave.stop();
        self.flush_profile();
        if let Ok(engine) = &self.engine {
            tracing::info!("releasing devices and stopping the engine");
            engine.shutdown(SHUTDOWN_TIMEOUT);
        }
    }
}
