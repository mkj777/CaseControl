//! The one managed [`AppState`]: storage locations, settings, the engine handle, the active profile's autosave, the
//! last conflict scan and the shutdown latch.
//!
//! The engine starts with the active profile already seeded (ADR 0001 #42, early apply): [`AppState::init`] loads it
//! before the engine opens any device, so each device gets its setting the moment it is found; [`start_profiles`]
//! then only does the bookkeeping instead of applying it a second time. Before it loads the machine state and starts
//! the engine, the signed-in instance takes the cross-session device lock ([`crate::power::take_device_lock`], which
//! first asks a running start-up instance to finish) and holds it until [`shutdown_engine`] stopped the engine; so it
//! reads the machine state only after a start-up instance wrote its last records.
//!
//! [`start_profiles`]: AppState::start_profiles
//! [`shutdown_engine`]: AppState::shutdown_engine

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cc_config::{HardwareState, Paths, Profile, ProfileStore, Settings};
use cc_conflicts::ScanReport;
use cc_engine::{ApplyReport, EngineHandle, PersistReport};
use parking_lot::{Mutex, MutexGuard};

use crate::autosave::{AUTOSAVE_INTERVAL, Autosave};
use crate::power::{ExitPlan, exit_plan};
use crate::prefs::{PowerSettings, clear_brightness_overrides, engine_config_for, start_brightness};
use crate::profiles::{ProfileList, StartPlan, merge_snapshot, profile_list, start_plan};

