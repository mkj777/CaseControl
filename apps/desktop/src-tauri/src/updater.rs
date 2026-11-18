//! Silent auto-update from GitHub releases (ADR 0001 #39), the app's only network access.
//!
//! Behaviour:
//! - Release builds check the updater endpoint (`plugins.updater` in `tauri.conf.json`) once after the start, on a
//!   background thread after the engine is up, and then every [`CHECK_INTERVAL`].
//! - A newer release is downloaded in the background, its signature verified by the Tauri updater plugin, and stored
//!   as the *pending update* under `%LOCALAPPDATA%\CaseControl\updates` ([`PendingStore`]).
//! - The pending update is installed on the next quit (tray Quit, the `quit` command, `--quit`, the run-loop exit that
//!   also covers logoff/shutdown) after the devices were released, and on the next start before the engine opens any
//!   device. On quit the app stays closed; on start the installer starts the new version with the same arguments.
//! - Settings shows the status (ADR 0001 #41): [`Updater::view`] and the `update-status` event carry an
//!   [`UpdateView`]. "Check for updates" runs [`Updater::check_once`] now (the same check, download and
//!   [`PendingStore`] as the background check; a check that is already running is not started twice). "Update now"
//!   ([`Updater::install_now`]) releases the devices through the normal shutdown path, installs the pending update
//!   and restarts the app. No dialog: every step is logged; a failure (offline, GitHub down, bad signature) is shown
//!   as the status and retried at the next interval. The NSIS installer runs in quiet mode, per user.
//! - Debug builds never check, download or install ([`enabled_for_build`]).
//!
//! The decisions live in [`Updater`], which talks to the plugin only through [`UpdateBackend`] so the state machine is
//! unit-tested without network or installer; [`TauriBackend`] is the real implementation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::UpdaterExt;

/// Time between two update checks while the app runs (the first one runs right after the start).
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Timeout for the request to the update endpoint.
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);

/// Timeout for downloading the installer.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Whether this build checks for and installs updates: release builds only.
pub const fn enabled_for_build() -> bool {
    !cfg!(debug_assertions)
}

/// Directory of the pending update: `%LOCALAPPDATA%\CaseControl\updates`. `None` without a home directory.
pub fn updates_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.data_local_dir().join("CaseControl").join("updates"))
}

/// A release the update endpoint announced as newer than the running version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Announced {
    /// The announced version.
    pub version: String,
    /// The endpoint's raw `latest.json`; enough to rebuild and verify the update later without the network.
    pub manifest: Value,
}

/// The plugin operations the updater needs. Implementations block; they are called from the update thread (check,
/// download) and from the start and quit paths (verify, install).
pub trait UpdateBackend: Send + Sync {
    /// Asks the endpoint for the latest release. `Ok(None)` when it is not newer than the running version.
    fn check(&self) -> Result<Option<Announced>, String>;
    /// Downloads the installer of `announced` and verifies its signature.
    fn download(&self, announced: &Announced) -> Result<Vec<u8>, String>;
    /// Verifies a stored installer against its stored manifest. `Ok(Some(version))` when it is valid and still newer
    /// than the running version, `Ok(None)` when it is no longer an update (e.g. already installed).
    fn verify(&self, manifest: &Value, bytes: &[u8]) -> Result<Option<String>, String>;
    /// Starts the installer (quiet, per user). On Windows a successful start ends this process and never returns.
    /// `restart`: whether and with which arguments the installer starts the new version afterwards.
    fn install(&self, manifest: &Value, bytes: &[u8], restart: &Restart) -> Result<(), String>;
}

/// Whether and how the installer starts CaseControl after installing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restart {
    /// It does not (install on quit).
    No,
    /// With the arguments this process was started with (install on start, e.g. keeping `--hidden`).
    SameArgs,
    /// With these arguments (Update now: the current ones without `--hidden` and `--quit`).
    WithArgs(Vec<String>),
}

/// Shown for a build without updates, as the `disabled` status's reason and the refusal of the update commands.
pub const DISABLED_REASON: &str = "Updates are only available in installed builds";

/// A downloaded, verified update waiting for the next quit or start.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// The release it belongs to.
    pub announced: Announced,
    /// The installer bytes.
    pub bytes: Vec<u8>,
}

/// Stores the pending update as two files: `pending.bin` (installer) and `pending.json` ([`Announced`]). The JSON file
/// is written last and removed first, so it marks a complete pending update; both are written to a temporary file
/// and renamed, so a quit during a download never sees a half-written installer. A third file, `pending.attempted`,
/// marks that its installer was started once: the update stays in place until the installer really launched (the
/// process ends then), and a pending update whose installer already ran without installing it is discarded instead
/// of started again, so a failing installer cannot cause a start/install loop.
#[derive(Debug, Clone)]
pub struct PendingStore {
    dir: PathBuf,
}

const PENDING_JSON: &str = "pending.json";
const PENDING_BIN: &str = "pending.bin";
const PENDING_ATTEMPTED: &str = "pending.attempted";

impl PendingStore {
    /// A store in `dir` (created on the first save).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The store directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The announced release of the pending update, without reading the installer. `None` when nothing is pending or
    /// the marker is unreadable.
    pub fn pending_version(&self) -> Option<String> {
        let text = fs::read_to_string(self.dir.join(PENDING_JSON)).ok()?;
        serde_json::from_str::<Announced>(&text).ok().map(|a| a.version)
    }

