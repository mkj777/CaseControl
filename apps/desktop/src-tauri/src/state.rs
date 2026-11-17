//! The one managed [`AppState`]: storage locations, settings, the engine handle, the last conflict scan and the
//! shutdown latch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cc_config::{HardwareState, Paths, Profile, ProfileStore, Settings};
use cc_conflicts::ScanReport;
use cc_engine::{ApplyReport, EngineConfig, EngineHandle};
use parking_lot::{Mutex, MutexGuard};

use crate::prefs::{engine_config, start_brightness};
use crate::profiles::{ProfileList, profile_list};

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
        let config = EngineConfig {
            hardware_state,
            ..engine_config(&settings)
        };
        let engine = cc_engine::start(config, cc_drivers::all_backends()).map_err(|err| {
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
        }
    }

    /// Re-applies the last applied profile if it still exists (ADR 0001 #24), with its device brightness overlaid by
    /// the changes made since (ADR 0001 #33). Without one, devices stay in their hardware mode. With master brightness
    /// 0 saved, the engine outputs every device dark, so the profile is applied underneath without a coloured frame.
    pub fn apply_last_profile(&self) {
        let Some(name) = self.settings.lock().last_profile.clone() else {
            return;
        };
        let (Ok(engine), Ok(storage)) = (&self.engine, &self.storage) else {
            return;
        };
        match storage.profiles.load(&name) {
            Ok(profile) => {
                let brightness = start_brightness(Some(&profile), &self.settings());
                engine.replace_brightness(brightness);
                let report = self.apply_visible(engine, &profile);
                tracing::info!(
                    profile = %profile.name,
                    applied = report.applied.len(),
                    missing = report.missing.len(),
                    failed = report.failed.len(),
                    "last profile applied on start"
                );
            }
            Err(err) => tracing::warn!(profile = %name, %err, "last profile not applied"),
        }
    }

    /// Applies `profile` without its hidden devices and with its hidden zones black (ADR 0001 #26).
    pub fn apply_visible(&self, engine: &EngineHandle, profile: &Profile) -> ApplyReport {
        let (devices, skipped) = crate::hidden::visible_profile(&self.settings(), &profile.devices, |id| {
            engine.device(id).map(|v| v.description)
        });
        if !skipped.is_empty() {
            tracing::info!(profile = %profile.name, hidden = skipped.len(), "hidden devices skipped");
        }
        engine.apply(&devices)
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

    /// Releases every device and stops the engine, exactly once; later calls return immediately.
    pub fn shutdown_engine(&self) {
        if self.shut_down.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(engine) = &self.engine {
            tracing::info!("releasing devices and stopping the engine");
            engine.shutdown(SHUTDOWN_TIMEOUT);
        }
    }
}