/// Time the engine gets to release every device on an explicit exit.
pub const SHUTDOWN_TIMEOUT: Duration = crate::power::STOP_TIMEOUT;

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
    /// The profile whose settings the engine was started with (ADR 0001 #42, early apply).
    seeded_profile: Option<String>,
    /// The line under "Save colours to the devices" after it was last switched this session.
    save_note: Mutex<Option<String>>,
    /// Serialises the "Save colours to the devices" switch (a save and a restore never overlap).
    persist_lock: Mutex<()>,
    /// Serialises the start-up autostart check with the "Start with Windows" switch, so the check never acts on a
    /// setting the user changed meanwhile.
    autostart_lock: Mutex<()>,
    /// Serialises the start-up check of the start-up task with the "Apply colours before sign-in" switch.
    boot_task_lock: Mutex<()>,
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
        // A start-up instance (`--apply-at-boot`) may still be driving the devices and writing the machine state; ask
        // it to finish and wait for it before the machine state is read.
        crate::power::take_device_lock(crate::power::SESSION_LOCK_WAIT);
        // The machine state (last good own lighting per device, ADR 0001 #38, and the records of saving to the
        // devices) lives in the local app-data folder; without storage it is kept in memory for this session only.
        let hardware_state = match &storage {
            Ok(s) => HardwareState::load(&s.paths),
            Err(_) => HardwareState::in_memory(),
        };
        // Early apply (ADR 0001 #42): the active profile, as the start will pick it, is handed to the engine so each
        // device gets its setting as soon as it is found.
        let seed = storage
            .as_ref()
            .ok()
            .and_then(|s| active_profile_for_start(s, &settings));
        if let Some(profile) = &seed {
            tracing::info!(profile = %profile.name, devices = profile.devices.len(), "active profile seeded into the engine");
        }
        let engine = cc_engine::start(
            engine_config_for(&settings, hardware_state, seed.as_ref()),
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
            seeded_profile: seed.map(|p| p.name),
            save_note: Mutex::new(None),
            persist_lock: Mutex::new(()),
            autostart_lock: Mutex::new(()),
            boot_task_lock: Mutex::new(()),
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
                let seeded = self
                    .seeded_profile
                    .as_deref()
                    .is_some_and(|seeded| crate::profiles::same_name(seeded, &profile.name));
                if seeded {
                    // The engine got this profile and its brightness before detection and drove each device as it
                    // was found; applying it again would only resend the same frames.
                    tracing::info!(profile = %profile.name, "active profile applied at detection");
                } else {
                    engine.replace_brightness(start_brightness(Some(&profile), &settings));
                    let report = self.apply_visible(engine, &profile);
                    tracing::info!(
                        profile = %profile.name,
                        applied = report.applied.len(),
                        missing = report.missing.len(),
                        failed = report.failed.len(),
                        "active profile applied on start"
                    );
                }
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

    /// Takes the autostart lock: hold it while the logon task is checked or changed.
    pub fn autostart_lock(&self) -> MutexGuard<'_, ()> {
        self.autostart_lock.lock()
    }

    /// Takes the start-up task lock: hold it while the start-up task is checked or changed.
    pub fn boot_task_lock(&self) -> MutexGuard<'_, ()> {
        self.boot_task_lock.lock()
    }

    /// The line under "Save colours to the devices", if any.
    pub fn save_note(&self) -> Option<String> {
        self.save_note.lock().clone()
    }

    /// Turns "Save colours to the devices" on or off (ADR 0001 #42): saves the switch, then saves the colours to
    /// every capable device (on) or writes their original saved state back (off; devices that are not connected are
    /// restored when they are next connected). The result per device is logged; the devices that were not saved or
    /// restored are kept as the line Settings shows under the switch. Fails without writing any device when the
    /// switch cannot be saved.
    pub fn set_save_to_devices(&self, on: bool) -> Result<(), String> {
        let _serial = self.persist_lock.lock();
        self.update_settings(|s| {
            s.save_to_devices = on;
            Ok(())
        })?;
        tracing::info!(on, "save colours to the devices switched");
        let Ok(engine) = &self.engine else {
            *self.save_note.lock() = None;
            return Ok(());
        };
        let timeout = crate::power::SWITCH_PERSIST_TIMEOUT;
        let report = if on {
            engine.persist_all(timeout)
        } else {
            engine.restore_saved(timeout)
        };
        *self.save_note.lock() = self.note_for(engine, &report, !on);
        Ok(())
    }

    fn note_for(&self, engine: &EngineHandle, report: &PersistReport, restoring: bool) -> Option<String> {
        crate::power::log_report(report, restoring);
        crate::power::persist_note(report, restoring, |id| {
            engine
                .device(id)
                .map(|d| d.description.info.name)
                .unwrap_or_else(|| id.to_string())
        })
    }

    /// Writes a pending change of the active profile, then, depending on why the app closes (ADR 0001 #42, see
    /// [`crate::power::exit_plan`]), saves the colours to the devices, releases the devices or keeps CaseControl's
    /// colours on them, and stops the engine, exactly once; later calls return immediately. Lets go of the device
    /// lock afterwards. The raw session-end flags and the decision are logged.
    pub fn shutdown_engine(&self) {
        self.shutdown_engine_with(None);
    }

    /// Like [`shutdown_engine`](Self::shutdown_engine), for the uninstaller's clean-up (ADR 0001 #42): releases every
    /// device and never saves to them, whatever the switches say.
    pub fn shutdown_engine_for_uninstall(&self) {
        self.shutdown_engine_with(Some(ExitPlan::uninstall()));
    }

    fn shutdown_engine_with(&self, forced: Option<ExitPlan>) {
        if self.shut_down.swap(true, Ordering::AcqRel) {
            return;
        }
        let why = cc_system::session_end::current();
        let power = PowerSettings::of(&self.settings());
        let plan =
            forced.unwrap_or_else(|| exit_plan(why, power.keep_colors_on_sign_out, power.save_to_devices));
        tracing::info!(
            message = ?cc_system::session_end::last_message(),
            shutting_down = cc_system::session_end::shutting_down(),
            ?why,
            uninstall = forced.is_some(),
            keep_colors_on_sign_out = power.keep_colors_on_sign_out,
            save_to_devices = power.save_to_devices,
            mode = ?plan.mode,
            persist = plan.persist,
            "exit decision"
        );
        self.autosave.stop();
        self.flush_profile();
        if let Ok(engine) = &self.engine {
            self.stop_engine(engine, &plan);
        }
        crate::power::drop_device_lock();
    }

    /// Saves to the devices when the plan says so, then stops the engine. A running switch of "Save colours to the
    /// devices" is waited for at most `plan.switch_wait` (its save or restore goes on in the engine; a restore has
    /// marked its devices pending already); after that the save is skipped, and at a session end the time waited is
    /// taken from the save's budget.
    fn stop_engine(&self, engine: &EngineHandle, plan: &ExitPlan) {
        if plan.persist {
            let waiting = std::time::Instant::now();
            match self.persist_lock.try_lock_for(plan.switch_wait) {
                Some(_serial) => {
                    let budget = if plan.switch_wait < crate::power::QUIT_SWITCH_WAIT {
                        plan.persist_timeout.saturating_sub(waiting.elapsed())
                    } else {
                        plan.persist_timeout
                    };
                    tracing::info!(
                        budget_ms = budget.as_millis() as u64,
                        "saving the colours to the devices before exit"
                    );
                    let report = engine.persist_all(budget);
                    crate::power::log_report(&report, false);
                }
                None => tracing::warn!(
                    waited_ms = plan.switch_wait.as_millis() as u64,
                    "the save colours switch is still running; not saving again before exit"
                ),
            }
        }
        tracing::info!(mode = ?plan.mode, "stopping the engine");
        engine.shutdown_with(plan.mode, plan.stop_timeout);
    }
}

/// The profile the start will make active ([`start_plan`]), loaded read-only, or `None` when there is none yet or it
/// cannot be read (the start then creates or reports it as before).
pub fn active_profile_for_start(storage: &Storage, settings: &Settings) -> Option<Profile> {
    let names = storage
        .profiles
        .list()
        .map_err(|err| tracing::warn!(%err, "cannot list profiles; nothing seeded"))
        .ok()?;
    match start_plan(&names, settings.last_profile.as_deref()) {
        StartPlan::Create(_) => None,
        StartPlan::Apply { name, .. } => storage
            .profiles
            .load(&name)
            .map_err(
                |err| tracing::warn!(profile = %name, %err, "cannot load the active profile; nothing seeded"),
            )
            .ok(),
    }
}