    /// Loads the pending update. `Ok(None)` when nothing is pending; an error when the files are damaged.
    pub fn load(&self) -> Result<Option<Pending>, String> {
        let text = match fs::read_to_string(self.dir.join(PENDING_JSON)) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(format!("cannot read {PENDING_JSON}: {err}")),
        };
        let announced: Announced =
            serde_json::from_str(&text).map_err(|err| format!("invalid {PENDING_JSON}: {err}"))?;
        let bytes = fs::read(self.dir.join(PENDING_BIN))
            .map_err(|err| format!("cannot read {PENDING_BIN}: {err}"))?;
        Ok(Some(Pending { announced, bytes }))
    }

    /// Replaces the pending update.
    pub fn save(&self, announced: &Announced, bytes: &[u8]) -> Result<(), String> {
        fs::create_dir_all(&self.dir)
            .map_err(|err| format!("cannot create {}: {err}", self.dir.display()))?;
        // Remove the marker first so a crash between the two writes leaves no marker pointing at the wrong installer.
        remove_if_exists(&self.dir.join(PENDING_JSON))?;
        remove_if_exists(&self.dir.join(PENDING_ATTEMPTED))?;
        write_atomic(&self.dir.join(PENDING_BIN), bytes)?;
        let json = serde_json::to_vec_pretty(announced)
            .map_err(|err| format!("cannot serialize the pending update: {err}"))?;
        write_atomic(&self.dir.join(PENDING_JSON), &json)
    }

    /// Removes the pending update (marker first). Missing files are fine.
    pub fn clear(&self) -> Result<(), String> {
        remove_if_exists(&self.dir.join(PENDING_JSON))?;
        remove_if_exists(&self.dir.join(PENDING_BIN))?;
        remove_if_exists(&self.dir.join(PENDING_ATTEMPTED))
    }

    /// Whether the installer of the pending update was started before.
    pub fn attempted(&self) -> bool {
        self.dir.join(PENDING_ATTEMPTED).exists()
    }

    /// Records that the installer of the pending update is being started.
    pub fn mark_attempted(&self) -> Result<(), String> {
        let path = self.dir.join(PENDING_ATTEMPTED);
        fs::write(&path, b"").map_err(|err| format!("cannot write {}: {err}", path.display()))
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|err| format!("cannot write {}: {err}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|err| format!("cannot move {} into place: {err}", tmp.display()))
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("cannot remove {}: {err}", path.display())),
    }
}

/// Result of one update check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Updates are off in this build.
    Disabled,
    /// The endpoint announces nothing newer.
    UpToDate,
    /// The announced version is already downloaded and pending.
    AlreadyPending(String),
    /// A newer version was downloaded, verified and is now pending.
    Downloaded(String),
    /// The check, the download or storing failed; retried at the next interval.
    Failed(String),
    /// Another check (background or button) or an install is running; this one was not started.
    AlreadyRunning,
}

/// Result of trying to install the pending update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Updates are off in this build.
    Disabled,
    /// Nothing is pending.
    NothingPending,
    /// The pending update is not newer than the running version any more; it was removed.
    Stale,
    /// The pending update could not be read or failed verification; it was removed and is downloaded again later.
    Discarded(String),
    /// The installer was started for this version. On Windows the process has already ended at this point, so this
    /// is only seen with test backends.
    Installing(String),
    /// The installer could not be started (or was started before without installing it); the pending update was
    /// removed and is downloaded again later.
    Failed(String),
    /// A check is running ([`Updater::install_now`] only); nothing was installed.
    Busy,
}

/// What the updater is doing or did last, for the Settings page (ADR 0001 #41). Serialized with a `state` tag:
/// `{ "state": "downloading", "version": "0.2.0" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateStatus {
    /// Updates are off in this build (debug builds).
    Disabled {
        /// Why, for the page to show ([`DISABLED_REASON`]).
        reason: String,
    },
    /// Nothing checked yet in this session.
    Idle,
    /// Asking the update endpoint.
    Checking,
    /// The endpoint announces nothing newer.
    UpToDate,
    /// Downloading and verifying a newer release.
    Downloading {
        /// The version being downloaded.
        version: String,
    },
    /// A downloaded, verified update is pending and nothing runs. Never stored: [`Updater::view`] derives it from the
    /// pending store, so it cannot outlive the update.
    Ready {
        /// The pending version.
        version: String,
    },
    /// The installer of this version is being started; the app ends right after.
    Installing {
        /// The version being installed.
        version: String,
    },
    /// The last check, download or install failed.
    Failed {
        /// Why, as the plugin or the store reported it.
        error: String,
    },
}

/// The update state the Settings page shows: [`Updater::view`] and the payload of the `update-status` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateView {
    /// What the updater is doing or did last.
    pub status: UpdateStatus,
    /// The version of the downloaded update waiting to be installed ("Update now" is possible), if any.
    pub pending: Option<String>,
    /// Grows with every status change; a page keeps the view with the highest one, so a command reply that was
    /// taken before a newer `update-status` event cannot roll the page back.
    pub seq: u64,
}

/// Called with the new view after every status change.
pub type StatusListener = Box<dyn Fn(&UpdateView) + Send + Sync>;

/// The update state machine: when to check, what to keep, when to install, and what to show.
pub struct Updater<B> {
    backend: B,
    store: PendingStore,
    enabled: bool,
    /// What runs or ran last (never [`UpdateStatus::Ready`], see [`Updater::view`]) and the change counter.
    status: Mutex<(UpdateStatus, u64)>,
    /// Set while a check or [`Updater::install_now`] runs, so neither starts twice.
    running: AtomicBool,
    listener: Option<StatusListener>,
}

impl<B: std::fmt::Debug> std::fmt::Debug for Updater<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updater")
            .field("backend", &self.backend)
            .field("store", &self.store)
            .field("enabled", &self.enabled)
            .field("status", &self.status.lock().0)
            .field("running", &self.running.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

/// Clears [`Updater::running`] when a check or an install ends, also on a panic.
struct RunningGuard<'a>(&'a AtomicBool);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl<B: UpdateBackend> Updater<B> {
    /// An updater over `backend` keeping its pending update in `store`. With `enabled == false` (debug builds, see
    /// [`enabled_for_build`]) every operation is a logged no-op.
    pub fn new(backend: B, store: PendingStore, enabled: bool) -> Self {
        Self {
            backend,
            store,
            enabled,
            status: Mutex::new((UpdateStatus::Idle, 0)),
            running: AtomicBool::new(false),
            listener: None,
        }
    }

    /// Calls `listener` with the new view after every status change (the app emits it as `update-status`).
    #[must_use]
    pub fn with_listener(mut self, listener: impl Fn(&UpdateView) + Send + Sync + 'static) -> Self {
        self.listener = Some(Box::new(listener));
        self
    }

    /// Whether this updater checks and installs at all.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The current status and the pending version. While nothing runs (idle or up to date) and an update is pending,
    /// the status is [`UpdateStatus::Ready`]; it is derived from the store on every call, so it is never stale.
    pub fn view(&self) -> UpdateView {
        let (stored, seq) = self.status.lock().clone();
        if !self.enabled {
            // A disabled updater may have no store directory; it never reads one.
            return UpdateView {
                status: UpdateStatus::Disabled {
                    reason: DISABLED_REASON.to_owned(),
                },
                pending: None,
                seq,
            };
        }
        let pending = self.store.pending_version();
        let status = match (stored, &pending) {
            (UpdateStatus::Idle | UpdateStatus::UpToDate, Some(version)) => UpdateStatus::Ready {
                version: version.clone(),
            },
            (stored, _) => stored,
        };
        UpdateView { status, pending, seq }
    }

