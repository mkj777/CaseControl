//! The start-up task's mode, `--apply-at-boot` (ADR 0001 #42, "Apply colours before sign-in").
//!
//! The Task Scheduler start-up task runs this executable at boot as the signed-in user with a limited token (S4U,
//! session 0), before anyone signs in. [`boot_main`] catches the flag in `main` before [`crate::run`], so no Tauri,
//! webview, tray, single-instance guard, updater, autosave or conflict check exists in this process. It:
//!
//! 1. resolves the user's config and log folders ([`resolve_paths`]): the known folders when they hold the settings,
//!    else the profile folder from `ProfileList\<own SID>` (the user's profile may not be loaded under S4U), and logs
//!    which one it used, the session, the token's elevation type and integrity level to its own file
//!    `boot-apply-YYYY-MM-DD.log` in the logs folder;
//! 2. opens the per-user yield signal, takes the cross-session device lock without waiting and exits at once when the
//!    signed-in instance holds it or has already asked it to finish;
//! 3. reads the settings and the active profile read-only and bounded ([`read_settings`], [`read_bounded`], at most
//!    [`MAX_FILE_BYTES`] per file; it never creates or writes settings or profiles, and does not move an unreadable
//!    settings file aside); the machine state is loaded through `cc-config`'s bounded reads (a file over
//!    [`cc_config::MAX_STATE_FILE_BYTES`] is not read and kept as it is; only an oversized `device-memory.toml`
//!    makes device memory read-only);
//! 4. starts the engine with hot-plug on, device-memory writes off and the visible active profile and its brightness
//!    as initial settings, waits (at most [`BOOT_WAIT`]) until the profile's devices are present and driven
//!    ([`still_waiting`]; devices of switched-off families and absent ones that cannot arrive later are not waited
//!    for), stops it with `ShutdownMode::Keep`, lets go of the lock and exits. When the signed-in instance asks it to
//!    finish (the yield signal), it stops waiting at once and keeps what it applied so far.
//!
//! It never writes to the devices' own memory: saving colours to them and restoring their saved state are off for
//! this engine (`EngineConfig::device_memory_writes`), pending restores wait for the signed-in app. A device that
//! cannot be opened in session 0 (NvAPI likely) shows up in the engine's detection failures and is logged and
//! skipped.
//!
//! [`malformed_mode_invocation`] makes `main` fail closed: a command line that names `--apply-at-boot` or
//! `--uninstall-cleanup` in any other than their exact forms ends at once with [`EXIT_BAD_INVOCATION`].

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cc_config::{DeviceSetting, HardwareState, Paths, Profile, Settings};
use cc_core::DeviceId;
use cc_engine::{DeviceStatus, DeviceView, ShutdownMode};
use cc_system::{DeviceLock, LockStatus, SignalStatus, YieldSignal};

use crate::profiles::{StartPlan, start_plan};

/// How long the start-up instance waits for the profile's devices to be present and driven.
pub const BOOT_WAIT: Duration = Duration::from_secs(60);
/// How often it looks.
const POLL: Duration = Duration::from_millis(250);
/// Time the engine gets to leave every device showing its colours.
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

