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
//! - Nothing is ever shown to the user: no dialog, no error. Every step is logged; a failure (offline, GitHub down,
//!   bad signature) is retried at the next interval. The NSIS installer runs in quiet mode, per user.
//! - Debug builds never check, download or install ([`enabled_for_build`]).
//!
//! The decisions live in [`Updater`], which talks to the plugin only through [`UpdateBackend`] so the state machine is
//! unit-tested without network or installer; [`TauriBackend`] is the real implementation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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
    /// `restart`: the installer starts the new version afterwards with the current arguments.
    fn install(&self, manifest: &Value, bytes: &[u8], restart: bool) -> Result<(), String>;
}

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
/// and renamed, so a quit during a download never sees a half-written installer.
#[derive(Debug, Clone)]
pub struct PendingStore {
    dir: PathBuf,
}

const PENDING_JSON: &str = "pending.json";
const PENDING_BIN: &str = "pending.bin";

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
        write_atomic(&self.dir.join(PENDING_BIN), bytes)?;
        let json = serde_json::to_vec_pretty(announced)
            .map_err(|err| format!("cannot serialize the pending update: {err}"))?;
        write_atomic(&self.dir.join(PENDING_JSON), &json)
    }

    /// Removes the pending update (marker first). Missing files are fine.
    pub fn clear(&self) -> Result<(), String> {
        remove_if_exists(&self.dir.join(PENDING_JSON))?;
        remove_if_exists(&self.dir.join(PENDING_BIN))
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
    /// The installer could not be started; the pending update was removed and is downloaded again later.
    Failed(String),
}

/// The update state machine: when to check, what to keep, when to install.
#[derive(Debug)]
pub struct Updater<B> {
    backend: B,
    store: PendingStore,
    enabled: bool,
}

impl<B: UpdateBackend> Updater<B> {
    /// An updater over `backend` keeping its pending update in `store`. With `enabled == false` (debug builds, see
    /// [`enabled_for_build`]) every operation is a logged no-op.
    pub fn new(backend: B, store: PendingStore, enabled: bool) -> Self {
        Self {
            backend,
            store,
            enabled,
        }
    }

    /// Whether this updater checks and installs at all.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// One check: ask the endpoint, download a newer release unless it is already pending, store it.
    pub fn check_once(&self) -> CheckOutcome {
        if !self.enabled {
            return CheckOutcome::Disabled;
        }
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
            "update downloaded and verified; installs on the next quit or start"
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

    /// Installs the pending update if there is a valid one. The pending files are removed before the installer
    /// starts, so a failing installer can never cause a start/install loop. `before_install` runs right before the
    /// installer starts (and only then). `restart`: see [`UpdateBackend::install`].
    pub fn install_pending(&self, restart: bool, before_install: impl FnOnce()) -> InstallOutcome {
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
        self.clear_logged();
        before_install();
        tracing::info!(%version, restart, "starting the update installer");
        match self.backend.install(&announced.manifest, &pending.bytes, restart) {
            Ok(()) => InstallOutcome::Installing(version),
            Err(err) => {
                tracing::error!(%version, %err, "cannot start the update installer; downloading it again later");
                InstallOutcome::Failed(err)
            }
        }
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

    fn install(&self, manifest: &Value, bytes: &[u8], restart: bool) -> Result<(), String> {
        let update = self
            .restore(manifest)?
            .ok_or_else(|| "the pending update is no longer an update".to_owned())?;
        update
            .restart_after_install(restart)
            .install(bytes)
            .map_err(|err| err.to_string())
    }
}

/// The updater the app manages as Tauri state.
pub type AppUpdater<R> = Arc<Updater<TauriBackend<R>>>;

/// Creates the app's updater: enabled in release builds that can determine `%LOCALAPPDATA%`.
pub fn create<R: Runtime>(app: &AppHandle<R>) -> AppUpdater<R> {
    let dir = updates_dir();
    let enabled = enabled_for_build() && dir.is_some();
    if enabled_for_build() && dir.is_none() {
        tracing::warn!("cannot determine the local data directory; updates are off");
    }
    if !enabled_for_build() {
        tracing::info!("debug build: updates are off");
    }
    let store = PendingStore::new(dir.unwrap_or_default());
    Arc::new(Updater::new(TauriBackend::new(app.clone()), store, enabled))
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
        Install { version: String, restart: bool },
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
        fn install(&self, manifest: &Value, _bytes: &[u8], restart: bool) -> Result<(), String> {
            self.calls.lock().push(Call::Install {
                version: version_of(manifest),
                restart,
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
            updater.install_pending(false, || panic!("must not install")),
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
            updater.install_pending(false, || panic!("must not install")),
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
            updater.install_pending(false, || before += 1),
            InstallOutcome::Installing("0.2.0".into())
        );
        assert_eq!(before, 1);
        assert_eq!(
            calls(&updater),
            vec![
                Call::Verify("0.2.0".into()),
                Call::Install {
                    version: "0.2.0".into(),
                    restart: false
                }
            ]
        );
        // Consumed: a second exit path (e.g. `RunEvent::Exit` after tray Quit) installs nothing.
        assert_eq!(store.pending_version(), None);
        assert_eq!(
            updater.install_pending(false, || panic!("must not install twice")),
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
            updater.install_pending(true, || {}),
            InstallOutcome::Installing("0.3.0".into())
        );
        assert_eq!(
            calls(&updater).last(),
            Some(&Call::Install {
                version: "0.3.0".into(),
                restart: true
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
            updater.install_pending(true, || panic!("must not install")),
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
            updater.install_pending(false, || panic!("must not install")),
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
            updater.install_pending(false, || panic!("must not install")),
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
            updater.install_pending(false, || panic!("must not install")),
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
            updater.install_pending(true, || {}),
            InstallOutcome::Failed("ShellExecute failed".into())
        );
        // Removed before the installer started, so the next start does not try again.
        assert_eq!(
            updater.install_pending(true, || panic!("must not retry")),
            InstallOutcome::NothingPending
        );
        let _ = fs::remove_dir_all(store.dir());
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