    fn set_status(&self, status: UpdateStatus) {
        {
            let mut guard = self.status.lock();
            guard.0 = status;
            guard.1 += 1;
        }
        if let Some(listener) = &self.listener {
            listener(&self.view());
        }
    }

    /// Claims the running flag; `None` while a check or an install already runs.
    fn claim(&self) -> Option<RunningGuard<'_>> {
        // `then`, not `then_some`: a guard built for nothing would clear the flag of the check that holds it.
        (!self.running.swap(true, Ordering::AcqRel)).then(|| RunningGuard(&self.running))
    }

    /// One check: ask the endpoint, download a newer release unless it is already pending, store it. Used by the
    /// background thread and by "Check for updates"; while a check (or [`Self::install_now`]) runs, another returns
    /// [`CheckOutcome::AlreadyRunning`] at once.
    pub fn check_once(&self) -> CheckOutcome {
        if !self.enabled {
            return CheckOutcome::Disabled;
        }
        let Some(_running) = self.claim() else {
            tracing::info!("update check: another check or an install is running; not starting a second one");
            return CheckOutcome::AlreadyRunning;
        };
        let outcome = self.check_claimed();
        match &outcome {
            // The view shows a pending update as ready.
            CheckOutcome::UpToDate | CheckOutcome::AlreadyPending(_) | CheckOutcome::Downloaded(_) => {
                self.set_status(UpdateStatus::UpToDate);
            }
            CheckOutcome::Failed(error) => self.set_status(UpdateStatus::Failed { error: error.clone() }),
            CheckOutcome::Disabled | CheckOutcome::AlreadyRunning => {}
        }
        outcome
    }

    fn check_claimed(&self) -> CheckOutcome {
        self.set_status(UpdateStatus::Checking);
        tracing::info!("update check: asking the update endpoint");
        let announced = match self.backend.check() {
            Ok(Some(announced)) => announced,
            Ok(None) => {
                tracing::info!("update check: up to date");
                return CheckOutcome::UpToDate;
            }
            Err(err) => {
                tracing::warn!(%err, "update check failed; retrying at the next interval");
                return CheckOutcome::Failed(err);
            }
        };
        if self.store.pending_version().as_deref() == Some(announced.version.as_str()) {
            tracing::info!(version = %announced.version, "update check: this version is already downloaded");
            return CheckOutcome::AlreadyPending(announced.version);
        }
        tracing::info!(version = %announced.version, "update available; downloading");
        self.set_status(UpdateStatus::Downloading {
            version: announced.version.clone(),
        });
        let bytes = match self.backend.download(&announced) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(version = %announced.version, %err, "update download failed; retrying at the next interval");
                return CheckOutcome::Failed(err);
            }
        };
        if let Err(err) = self.store.save(&announced, &bytes) {
            tracing::warn!(version = %announced.version, %err, "cannot store the downloaded update; retrying at the next interval");
            return CheckOutcome::Failed(err);
        }
        tracing::info!(
            version = %announced.version,
            size = bytes.len(),
            dir = %self.store.dir().display(),
            "update downloaded and verified; installs on the next quit or start, or with Update now"
        );
        CheckOutcome::Downloaded(announced.version)
    }

    /// Checks now and then every [`CHECK_INTERVAL`] until `sleep` returns `false`. `sleep` is called with the time to
    /// wait before the next check.
    pub fn run(&self, mut sleep: impl FnMut(Duration) -> bool) {
        if !self.enabled {
            tracing::info!("update checks are off in this build");
            return;
        }
        loop {
            self.check_once();
            if !sleep(CHECK_INTERVAL) {
                return;
            }
        }
    }

    /// Installs the pending update if there is a valid one. The pending files stay until the installer really
    /// launched (the process ends then); they are marked as attempted first, so an update whose installer ran without
    /// installing it is discarded the next time instead of started again (no start/install loop), and they are
    /// removed when the installer cannot be started. `before_install` runs right before the installer starts (and
    /// only then). `restart`: see [`UpdateBackend::install`].
    pub fn install_pending(&self, restart: &Restart, before_install: impl FnOnce()) -> InstallOutcome {
        if !self.enabled {
            return InstallOutcome::Disabled;
        }
        let pending = match self.store.load() {
            Ok(Some(pending)) => pending,
            Ok(None) => {
                tracing::debug!("no pending update");
                return InstallOutcome::NothingPending;
            }
            Err(err) => {
                tracing::warn!(%err, "pending update is damaged; removing it");
                self.clear_logged();
                return InstallOutcome::Discarded(err);
            }
        };
        let announced = &pending.announced;
        let version = match self.backend.verify(&announced.manifest, &pending.bytes) {
            Ok(Some(version)) => version,
            Ok(None) => {
                tracing::info!(version = %announced.version, "pending update is not newer than this version; removing it");
                self.clear_logged();
                return InstallOutcome::Stale;
            }
            Err(err) => {
                tracing::warn!(version = %announced.version, %err, "pending update failed verification; removing it");
                self.clear_logged();
                return InstallOutcome::Discarded(err);
            }
        };
        if self.store.attempted() {
            let err = format!("the installer of {version} ran before without installing it");
            tracing::warn!(%version, "pending update was started before and is still not installed; removing it");
            self.clear_logged();
            return InstallOutcome::Failed(err);
        }
        if let Err(err) = self.store.mark_attempted() {
            // Without the marker a failing installer could loop; fall back to removing the update first.
            tracing::warn!(%err, "cannot mark the pending update as attempted; removing it before installing");
            self.clear_logged();
        }
        self.set_status(UpdateStatus::Installing {
            version: version.clone(),
        });
        before_install();
        tracing::info!(%version, ?restart, "starting the update installer");
        match self.backend.install(&announced.manifest, &pending.bytes, restart) {
            Ok(()) => InstallOutcome::Installing(version),
            Err(err) => {
                tracing::error!(%version, %err, "cannot start the update installer; downloading it again later");
                self.clear_logged();
                self.set_status(UpdateStatus::Failed { error: err.clone() });
                InstallOutcome::Failed(err)
            }
        }
    }

    /// "Update now" (ADR 0001 #41): installs the pending update with `restart`, unless a check runs
    /// ([`InstallOutcome::Busy`]); no check starts while it runs. `before_install` runs only once the stored update
    /// verified, right before the installer starts; the app releases the devices there. A pending update that is
    /// stale or fails verification is removed, and the status says so.
    pub fn install_now(&self, restart: Restart, before_install: impl FnOnce()) -> InstallOutcome {
        if !self.enabled {
            return InstallOutcome::Disabled;
        }
        let Some(_running) = self.claim() else {
            tracing::info!("update now: a check is running; not installing");
            return InstallOutcome::Busy;
        };
        tracing::info!("update now: installing the pending update");
        let outcome = self.install_pending(&restart, before_install);
        match &outcome {
            InstallOutcome::Stale => self.set_status(UpdateStatus::UpToDate),
            InstallOutcome::Discarded(error) => {
                self.set_status(UpdateStatus::Failed { error: error.clone() })
            }
            _ => {}
        }
        outcome
    }

    fn clear_logged(&self) {
        if let Err(err) = self.store.clear() {
            tracing::warn!(%err, "cannot remove the pending update");
        }
    }
}