/// Exit code: done (applied, or nothing to do).
pub const EXIT_OK: i32 = 0;
/// Exit code: the user's folders or SID could not be determined, or the engine did not start.
pub const EXIT_FAILED: i32 = 1;
/// Exit code: a command line that names a headless mode in another than its exact form, or a window start in
/// session 0. Nothing was started.
pub const EXIT_BAD_INVOCATION: i32 = 2;
/// The largest settings, profile or machine-state file the start-up instance reads (1 MiB).
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Whether the command line (with the program name) names `--apply-at-boot` or `--uninstall-cleanup` (in any case,
/// also inside a longer argument) without being one of their exact forms. `main` then exits with
/// [`EXIT_BAD_INVOCATION`] before anything starts.
pub fn malformed_mode_invocation(args: &[OsString]) -> bool {
    let names_mode = args.iter().skip(1).any(|arg| {
        let arg = arg.to_string_lossy().to_ascii_lowercase();
        arg.contains(cc_system::BOOT_TASK_ARGUMENT) || arg.contains(crate::cleanup::UNINSTALL_CLEANUP_FLAG)
    });
    names_mode
        && !cc_system::is_apply_at_boot_invocation(args.iter().cloned())
        && crate::cleanup::parse_cleanup_invocation(args).is_none()
}

/// Entry point for `main`, before the headless modes: `Some(EXIT_BAD_INVOCATION)` for a malformed headless-mode
/// command line ([`malformed_mode_invocation`]), `None` otherwise.
pub fn mode_guard_main(args: impl IntoIterator<Item = OsString>) -> Option<i32> {
    let args: Vec<OsString> = args.into_iter().collect();
    malformed_mode_invocation(&args).then_some(EXIT_BAD_INVOCATION)
}

/// Reads `path` as text through a bounded reader ([`cc_config::read_text_bounded`]), so neither a large nor a
/// growing file makes it read more than [`MAX_FILE_BYTES`] + 1 bytes. A file larger than [`MAX_FILE_BYTES`] fails with
/// [`ErrorKind::FileTooLarge`](std::io::ErrorKind::FileTooLarge) (the size is checked before the bytes are decoded,
/// so also when the limit falls inside a multi-byte character); text that is not UTF-8 with `InvalidData`.
pub fn read_bounded(path: &Path) -> std::io::Result<String> {
    cc_config::read_text_bounded(path, MAX_FILE_BYTES)
}

/// The profiles in `dir`, in folder order, each file read once through [`read_bounded`] and parsed as
/// [`ProfileStore`](cc_config::ProfileStore) would (a `.toml` file; one that cannot be read, does not parse or has an
/// empty name is skipped, logged). A missing folder gives none. A file larger than [`MAX_FILE_BYTES`] refuses the whole read: the signed-in
/// app might see a profile there that this instance cannot, and so pick another one.
pub fn read_profiles(dir: &Path) -> Result<Vec<Profile>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot list profiles: {e}")),
    };
    let mut profiles = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| format!("cannot list profiles: {e}"))?.path();
        let is_toml = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("toml"));
        if !is_toml || !path.is_file() {
            continue;
        }
        let text = match read_bounded(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::FileTooLarge => {
                return Err(format!(
                    "profile file {} is larger than {MAX_FILE_BYTES} bytes",
                    path.display()
                ));
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping unreadable profile file");
                continue;
            }
        };
        match toml::from_str::<Profile>(&text) {
            Ok(profile) if !profile.name.trim().is_empty() => profiles.push(profile),
            Ok(_) => tracing::warn!(path = %path.display(), "skipping profile file with an empty name"),
            Err(e) => tracing::warn!(path = %path.display(), error = %e, "skipping unparsable profile file"),
        }
    }
    Ok(profiles)
}

/// The machine state for the start-up instance, loaded as usual. Every read of its files (also the old roaming one
/// it migrates from) is bounded by [`cc_config::MAX_STATE_FILE_BYTES`] inside `cc-config`: a larger file is not read,
/// kept as it is and not written; only an oversized `device-memory.toml` makes device memory read-only (logged there).
pub fn load_machine_state(paths: &Paths) -> HardwareState {
    HardwareState::load(paths)
}

/// Where the folders came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSource {
    /// The user's known folders (`%APPDATA%`, `%LOCALAPPDATA%`).
    KnownFolders,
    /// The profile folder from `HKLM\...\ProfileList\<SID>`.
    ProfileList,
}

/// The folders the start-up instance uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPaths {
    /// Config, profiles and logs.
    pub paths: Paths,
    /// Where they came from.
    pub source: PathSource,
}

/// CaseControl's folders inside the profile folder `profile`: `AppData\Roaming\CaseControl` (with `profiles`) and
/// `AppData\Local\CaseControl\logs`, the default locations of the known folders.
pub fn profile_paths(profile: &Path) -> Paths {
    let config_dir = profile.join("AppData").join("Roaming").join("CaseControl");
    Paths {
        profiles_dir: config_dir.join("profiles"),
        logs_dir: profile
            .join("AppData")
            .join("Local")
            .join("CaseControl")
            .join("logs"),
        config_dir,
    }
}