/// [`UpdateBackend`] over `tauri-plugin-updater` with the endpoint, public key and install mode from
/// `tauri.conf.json`.
#[derive(Debug)]
pub struct TauriBackend<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> TauriBackend<R> {
    /// A backend for `app` (the updater plugin must be registered).
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }

    fn restore(&self, manifest: &Value) -> Result<Option<tauri_plugin_updater::Update>, String> {
        let updater = self
            .app
            .updater()
            .map_err(|err| format!("updater unavailable: {err}"))?;
        updater
            .restore_update(manifest.clone())
            .map_err(|err| format!("invalid update manifest: {err}"))
    }
}

impl<R: Runtime> UpdateBackend for TauriBackend<R> {
    fn check(&self) -> Result<Option<Announced>, String> {
        let updater = self
            .app
            .updater_builder()
            .timeout(CHECK_TIMEOUT)
            .build()
            .map_err(|err| format!("updater unavailable: {err}"))?;
        let update = tauri::async_runtime::block_on(updater.check()).map_err(|err| err.to_string())?;
        Ok(update.map(|update| Announced {
            version: update.version,
            manifest: update.raw_json,
        }))
    }

    fn download(&self, announced: &Announced) -> Result<Vec<u8>, String> {
        let mut update = self
            .restore(&announced.manifest)?
            .ok_or_else(|| format!("{} is no longer an update", announced.version))?;
        update.timeout = Some(DOWNLOAD_TIMEOUT);
        // `download` verifies the signature (including the signed version) before returning the bytes.
        tauri::async_runtime::block_on(update.download(|_, _| {}, || {})).map_err(|err| err.to_string())
    }

    fn verify(&self, manifest: &Value, bytes: &[u8]) -> Result<Option<String>, String> {
        let Some(update) = self.restore(manifest)? else {
            return Ok(None);
        };
        update.verify(bytes).map_err(|err| err.to_string())?;
        Ok(Some(update.version))
    }

    fn install(&self, manifest: &Value, bytes: &[u8], restart: &Restart) -> Result<(), String> {
        let builder = self.app.updater_builder();
        // The plugin's own restart always passes this process's arguments; for other arguments it is off and the
        // NSIS restart flags are passed as installer arguments instead (see `nsis_restart_args`).
        let builder = match restart {
            Restart::WithArgs(args) => builder.installer_args(nsis_restart_args(args)),
            Restart::No | Restart::SameArgs => builder,
        };
        let update = builder
            .build()
            .map_err(|err| format!("updater unavailable: {err}"))?
            .restore_update(manifest.clone())
            .map_err(|err| format!("invalid update manifest: {err}"))?
            .ok_or_else(|| "the pending update is no longer an update".to_owned())?;
        update
            .restart_after_install(*restart == Restart::SameArgs)
            .install(bytes)
            .map_err(|err| err.to_string())
    }
}

/// The NSIS installer arguments that start the app after a quiet install with `args`, as the updater plugin builds
/// them for its own restart (`/R`, then `/ARGS` and each argument escaped like the plugin's
/// `escape_nsis_current_exe_arg`). Valid for the quiet and passive install modes and the NSIS bundle this app ships
/// (`tauri.conf.json`, tested in `updater_config_in_tauri_conf_parses`).
pub fn nsis_restart_args(args: &[String]) -> Vec<String> {
    let mut out = vec!["/R".to_owned()];
    if !args.is_empty() {
        out.push("/ARGS".to_owned());
        out.extend(args.iter().map(|a| escape_nsis_arg(a)));
    }
    out
}

/// Quotes an argument for the NSIS command line like the updater plugin does: quoted when it is empty or holds a
/// space, tab or `/` (so NSIS does not read it as an option), backslashes before a quote doubled and the quote
/// escaped.
fn escape_nsis_arg(arg: &str) -> String {
    let quote = arg.is_empty() || arg.contains([' ', '\t', '/']);
    let mut out = String::with_capacity(arg.len() + 2);
    if quote {
        out.push('"');
    }
    let mut backslashes = 0usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                // n backslashes before a quote become 2n + 1.
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    if quote {
        out.extend(std::iter::repeat_n('\\', backslashes));
        out.push('"');
    }
    out
}

/// The updater the app manages as Tauri state.
pub type AppUpdater<R> = Arc<Updater<TauriBackend<R>>>;

/// Creates the app's updater: enabled in release builds that can determine `%LOCALAPPDATA%`. Every status change is
/// passed to `listener` (the app emits it as `update-status`).
pub fn create<R: Runtime>(
    app: &AppHandle<R>,
    listener: impl Fn(&UpdateView) + Send + Sync + 'static,
) -> AppUpdater<R> {
    let dir = updates_dir();
    let enabled = enabled_for_build() && dir.is_some();
    if enabled_for_build() && dir.is_none() {
        tracing::warn!("cannot determine the local data directory; updates are off");
    }
    if !enabled_for_build() {
        tracing::info!("debug build: updates are off");
    }
    let store = PendingStore::new(dir.unwrap_or_default());
    Arc::new(Updater::new(TauriBackend::new(app.clone()), store, enabled).with_listener(listener))
}