/// Picks the folders: the known folders (`known`) when their config file exists, else the ProfileList folders
/// (`profile`, see [`profile_paths`]) when theirs exists, else whichever of the two is available (known folders
/// first; there are no settings then and the defaults apply). `exists` checks a file.
pub fn resolve_paths(
    known: Option<Paths>,
    profile: Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> Option<ResolvedPaths> {
    let from_profile = profile.as_deref().map(profile_paths);
    let known = known.map(|paths| ResolvedPaths {
        paths,
        source: PathSource::KnownFolders,
    });
    let from_profile = from_profile.map(|paths| ResolvedPaths {
        paths,
        source: PathSource::ProfileList,
    });
    let has_config = |r: &ResolvedPaths| exists(&r.paths.config_file());
    if let Some(k) = known.as_ref().filter(|k| has_config(k)) {
        return Some(k.clone());
    }
    if let Some(p) = from_profile.as_ref().filter(|p| has_config(p)) {
        return Some(p.clone());
    }
    known.or(from_profile)
}

/// Reads `config.toml` without writing anything and bounded ([`read_bounded`]): a missing file gives the defaults;
/// a file that cannot be read, is too large or does not parse gives the defaults and the problem (it is not moved
/// aside, unlike `Settings::load`).
pub fn read_settings(paths: &Paths) -> (Settings, Option<String>) {
    let path = paths.config_file();
    match read_bounded(&path) {
        Ok(text) => match Settings::from_toml(&text) {
            Ok(settings) => (settings, None),
            Err(err) => (
                Settings::default(),
                Some(format!("{} does not parse: {err}", path.display())),
            ),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(err) => (
            Settings::default(),
            Some(format!("cannot read {}: {err}", path.display())),
        ),
    }
}

/// The active profile as the signed-in start would pick it, read-only and bounded ([`read_profiles`]: every file is
/// read once, so the profile applied is the one that was size-checked); `None` when there is none. Refused when a
/// profile file is larger than [`MAX_FILE_BYTES`].
pub fn read_active_profile(paths: &Paths, settings: &Settings) -> Result<Option<Profile>, String> {
    let profiles = read_profiles(&paths.profiles_dir)?;
    // Sorted and looked up as `ProfileStore::list` and `ProfileStore::load` do.
    let mut names: Vec<String> = profiles.iter().map(|p| p.name.clone()).collect();
    names.sort_by_cached_key(|n| (n.to_lowercase(), n.clone()));
    match start_plan(&names, settings.last_profile.as_deref()) {
        StartPlan::Create(_) => Ok(None),
        StartPlan::Apply { name, .. } => {
            let key = name.trim().to_lowercase();
            profiles
                .into_iter()
                .find(|p| p.name.trim().to_lowercase() == key)
                .map(Some)
                .ok_or_else(|| format!("cannot load profile {name:?}: not found"))
        }
    }
}

/// Whether a device of this id can still arrive after the first detection (a HID device, through hot-plug).
fn can_arrive_later(id: &DeviceId) -> bool {
    id.as_str().starts_with("hid:")
}

/// The devices of `initial` worth waiting for once the first detection is done (`present` are the devices found):
/// every present one, and absent ones only if they can still arrive (HID) and their family is not switched off
/// (`disabled`, see [`crate::prefs::family_of`]). Returns them and the ones given up on.
pub fn devices_to_wait_for(
    initial: &BTreeMap<DeviceId, DeviceSetting>,
    present: &BTreeSet<DeviceId>,
    disabled: &HashSet<String>,
) -> (BTreeSet<DeviceId>, Vec<DeviceId>) {
    let mut wait = BTreeSet::new();
    let mut given_up = Vec::new();
    for id in initial.keys() {
        let family_off = crate::prefs::family_of(id).is_some_and(|f| disabled.contains(f));
        if present.contains(id) || (can_arrive_later(id) && !family_off) {
            wait.insert(id.clone());
        } else {
            given_up.push(id.clone());
        }
    }
    (wait, given_up)
}

/// The devices of `wanted` that are not yet present and driven (a faulted device is not waited for any more).
pub fn still_waiting(wanted: &BTreeSet<DeviceId>, devices: &[DeviceView]) -> Vec<DeviceId> {
    wanted
        .iter()
        .filter(|id| {
            !devices.iter().any(|d| {
                d.description.info.id == **id
                    && matches!(d.status, DeviceStatus::Active | DeviceStatus::Faulted { .. })
            })
        })
        .cloned()
        .collect()
}

/// Entry point for `main`: `None` unless the command line is exactly `<exe> --apply-at-boot`; otherwise runs the
/// start-up mode and returns its exit code.
pub fn boot_main(args: impl IntoIterator<Item = OsString>) -> Option<i32> {
    let args: Vec<OsString> = args.into_iter().collect();
    if !cc_system::is_apply_at_boot_invocation(&args) {
        return None;
    }
    Some(run_boot())
}

fn run_boot() -> i32 {
    let mut pending = vec![format!(
        "CaseControl {} start-up task (--apply-at-boot)",
        env!("CARGO_PKG_VERSION")
    )];
    let sid = cc_system::current_user_sid();
    let profile = match &sid {
        Ok(sid) => cc_system::profile_path(sid)
            .map_err(|e| pending.push(format!("no ProfileList folder for {sid}: {e}")))
            .ok(),
        Err(e) => {
            pending.push(format!("cannot determine the user's SID: {e}"));
            None
        }
    };
    let Some(resolved) = resolve_paths(Paths::from_env(), profile, Path::exists) else {
        // Nowhere to log or read from.
        return EXIT_FAILED;
    };
    pending.push(format!(
        "folders from {:?}: config {}, logs {}",
        resolved.source,
        resolved.paths.config_dir.display(),
        resolved.paths.logs_dir.display()
    ));
    // Without a log file the colours are still applied; there is just no log.
    let _ = crate::logging::init_boot(&resolved.paths.logs_dir, &pending);
    log_token();
    let Ok(sid) = sid else {
        tracing::error!("no user SID; the device lock cannot be taken, nothing applied");
        return EXIT_FAILED;
    };
    // Opened before the lock, so a request of the signed-in instance is seen from the start.
    let signal = match YieldSignal::open(&sid) {
        Ok(SignalStatus::Opened(signal)) => Some(signal),
        Ok(SignalStatus::HeldBySomeoneElse) => {
            tracing::warn!("the yield signal belongs to another account; it is not watched");
            None
        }
        Err(err) => {
            tracing::warn!(%err, "cannot open the yield signal; it is not watched");
            None
        }
    };
    let lock = match DeviceLock::try_acquire(&sid) {
        Ok(LockStatus::Acquired(lock)) => lock,
        Ok(LockStatus::Busy) => {
            tracing::info!("the signed-in CaseControl holds the devices; nothing to do");
            return EXIT_OK;
        }
        Ok(LockStatus::HeldBySomeoneElse) => {
            tracing::warn!("the device lock belongs to another account; nothing applied");
            return EXIT_OK;
        }
        Err(err) => {
            tracing::error!(%err, "cannot take the device lock; nothing applied");
            return EXIT_FAILED;
        }
    };
    if signal.as_ref().is_some_and(YieldSignal::yield_requested) {
        tracing::info!("the signed-in CaseControl asked to finish; nothing applied");
        drop(lock);
        return EXIT_OK;
    }
    let code = apply(&resolved.paths, signal.as_ref());
    drop(lock);
    tracing::info!(code, "start-up task done");
    code
}

/// Logs the session, the token's elevation type and its integrity level.
fn log_token() {
    let session = cc_system::current_session_id().map_err(|e| e.to_string());
    match cc_system::token_elevation() {
        Ok(t) => tracing::info!(
            ?session,
            elevation = t.elevation_name(),
            elevation_type = t.elevation_type,
            integrity = t.integrity_name(),
            integrity_sid = %t.integrity_sid,
            "token"
        ),
        Err(err) => tracing::warn!(?session, %err, "cannot read the token's elevation"),
    }
}

/// Waits `timeout` for the signed-in instance's request to finish; whether it asked. Without a signal it just waits.
fn yield_asked(signal: Option<&YieldSignal>, timeout: Duration) -> bool {
    match signal {
        Some(signal) => signal.wait(timeout),
        None => {
            std::thread::sleep(timeout);
            false
        }
    }
}

/// Steps 3 and 4 of the module docs, with the lock held. `signal` is the yield signal, if it could be opened.
fn apply(paths: &Paths, signal: Option<&YieldSignal>) -> i32 {
    let (settings, problem) = read_settings(paths);
    if let Some(problem) = problem {
        tracing::warn!(%problem, "settings not readable; using the defaults (the file is left as it is)");
    }
    let profile = match read_active_profile(paths, &settings) {
        Ok(Some(profile)) => profile,
        Ok(None) => {
            tracing::info!("no profile yet; nothing to apply");
            return EXIT_OK;
        }
        Err(err) => {
            tracing::warn!(%err, "no profile applied");
            return EXIT_OK;
        }
    };
    let config = boot_engine_config(&settings, load_machine_state(paths), &profile);
    let initial = config.initial_settings.clone();
    let disabled = config.disabled_families.clone();
    if initial.is_empty() {
        tracing::info!(profile = %profile.name, "the active profile has no visible device settings; nothing to apply");
        return EXIT_OK;
    }
    tracing::info!(profile = %profile.name, devices = initial.len(), "applying the active profile");
    let started = Instant::now();
    let engine = match cc_engine::start(config, cc_drivers::all_backends()) {
        Ok(engine) => engine,
        Err(err) => {
            tracing::error!(%err, "the device engine did not start");
            return EXIT_FAILED;
        }
    };
    for failure in engine.failures() {
        tracing::warn!(family = %failure.family, location = %failure.location, error = %failure.error, "device skipped: it cannot be opened here");
    }
    let present: BTreeSet<DeviceId> = engine
        .devices()
        .into_iter()
        .map(|d| d.description.info.id)
        .collect();
    let (wanted, given_up) = devices_to_wait_for(&initial, &present, &disabled);
    for id in &given_up {
        tracing::warn!(%id, "device not found, and it cannot arrive later or its family is switched off; skipped");
    }
    let deadline = started + BOOT_WAIT;
    let mut waiting = still_waiting(&wanted, &engine.devices());
    let mut yielded = false;
    while !waiting.is_empty() && Instant::now() < deadline {
        if yield_asked(signal, POLL) {
            yielded = true;
            break;
        }
        waiting = still_waiting(&wanted, &engine.devices());
    }
    if yielded {
        tracing::info!(
            waiting = waiting.len(),
            "the signed-in CaseControl asked to finish; stopping now"
        );
    } else {
        for id in &waiting {
            tracing::warn!(%id, "device not present or not driven within the wait; skipped");
        }
    }
    for device in engine.devices() {
        tracing::info!(id = %device.description.info.id, name = %device.description.info.name, status = ?device.status, "device");
    }
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        "keeping the colours and stopping the engine"
    );
    engine.shutdown_with(ShutdownMode::Keep, STOP_TIMEOUT);
    EXIT_OK
}

/// The start-up instance's engine configuration: that of the signed-in app for `settings` and `profile`, with
/// device-memory writes off (no save, no restore, pending restores wait for the signed-in app).
pub fn boot_engine_config(
    settings: &Settings,
    hardware_state: HardwareState,
    profile: &Profile,
) -> cc_engine::EngineConfig {
    cc_engine::EngineConfig {
        device_memory_writes: false,
        ..crate::prefs::engine_config_for(settings, hardware_state, Some(profile))
    }
}

#[cfg(test)]
mod tests {
    use cc_config::ProfileStore;
    use cc_core::Rgb;

    use super::*;

    fn known() -> Paths {
        Paths {
            config_dir: PathBuf::from(r"C:\Users\me\AppData\Roaming\CaseControl"),
            profiles_dir: PathBuf::from(r"C:\Users\me\AppData\Roaming\CaseControl\profiles"),
            logs_dir: PathBuf::from(r"C:\Users\me\AppData\Local\CaseControl\logs"),
        }
    }

    fn system_profile() -> Paths {
        Paths {
            config_dir: PathBuf::from(
                r"C:\Windows\system32\config\systemprofile\AppData\Roaming\CaseControl",
            ),
            profiles_dir: PathBuf::from(
                r"C:\Windows\system32\config\systemprofile\AppData\Roaming\CaseControl\profiles",
            ),
            logs_dir: PathBuf::from(
                r"C:\Windows\system32\config\systemprofile\AppData\Local\CaseControl\logs",
            ),
        }
    }

    #[test]
    fn only_the_exact_flag_enters_the_boot_mode() {
        assert_eq!(boot_main(["cc.exe", "--hidden"].map(OsString::from)), None);
        assert_eq!(boot_main(["cc.exe"].map(OsString::from)), None);
        assert_eq!(
            boot_main(["cc.exe", "--apply-at-boot", "--hidden"].map(OsString::from)),
            None
        );
        assert_eq!(boot_main(["--apply-at-boot"].map(OsString::from)), None);
    }

    #[test]
    fn profile_paths_follow_the_default_layout() {
        let p = profile_paths(Path::new(r"C:\Users\me"));
        assert_eq!(
            p.config_dir,
            Path::new(r"C:\Users\me").join(r"AppData\Roaming\CaseControl")
        );
        assert_eq!(p.profiles_dir, p.config_dir.join("profiles"));
        assert_eq!(
            p.logs_dir,
            Path::new(r"C:\Users\me").join(r"AppData\Local\CaseControl\logs")
        );
        assert_eq!(p.config_file(), p.config_dir.join("config.toml"));
    }

    #[test]
    fn known_folders_win_when_they_hold_the_settings() {
        let r =
            resolve_paths(Some(known()), Some(PathBuf::from(r"D:\Profiles\me")), |_| true).expect("paths");
        assert_eq!(r.source, PathSource::KnownFolders);
        assert_eq!(r.paths, known());
    }

    #[test]
    fn profile_list_is_used_when_the_known_folders_have_no_settings() {
        // Under S4U without a loaded profile the known folders may resolve elsewhere (the system profile).
        let profile = PathBuf::from(r"C:\Users\me");
        let expected = profile_paths(&profile);
        let config = expected.config_file();
        let r = resolve_paths(Some(system_profile()), Some(profile), |p| p == config).expect("paths");
        assert_eq!(r.source, PathSource::ProfileList);
        assert_eq!(r.paths, expected);
        // No known folders at all.
        let r = resolve_paths(None, Some(PathBuf::from(r"C:\Users\me")), |_| false).expect("paths");
        assert_eq!(r.source, PathSource::ProfileList);
    }

    #[test]
    fn without_settings_anywhere_the_known_folders_are_kept() {
        let r = resolve_paths(Some(known()), Some(PathBuf::from(r"C:\Users\me")), |_| false).expect("paths");
        assert_eq!(r.source, PathSource::KnownFolders);
        assert_eq!(resolve_paths(None, None, |_| true), None);
    }

    #[test]
    fn settings_are_read_without_writing() {
        let root = std::env::temp_dir().join(format!("cc-boot-settings-{}", std::process::id()));
        let paths = Paths::at(&root);
        // Missing folder and file: defaults, nothing created.
        let (s, problem) = read_settings(&paths);
        assert_eq!(s, Settings::default());
        assert!(problem.is_none());
        assert!(!root.exists(), "nothing is created");
        std::fs::create_dir_all(&root).expect("dir");
        std::fs::write(
            paths.config_file(),
            "frame_rate = 45\nlast_profile = \"Evening\"\n",
        )
        .expect("write");
        let (s, problem) = read_settings(&paths);
        assert_eq!(s.frame_rate, 45);
        assert_eq!(s.last_profile.as_deref(), Some("Evening"));
        assert!(problem.is_none());
        // A broken file stays where it is.
        std::fs::write(paths.config_file(), "frame_rate = [").expect("write");
        let (s, problem) = read_settings(&paths);
        assert_eq!(s, Settings::default());
        assert!(problem.is_some());
        assert!(paths.config_file().exists(), "not moved aside");
        // No profiles: nothing to apply, and no profile is created.
        assert_eq!(read_active_profile(&paths, &s), Ok(None));
        assert!(!paths.profiles_dir.exists());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn active_profile_is_picked_like_the_signed_in_start() {
        let root = std::env::temp_dir().join(format!("cc-boot-profile-{}", std::process::id()));
        let paths = Paths::at(&root);
        let store = ProfileStore::new(&paths);
        for name in ["Day", "Evening"] {
            store
                .save(&Profile {
                    name: name.into(),
                    devices: BTreeMap::new(),
                    brightness: BTreeMap::new(),
                })
                .expect("save");
        }
        let mut settings = Settings {
            last_profile: Some("evening".into()),
            ..Settings::default()
        };
        let p = read_active_profile(&paths, &settings)
            .expect("reads")
            .expect("a profile");
        assert_eq!(p.name, "Evening");
        settings.last_profile = Some("Gone".into());
        let p = read_active_profile(&paths, &settings)
            .expect("reads")
            .expect("a profile");
        assert_eq!(p.name, "Day", "the first one, as the signed-in start would pick");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn absent_devices_are_waited_for_only_when_they_can_still_arrive() {
        let board = DeviceId::hid(0x1462, 0x7D75, None);
        let keyboard = DeviceId::hid(0x31E3, 0x1310, None);
        let gpu = DeviceId::nvapi(0x2702_10DE, 0x1612_10DE);
        let setting = DeviceSetting::Direct {
            colors: vec![Rgb::WHITE],
        };
        let initial = BTreeMap::from([
            (board.clone(), setting.clone()),
            (keyboard.clone(), setting.clone()),
            (gpu.clone(), setting),
        ]);
        let present = BTreeSet::from([board.clone()]);
        let none = HashSet::new();
        let (wait, given_up) = devices_to_wait_for(&initial, &present, &none);
        assert_eq!(wait, BTreeSet::from([board.clone(), keyboard.clone()]));
        assert_eq!(given_up, vec![gpu.clone()]);
        // An absent device of a switched-off family is not waited for.
        let off = HashSet::from([cc_core::family::WOOTING.to_owned()]);
        let (wait, given_up) = devices_to_wait_for(&initial, &present, &off);
        assert_eq!(wait, BTreeSet::from([board.clone()]));
        assert!(given_up.contains(&keyboard) && given_up.contains(&gpu));
        // A GPU that was found is waited for like any device.
        let (wait, given_up) = devices_to_wait_for(&initial, &BTreeSet::from([gpu.clone()]), &none);
        assert!(wait.contains(&gpu) && given_up.is_empty());
        // Nothing found yet and nothing wanted: done at once.
        assert!(still_waiting(&BTreeSet::new(), &[]).is_empty());
        assert_eq!(still_waiting(&BTreeSet::from([board.clone()]), &[]), vec![board]);
    }

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn malformed_headless_command_lines_fail_closed() {
        for bad in [
            &["cc.exe", "--apply-at-boot", "--hidden"][..],
            &["cc.exe", "--hidden", "--apply-at-boot"],
            &["cc.exe", "--APPLY-AT-BOOT"],
            &["cc.exe", "--apply-at-boot=1"],
            &["cc.exe", "--uninstall-cleanup", "x"],
            &["cc.exe", "--uninstall-cleanup", "--no-prompt", "x"],
            &["cc.exe", "--quit", "--uninstall-cleanup"],
            &["cc.exe", "--Uninstall-Cleanup"],
        ] {
            assert!(malformed_mode_invocation(&args(bad)), "{bad:?}");
            assert_eq!(mode_guard_main(args(bad)), Some(EXIT_BAD_INVOCATION), "{bad:?}");
        }
        for good in [
            &["cc.exe"][..],
            &["cc.exe", "--hidden"],
            &["cc.exe", "--quit"],
            &["cc.exe", "--quit-for-uninstall"],
            &["cc.exe", "--apply-at-boot"],
            &["cc.exe", "--uninstall-cleanup"],
            &["cc.exe", "--uninstall-cleanup", "--no-prompt"],
            // The program name is never looked at.
            &[r"C:\--apply-at-boot\cc.exe"],
        ] {
            assert!(!malformed_mode_invocation(&args(good)), "{good:?}");
        }
    }

    #[test]
    fn the_start_up_engine_never_writes_device_memory() {
        let profile = Profile {
            name: "Day".into(),
            devices: BTreeMap::new(),
            brightness: BTreeMap::new(),
        };
        let cfg = boot_engine_config(&Settings::default(), HardwareState::in_memory(), &profile);
        assert!(!cfg.device_memory_writes);
        assert!(cfg.hotplug);
    }

    #[test]
    fn reads_are_bounded() {
        let root = std::env::temp_dir().join(format!("cc-boot-bounded-{}", std::process::id()));
        let paths = Paths::at(&root);
        std::fs::create_dir_all(&paths.profiles_dir).expect("dirs");
        let big = vec![b'#'; (MAX_FILE_BYTES + 1) as usize];
        std::fs::write(paths.config_file(), &big).expect("write");
        let (s, problem) = read_settings(&paths);
        assert_eq!(s, Settings::default());
        assert!(problem.is_some_and(|p| p.contains("larger than")));
        std::fs::write(paths.config_file(), vec![b'#'; MAX_FILE_BYTES as usize]).expect("write");
        assert!(
            read_bounded(&paths.config_file()).is_ok(),
            "exactly the limit is fine"
        );
        assert_eq!(
            read_bounded(&paths.config_file().with_file_name("missing.toml"))
                .expect_err("missing")
                .kind(),
            std::io::ErrorKind::NotFound
        );
        // A large profile file refuses the profile read; it is found by the bounded read itself.
        std::fs::write(paths.profiles_dir.join("huge.toml"), &big).expect("write");
        let err = read_profiles(&paths.profiles_dir).expect_err("too large");
        assert!(err.contains("huge.toml") && err.contains("larger than"), "{err}");
        let err = read_active_profile(&paths, &Settings::default()).expect_err("too large");
        assert!(err.contains("larger than"), "{err}");
        assert_eq!(
            read_bounded(&paths.profiles_dir.join("huge.toml"))
                .expect_err("too large")
                .kind(),
            std::io::ErrorKind::FileTooLarge
        );
        assert_eq!(read_profiles(&root.join("missing")), Ok(Vec::new()));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn an_oversized_profile_is_too_large_even_when_the_limit_splits_a_character() {
        let root = std::env::temp_dir().join(format!("cc-boot-utf8-{}", std::process::id()));
        let paths = Paths::at(&root);
        std::fs::create_dir_all(&paths.profiles_dir).expect("dirs");
        // A valid profile padded with two-byte characters so that byte MAX_FILE_BYTES + 1 is the first half of one:
        // `read_to_string` over MAX_FILE_BYTES + 1 bytes would fail with InvalidData and skip the file.
        let mut text = String::from("name = \"Big\"\n#");
        assert_eq!(
            text.len() % 2,
            0,
            "even, so the padding ends exactly at MAX_FILE_BYTES"
        );
        while (text.len() as u64) <= MAX_FILE_BYTES {
            text.push(char::from(0xE9_u8));
        }
        assert_eq!(text.len() as u64, MAX_FILE_BYTES + 2);
        std::fs::write(paths.profiles_dir.join("big.toml"), &text).expect("write");
        assert_eq!(
            read_bounded(&paths.profiles_dir.join("big.toml"))
                .expect_err("too large")
                .kind(),
            std::io::ErrorKind::FileTooLarge
        );
        let err = read_profiles(&paths.profiles_dir).expect_err("refused, not skipped");
        assert!(err.contains("big.toml") && err.contains("larger than"), "{err}");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn an_oversized_machine_state_file_is_kept_and_device_memory_is_read_only() {
        let root = std::env::temp_dir().join(format!("cc-boot-state-{}", std::process::id()));
        let paths = Paths::at(&root);
        std::fs::create_dir_all(&root).expect("dirs");
        let big = vec![b'#'; (cc_config::MAX_STATE_FILE_BYTES + 1) as usize];
        std::fs::write(paths.device_memory_file(), &big).expect("write");
        let state = load_machine_state(&paths);
        assert!(!state.device_memory_writable());
        assert_eq!(std::fs::read(paths.device_memory_file()).expect("read"), big);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn bounded_profile_read_matches_the_profile_store() {
        let root = std::env::temp_dir().join(format!("cc-boot-profiles-{}", std::process::id()));
        let paths = Paths::at(&root);
        let store = ProfileStore::new(&paths);
        let mut day = Profile {
            name: "Day".into(),
            devices: BTreeMap::new(),
            brightness: BTreeMap::new(),
        };
        day.devices.insert(
            DeviceId::hid(0x1462, 0x7D75, None),
            DeviceSetting::Direct {
                colors: vec![Rgb::WHITE],
            },
        );
        store.save(&day).expect("save");
        store
            .save(&Profile {
                name: "evening".into(),
                ..day.clone()
            })
            .expect("save");
        // Files the store skips are skipped here too: broken TOML, not UTF-8, an empty name, another extension.
        std::fs::write(paths.profiles_dir.join("broken.toml"), "name = [").expect("write");
        std::fs::write(paths.profiles_dir.join("binary.toml"), [0xFF, 0xFE, 0x00]).expect("write");
        std::fs::write(paths.profiles_dir.join("blank.toml"), "name = \"  \"\n").expect("write");
        std::fs::write(paths.profiles_dir.join("notes.txt"), "name = \"Notes\"\n").expect("write");
        let mut names: Vec<String> = read_profiles(&paths.profiles_dir)
            .expect("reads")
            .into_iter()
            .map(|p| p.name)
            .collect();
        names.sort_by_cached_key(|n| (n.to_lowercase(), n.clone()));
        assert_eq!(names, store.list().expect("list"));
        let settings = Settings {
            last_profile: Some("DAY".into()),
            ..Settings::default()
        };
        let p = read_active_profile(&paths, &settings)
            .expect("reads")
            .expect("a profile");
        assert_eq!(
            p,
            store.load("Day").expect("load"),
            "the same profile as the store loads"
        );
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