/// Starts the background thread that checks now and every [`CHECK_INTERVAL`]. Does nothing when updates are off.
pub fn spawn_checks<R: Runtime>(updater: AppUpdater<R>) {
    if !updater.enabled() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("cc-updater".into())
        .spawn(move || {
            updater.run(|interval| {
                std::thread::sleep(interval);
                true
            });
        });
    if let Err(err) = spawned {
        tracing::error!(%err, "cannot start the update thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Check,
        Download(String),
        Verify(String),
        Install { version: String, restart: Restart },
    }

    /// Scripted backend. Manifests are `{"version": "..."}`; a package is valid when its bytes equal
    /// `valid_bytes` and its version is in `newer`.
    #[derive(Debug, Default)]
    struct Fake {
        calls: Mutex<Vec<Call>>,
        check: Mutex<Option<Result<Option<Announced>, String>>>,
        download: Mutex<Option<Result<Vec<u8>, String>>>,
        valid_bytes: Vec<u8>,
        newer: Vec<String>,
        install_error: Option<String>,
    }

    fn version_of(manifest: &Value) -> String {
        manifest["version"].as_str().unwrap_or_default().to_owned()
    }

    impl UpdateBackend for Fake {
        fn check(&self) -> Result<Option<Announced>, String> {
            self.calls.lock().push(Call::Check);
            self.check.lock().clone().unwrap_or(Ok(None))
        }
        fn download(&self, announced: &Announced) -> Result<Vec<u8>, String> {
            self.calls.lock().push(Call::Download(announced.version.clone()));
            self.download
                .lock()
                .clone()
                .unwrap_or_else(|| Err("offline".into()))
        }
        fn verify(&self, manifest: &Value, bytes: &[u8]) -> Result<Option<String>, String> {
            let version = version_of(manifest);
            self.calls.lock().push(Call::Verify(version.clone()));
            if !self.newer.contains(&version) {
                return Ok(None);
            }
            if bytes != self.valid_bytes.as_slice() {
                return Err("bad signature".into());
            }
            Ok(Some(version))
        }
        fn install(&self, manifest: &Value, _bytes: &[u8], restart: &Restart) -> Result<(), String> {
            self.calls.lock().push(Call::Install {
                version: version_of(manifest),
                restart: restart.clone(),
            });
            match &self.install_error {
                Some(err) => Err(err.clone()),
                None => Ok(()),
            }
        }
    }

    fn announced(version: &str) -> Announced {
        Announced {
            version: version.to_owned(),
            manifest: json!({ "version": version }),
        }
    }

    fn temp_store(name: &str) -> PendingStore {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "casecontrol-updater-test-{}-{}-{name}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        PendingStore::new(dir)
    }

    fn fake() -> Fake {
        Fake {
            valid_bytes: b"installer".to_vec(),
            newer: vec!["0.2.0".into(), "0.3.0".into()],
            ..Fake::default()
        }
    }

    fn updater(backend: Fake, store: &PendingStore) -> Updater<Fake> {
        Updater::new(backend, store.clone(), true)
    }

    fn calls(updater: &Updater<Fake>) -> Vec<Call> {
        updater.backend.calls.lock().clone()
    }

    /// The plugin reads `plugins.updater` when it initializes at start; a config it cannot parse would stop the app.
    #[test]
    fn updater_config_in_tauri_conf_parses() {
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        let config: tauri_plugin_updater::Config =
            serde_json::from_value(conf["plugins"]["updater"].clone()).expect("updater config");
        assert_eq!(
            config.endpoints.iter().map(|u| u.as_str()).collect::<Vec<_>>(),
            ["https://github.com/mkj777/CaseControl/releases/latest/download/latest.json"]
        );
        assert!(config.require_signed_version);
        assert_eq!(config.windows.expect("windows").install_mode.to_string(), "quiet");
        // The owner's minisign public key (key id 415776BB651163F2), base64 as `tauri signer generate` writes it.
        assert!(
            config
                .pubkey
                .starts_with("dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDQxNTc3NkJCNjUxMTYzRjIK")
        );
        assert_eq!(conf["bundle"]["createUpdaterArtifacts"], Value::Bool(true));
    }

    #[test]
    fn interval_is_six_hours() {
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(21_600));
    }

    #[test]
    fn test_builds_are_debug_builds_without_updates() {
        // Tests compile with debug assertions, like `cargo build` / `tauri dev`.
        assert!(!enabled_for_build());
    }

    #[test]
    fn disabled_updater_never_touches_the_backend_or_store() {
        let store = temp_store("disabled");
        store.save(&announced("0.2.0"), b"installer").expect("save");
        let updater = Updater::new(fake(), store.clone(), false);
        assert_eq!(updater.check_once(), CheckOutcome::Disabled);
        assert_eq!(
            updater.install_pending(&Restart::No, || panic!("must not install")),
            InstallOutcome::Disabled
        );
        let mut slept = false;
        updater.run(|_| {
            slept = true;
            false
        });
        assert!(!slept);
        assert!(calls(&updater).is_empty());
        assert_eq!(store.pending_version().as_deref(), Some("0.2.0"));
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn run_checks_immediately_then_every_interval() {
        let store = temp_store("run");
        let updater = updater(fake(), &store);
        let mut waits = Vec::new();
        updater.run(|d| {
            waits.push(d);
            waits.len() < 3
        });
        assert_eq!(waits, vec![CHECK_INTERVAL; 3]);
        assert_eq!(calls(&updater), vec![Call::Check; 3]);
    }

    #[test]
    fn up_to_date_downloads_nothing() {
        let store = temp_store("uptodate");
        let updater = updater(fake(), &store);
        assert_eq!(updater.check_once(), CheckOutcome::UpToDate);
        assert_eq!(calls(&updater), vec![Call::Check]);
        assert_eq!(store.pending_version(), None);
    }

    #[test]
    fn newer_release_is_downloaded_and_pending() {
        let store = temp_store("download");
        let backend = fake();
        *backend.check.lock() = Some(Ok(Some(announced("0.2.0"))));
        *backend.download.lock() = Some(Ok(b"installer".to_vec()));
        let updater = updater(backend, &store);
        assert_eq!(updater.check_once(), CheckOutcome::Downloaded("0.2.0".into()));
        assert_eq!(store.pending_version().as_deref(), Some("0.2.0"));
        let pending = store.load().expect("load").expect("pending");
        assert_eq!(pending.bytes, b"installer");
        assert_eq!(pending.announced, announced("0.2.0"));

        // The next check finds the same version and does not download again.
        assert_eq!(updater.check_once(), CheckOutcome::AlreadyPending("0.2.0".into()));
        assert_eq!(
            calls(&updater),
            vec![Call::Check, Call::Download("0.2.0".into()), Call::Check]
        );
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn an_even_newer_release_replaces_the_pending_one() {
        let store = temp_store("replace");
        store.save(&announced("0.2.0"), b"old").expect("save");
        let backend = fake();
        *backend.check.lock() = Some(Ok(Some(announced("0.3.0"))));
        *backend.download.lock() = Some(Ok(b"installer".to_vec()));
        let updater = updater(backend, &store);
        assert_eq!(updater.check_once(), CheckOutcome::Downloaded("0.3.0".into()));
        let pending = store.load().expect("load").expect("pending");
        assert_eq!(pending.announced.version, "0.3.0");
        assert_eq!(pending.bytes, b"installer");
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn failures_are_reported_and_keep_nothing() {
        let store = temp_store("fail");
        let backend = fake();
        *backend.check.lock() = Some(Err("offline".into()));
        let updater = updater(backend, &store);
        assert_eq!(updater.check_once(), CheckOutcome::Failed("offline".into()));

        *updater.backend.check.lock() = Some(Ok(Some(announced("0.2.0"))));
        *updater.backend.download.lock() = Some(Err("bad signature".into()));
        assert_eq!(updater.check_once(), CheckOutcome::Failed("bad signature".into()));
        assert_eq!(store.pending_version(), None);

        // The next interval succeeds.
        *updater.backend.download.lock() = Some(Ok(b"installer".to_vec()));
        assert_eq!(updater.check_once(), CheckOutcome::Downloaded("0.2.0".into()));
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn nothing_pending_installs_nothing() {
        let store = temp_store("nothing");
        let updater = updater(fake(), &store);
        assert_eq!(
            updater.install_pending(&Restart::No, || panic!("must not install")),
            InstallOutcome::NothingPending
        );
        assert!(calls(&updater).is_empty());
    }

    #[test]
    fn pending_update_installs_on_quit_without_restart() {
        let store = temp_store("quit");
        store.save(&announced("0.2.0"), b"installer").expect("save");
        let updater = updater(fake(), &store);
        let mut before = 0;
        assert_eq!(
            updater.install_pending(&Restart::No, || before += 1),
            InstallOutcome::Installing("0.2.0".into())
        );
        assert_eq!(before, 1);
        assert_eq!(
            calls(&updater),
            vec![
                Call::Verify("0.2.0".into()),
                Call::Install {
                    version: "0.2.0".into(),
                    restart: Restart::No
                }
            ]
        );
        // Kept until the installer really launched (on Windows the process has ended by then), but marked: if that
        // installer did not install it, the next attempt removes it instead of starting it again.
        assert_eq!(store.pending_version().as_deref(), Some("0.2.0"));
        assert!(store.attempted());
        assert!(matches!(
            updater.install_pending(&Restart::No, || panic!("must not install twice")),
            InstallOutcome::Failed(_)
        ));
        assert_eq!(store.pending_version(), None);
        assert_eq!(
            updater.install_pending(&Restart::No, || panic!("must not install")),
            InstallOutcome::NothingPending
        );
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn pending_update_installs_on_start_with_restart() {
        let store = temp_store("start");
        store.save(&announced("0.3.0"), b"installer").expect("save");
        let updater = updater(fake(), &store);
        assert_eq!(
            updater.install_pending(&Restart::SameArgs, || {}),
            InstallOutcome::Installing("0.3.0".into())
        );
        assert_eq!(
            calls(&updater).last(),
            Some(&Call::Install {
                version: "0.3.0".into(),
                restart: Restart::SameArgs
            })
        );
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn already_installed_pending_update_is_removed() {
        let store = temp_store("stale");
        store.save(&announced("0.1.0"), b"installer").expect("save");
        let updater = updater(fake(), &store);
        assert_eq!(
            updater.install_pending(&Restart::SameArgs, || panic!("must not install")),
            InstallOutcome::Stale
        );
        assert_eq!(store.pending_version(), None);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn tampered_pending_update_is_removed_not_installed() {
        let store = temp_store("tampered");
        store.save(&announced("0.2.0"), b"evil").expect("save");
        let updater = updater(fake(), &store);
        assert_eq!(
            updater.install_pending(&Restart::No, || panic!("must not install")),
            InstallOutcome::Discarded("bad signature".into())
        );
        assert_eq!(store.pending_version(), None);
        assert!(!calls(&updater).iter().any(|c| matches!(c, Call::Install { .. })));
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn damaged_pending_files_are_removed() {
        let store = temp_store("damaged");
        fs::create_dir_all(store.dir()).expect("dir");
        fs::write(store.dir().join(PENDING_JSON), b"{not json").expect("write");
        let updater = updater(fake(), &store);
        assert!(matches!(
            updater.install_pending(&Restart::No, || panic!("must not install")),
            InstallOutcome::Discarded(_)
        ));
        assert!(!store.dir().join(PENDING_JSON).exists());

        // A marker without its installer is damaged too.
        fs::write(
            store.dir().join(PENDING_JSON),
            serde_json::to_vec(&announced("0.2.0")).expect("json"),
        )
        .expect("write");
        assert!(matches!(
            updater.install_pending(&Restart::No, || panic!("must not install")),
            InstallOutcome::Discarded(_)
        ));
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn failed_installer_start_does_not_loop() {
        let store = temp_store("installfail");
        store.save(&announced("0.2.0"), b"installer").expect("save");
        let backend = Fake {
            install_error: Some("ShellExecute failed".into()),
            ..fake()
        };
        let updater = updater(backend, &store);
        assert_eq!(
            updater.install_pending(&Restart::SameArgs, || {}),
            InstallOutcome::Failed("ShellExecute failed".into())
        );
        // Removed when the installer did not start, so the next start does not try again.
        assert_eq!(
            updater.install_pending(&Restart::SameArgs, || panic!("must not retry")),
            InstallOutcome::NothingPending
        );
        let _ = fs::remove_dir_all(store.dir());
    }

    /// An updater whose listener records every status it is told about.
    fn recording(backend: Fake, store: &PendingStore) -> (Updater<Fake>, Arc<Mutex<Vec<UpdateView>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let updater = updater(backend, store).with_listener(move |view| sink.lock().push(view.clone()));
        (updater, seen)
    }

    fn statuses(seen: &Mutex<Vec<UpdateView>>) -> Vec<UpdateStatus> {
        seen.lock().iter().map(|v| v.status.clone()).collect()
    }

    fn ready(version: &str) -> UpdateStatus {
        UpdateStatus::Ready {
            version: version.into(),
        }
    }

    fn update_now() -> Restart {
        Restart::WithArgs(Vec::new())
    }

    #[test]
    fn status_starts_idle_disabled_or_ready() {
        let store = temp_store("status-start");
        assert_eq!(updater(fake(), &store).view().status, UpdateStatus::Idle);
        let disabled = Updater::new(fake(), store.clone(), false);
        assert_eq!(
            disabled.view(),
            UpdateView {
                status: UpdateStatus::Disabled {
                    reason: DISABLED_REASON.into()
                },
                pending: None,
                seq: 0,
            }
        );
        store.save(&announced("0.2.0"), b"installer").expect("save");
        let view = updater(fake(), &store).view();
        assert_eq!(view.status, ready("0.2.0"));
        assert_eq!(view.pending.as_deref(), Some("0.2.0"));
        let _ = fs::remove_dir_all(store.dir());
    }

    /// The start path removes a pending update that is stale or invalid: the page then shows idle, never a "ready"
    /// for an update that is gone.
    #[test]
    fn start_install_that_removes_the_update_shows_idle_not_ready() {
        for (version, bytes) in [("0.1.0", &b"installer"[..]), ("0.2.0", &b"evil"[..])] {
            let store = temp_store("status-start-stale");
            store.save(&announced(version), bytes).expect("save");
            let (updater, seen) = recording(fake(), &store);
            assert_eq!(updater.view().status, ready(version));
            let outcome = updater.install_pending(&Restart::SameArgs, || panic!("must not install"));
            assert!(
                matches!(outcome, InstallOutcome::Stale | InstallOutcome::Discarded(_)),
                "{outcome:?}"
            );
            assert_eq!(
                updater.view(),
                UpdateView {
                    status: UpdateStatus::Idle,
                    pending: None,
                    seq: 0
                }
            );
            assert!(seen.lock().is_empty());
            let _ = fs::remove_dir_all(store.dir());
        }
    }

    #[test]
    fn status_up_to_date() {
        let store = temp_store("status-uptodate");
        let (updater, seen) = recording(fake(), &store);
        assert_eq!(updater.check_once(), CheckOutcome::UpToDate);
        assert_eq!(
            statuses(&seen),
            vec![UpdateStatus::Checking, UpdateStatus::UpToDate]
        );
        assert_eq!(updater.view().pending, None);
    }

    #[test]
    fn status_downloading_then_ready() {
        let store = temp_store("status-download");
        let backend = fake();
        *backend.check.lock() = Some(Ok(Some(announced("0.2.0"))));
        *backend.download.lock() = Some(Ok(b"installer".to_vec()));
        let (updater, seen) = recording(backend, &store);
        assert_eq!(updater.check_once(), CheckOutcome::Downloaded("0.2.0".into()));
        assert_eq!(
            statuses(&seen),
            vec![
                UpdateStatus::Checking,
                UpdateStatus::Downloading {
                    version: "0.2.0".into()
                },
                ready("0.2.0"),
            ]
        );
        assert_eq!(
            seen.lock().last().and_then(|v| v.pending.clone()).as_deref(),
            Some("0.2.0")
        );

        // A second check finds it already downloaded: ready again, no download.
        seen.lock().clear();
        assert_eq!(updater.check_once(), CheckOutcome::AlreadyPending("0.2.0".into()));
        assert_eq!(statuses(&seen), vec![UpdateStatus::Checking, ready("0.2.0")]);

        // The pending update disappears (removed elsewhere): the view follows the store, no stale "ready".
        store.clear().expect("clear");
        assert_eq!(updater.view().status, UpdateStatus::UpToDate);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn every_status_change_has_a_higher_seq() {
        let store = temp_store("status-seq");
        let backend = fake();
        *backend.check.lock() = Some(Ok(Some(announced("0.2.0"))));
        *backend.download.lock() = Some(Ok(b"installer".to_vec()));
        let (updater, seen) = recording(backend, &store);
        let before = updater.view().seq;
        updater.check_once();
        let seqs: Vec<u64> = seen.lock().iter().map(|v| v.seq).collect();
        assert_eq!(seqs, vec![before + 1, before + 2, before + 3]);
        assert_eq!(updater.view().seq, before + 3, "reading does not count");
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn status_failed_keeps_a_pending_update_installable() {
        let store = temp_store("status-fail");
        let backend = fake();
        *backend.check.lock() = Some(Err("offline".into()));
        let (updater, seen) = recording(backend, &store);
        assert_eq!(updater.check_once(), CheckOutcome::Failed("offline".into()));
        assert_eq!(
            statuses(&seen),
            vec![
                UpdateStatus::Checking,
                UpdateStatus::Failed {
                    error: "offline".into()
                }
            ]
        );

        // Failed download of a newer version while an older one is pending: the old one stays installable.
        store.save(&announced("0.2.0"), b"installer").expect("save");
        *updater.backend.check.lock() = Some(Ok(Some(announced("0.3.0"))));
        *updater.backend.download.lock() = Some(Err("bad signature".into()));
        assert_eq!(updater.check_once(), CheckOutcome::Failed("bad signature".into()));
        let view = updater.view();
        assert_eq!(
            view.status,
            UpdateStatus::Failed {
                error: "bad signature".into()
            }
        );
        assert_eq!(view.pending.as_deref(), Some("0.2.0"));

        // Up to date with something still pending: it shows as ready.
        *updater.backend.check.lock() = Some(Ok(None));
        assert_eq!(updater.check_once(), CheckOutcome::UpToDate);
        assert_eq!(updater.view().status, ready("0.2.0"));
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_running_check_is_not_started_twice() {
        let store = temp_store("status-running");
        let (updater, seen) = recording(fake(), &store);
        let guard = updater.claim().expect("free");
        assert_eq!(updater.check_once(), CheckOutcome::AlreadyRunning);
        assert_eq!(
            updater.install_now(update_now(), || panic!("must not install")),
            InstallOutcome::Busy
        );
        assert!(calls(&updater).is_empty());
        assert!(seen.lock().is_empty(), "nothing started, nothing reported");
        drop(guard);
        assert_eq!(updater.check_once(), CheckOutcome::UpToDate);
        // The flag is free again after a check.
        assert!(updater.claim().is_some());
    }

    #[test]
    fn a_check_blocks_a_concurrent_check() {
        /// A backend whose check waits until the test lets it go.
        #[derive(Debug)]
        struct Slow {
            entered: std::sync::mpsc::SyncSender<()>,
            release: Mutex<std::sync::mpsc::Receiver<()>>,
            checks: AtomicUsize,
        }
        impl UpdateBackend for Slow {
            fn check(&self) -> Result<Option<Announced>, String> {
                self.checks.fetch_add(1, Ordering::SeqCst);
                let _ = self.entered.send(());
                let _ = self.release.lock().recv();
                Ok(None)
            }
            fn download(&self, _: &Announced) -> Result<Vec<u8>, String> {
                Err("unused".into())
            }
            fn verify(&self, _: &Value, _: &[u8]) -> Result<Option<String>, String> {
                Ok(None)
            }
            fn install(&self, _: &Value, _: &[u8], _: &Restart) -> Result<(), String> {
                Ok(())
            }
        }
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let store = temp_store("status-concurrent");
        let updater = Arc::new(Updater::new(
            Slow {
                entered: entered_tx,
                release: Mutex::new(release_rx),
                checks: AtomicUsize::new(0),
            },
            store,
            true,
        ));
        let background = {
            let updater = updater.clone();
            std::thread::spawn(move || updater.check_once())
        };
        entered_rx.recv().expect("background check started");
        assert_eq!(updater.view().status, UpdateStatus::Checking);
        // "Check for updates" while the background check runs.
        assert_eq!(updater.check_once(), CheckOutcome::AlreadyRunning);
        release_tx.send(()).expect("release");
        assert_eq!(background.join().expect("join"), CheckOutcome::UpToDate);
        assert_eq!(updater.backend.checks.load(Ordering::SeqCst), 1);
        assert_eq!(updater.view().status, UpdateStatus::UpToDate);
    }

    #[test]
    fn update_now_installs_with_its_restart_after_before_install() {
        let store = temp_store("status-install");
        store.save(&announced("0.2.0"), b"installer").expect("save");
        let (updater, seen) = recording(fake(), &store);
        let mut released = false;
        let restart = Restart::WithArgs(vec!["--other".into()]);
        assert_eq!(
            updater.install_now(restart.clone(), || released = true),
            InstallOutcome::Installing("0.2.0".into())
        );
        assert!(released, "devices are released before the installer starts");
        assert_eq!(
            calls(&updater),
            vec![
                Call::Verify("0.2.0".into()),
                Call::Install {
                    version: "0.2.0".into(),
                    restart
                }
            ]
        );
        assert_eq!(
            statuses(&seen),
            vec![UpdateStatus::Installing {
                version: "0.2.0".into()
            }]
        );
        // Not deleted before the installer launched; only marked.
        assert_eq!(store.pending_version().as_deref(), Some("0.2.0"));
        assert!(store.attempted());
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn update_now_failures_show_in_the_status() {
        // The installer does not start: devices were already released, the status says why.
        let store = temp_store("status-install-fail");
        store.save(&announced("0.2.0"), b"installer").expect("save");
        let backend = Fake {
            install_error: Some("ShellExecute failed".into()),
            ..fake()
        };
        let (updater, seen) = recording(backend, &store);
        let mut released = false;
        assert_eq!(
            updater.install_now(update_now(), || released = true),
            InstallOutcome::Failed("ShellExecute failed".into())
        );
        assert!(released);
        assert_eq!(
            statuses(&seen).last(),
            Some(&UpdateStatus::Failed {
                error: "ShellExecute failed".into()
            })
        );
        assert_eq!(store.pending_version(), None);

        // A tampered update is removed without releasing anything.
        store.save(&announced("0.2.0"), b"evil").expect("save");
        let (updater, seen) = recording(fake(), &store);
        assert_eq!(
            updater.install_now(update_now(), || panic!("must not release")),
            InstallOutcome::Discarded("bad signature".into())
        );
        assert_eq!(
            statuses(&seen),
            vec![UpdateStatus::Failed {
                error: "bad signature".into()
            }]
        );
        assert_eq!(updater.view().pending, None);

        // An update that is no longer newer: up to date.
        store.save(&announced("0.1.0"), b"installer").expect("save");
        let (updater, seen) = recording(fake(), &store);
        assert_eq!(updater.view().status, ready("0.1.0"));
        assert_eq!(
            updater.install_now(update_now(), || panic!("must not release")),
            InstallOutcome::Stale
        );
        assert_eq!(statuses(&seen), vec![UpdateStatus::UpToDate]);
        assert_eq!(updater.view().status, UpdateStatus::UpToDate);

        // Nothing pending: nothing happens.
        assert_eq!(
            updater.install_now(update_now(), || panic!("must not release")),
            InstallOutcome::NothingPending
        );
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn disabled_updater_does_not_install_now() {
        let store = temp_store("status-disabled");
        store.save(&announced("0.2.0"), b"installer").expect("save");
        let updater = Updater::new(fake(), store.clone(), false);
        assert_eq!(
            updater.install_now(update_now(), || panic!("must not release")),
            InstallOutcome::Disabled
        );
        assert!(calls(&updater).is_empty());
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn status_json_shape() {
        let view = UpdateView {
            status: UpdateStatus::Downloading {
                version: "0.2.0".into(),
            },
            pending: None,
            seq: 7,
        };
        assert_eq!(
            serde_json::to_value(&view).expect("json"),
            json!({ "status": { "state": "downloading", "version": "0.2.0" }, "pending": null, "seq": 7 })
        );
        assert_eq!(
            serde_json::to_value(UpdateStatus::UpToDate).expect("json"),
            json!({ "state": "up_to_date" })
        );
        assert_eq!(
            serde_json::to_value(UpdateStatus::Disabled {
                reason: DISABLED_REASON.into()
            })
            .expect("json"),
            json!({ "state": "disabled", "reason": "Updates are only available in installed builds" })
        );
    }

    #[test]
    fn nsis_restart_arguments_match_the_plugin() {
        assert_eq!(nsis_restart_args(&[]), vec!["/R"]);
        assert_eq!(
            nsis_restart_args(&["--other".into(), "two words".into(), "a/b".into(), String::new()]),
            vec!["/R", "/ARGS", "--other", "\"two words\"", "\"a/b\"", "\"\""]
        );
        // Quotes inside: backslashes before a quote doubled plus one; trailing backslashes doubled when quoted.
        assert_eq!(escape_nsis_arg(r#"say"hi"#), r#"say\"hi"#);
        assert_eq!(escape_nsis_arg(r#"a\"b"#), r#"a\\\"b"#);
        assert_eq!(escape_nsis_arg(r"C:\dir with space\"), r#""C:\dir with space\\""#);
        assert_eq!(escape_nsis_arg(r"C:\plain"), r"C:\plain");
    }

    #[test]
    fn store_save_load_clear_round_trip() {
        let store = temp_store("store");
        assert_eq!(store.load().expect("load"), None);
        store.save(&announced("0.2.0"), b"abc").expect("save");
        assert_eq!(
            store.load().expect("load"),
            Some(Pending {
                announced: announced("0.2.0"),
                bytes: b"abc".to_vec()
            })
        );
        assert!(!store.dir().join("pending.tmp").exists());
        store.clear().expect("clear");
        store.clear().expect("clear twice");
        assert_eq!(store.load().expect("load"), None);
        let _ = fs::remove_dir_all(store.dir());
    }
}
