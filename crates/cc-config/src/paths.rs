//! Where settings, profiles and logs live.

use std::path::{Path, PathBuf};

use crate::error::{ConfigError, Result};

/// Name of the application directory under the roaming and local app data folders.
const APP_DIR: &str = "CaseControl";
/// File name of the settings file inside [`Paths::config_dir`].
const CONFIG_FILE: &str = "config.toml";
/// File name of the machine state file inside [`Paths::state_dir`] (ADR 0001 #38), and of the old one inside
/// [`Paths::config_dir`].
const HARDWARE_STATE_FILE: &str = "hardware-state.toml";
/// File name of the device-memory records inside [`Paths::state_dir`] (ADR 0001 #42).
pub(crate) const DEVICE_MEMORY_FILE: &str = "device-memory.toml";

/// The directories this crate reads and writes. Nothing outside them is touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Holds `config.toml` (`%APPDATA%\CaseControl`).
    pub config_dir: PathBuf,
    /// One TOML file per profile (`%APPDATA%\CaseControl\profiles`).
    pub profiles_dir: PathBuf,
    /// Log files, written by the desktop app (`%LOCALAPPDATA%\CaseControl\logs`).
    pub logs_dir: PathBuf,
}

impl Paths {
    /// The standard locations of the current user.
    ///
    /// On Windows: `%APPDATA%\CaseControl`, `%APPDATA%\CaseControl\profiles` and `%LOCALAPPDATA%\CaseControl\logs`
    /// (the platform's roaming config and local data folders elsewhere). `None` when the user's folders cannot be
    /// determined.
    pub fn from_env() -> Option<Self> {
        let base = directories::BaseDirs::new()?;
        let config_dir = base.config_dir().join(APP_DIR);
        Some(Self {
            profiles_dir: config_dir.join("profiles"),
            logs_dir: base.data_local_dir().join(APP_DIR).join("logs"),
            config_dir,
        })
    }

    /// Everything under `root` (`root`, `root/profiles`, `root/logs`); for tests and portable setups.
    pub fn at(root: &Path) -> Self {
        Self {
            config_dir: root.to_path_buf(),
            profiles_dir: root.join("profiles"),
            logs_dir: root.join("logs"),
        }
    }

    /// Full path of the settings file.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILE)
    }

    /// The machine-local folder of the machine state: the parent of [`logs_dir`](Self::logs_dir)
    /// (`%LOCALAPPDATA%\CaseControl`), or [`config_dir`](Self::config_dir) when the logs folder has no parent. Not
    /// roaming, so a profile that follows the user to another PC does not carry device backups along.
    pub fn state_dir(&self) -> PathBuf {
        self.logs_dir
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| self.config_dir.clone(), Path::to_path_buf)
    }

    /// Full path of the machine state file ([`HardwareState`](crate::HardwareState)): own lighting and markers, in
    /// [`state_dir`](Self::state_dir).
    pub fn hardware_state_file(&self) -> PathBuf {
        self.state_dir().join(HARDWARE_STATE_FILE)
    }

    /// Full path of the device-memory records ([`HardwareState`](crate::HardwareState), ADR 0001 #42), in
    /// [`state_dir`](Self::state_dir).
    pub fn device_memory_file(&self) -> PathBuf {
        self.state_dir().join(DEVICE_MEMORY_FILE)
    }

    /// Where the machine state lived before it moved to [`state_dir`](Self::state_dir): `hardware-state.toml` in
    /// [`config_dir`](Self::config_dir) (`%APPDATA%\CaseControl`). Read for the migration on every start while the
    /// new file is missing (a failed or skipped migration is retried), never written.
    pub fn legacy_hardware_state_file(&self) -> PathBuf {
        self.config_dir.join(HARDWARE_STATE_FILE)
    }

    /// Creates all three directories (and their parents) if they do not exist.
    pub fn ensure(&self) -> Result<()> {
        for dir in [&self.config_dir, &self.profiles_dir, &self.logs_dir] {
            std::fs::create_dir_all(dir).map_err(|e| ConfigError::io(dir, e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_puts_everything_under_root() {
        let root = Path::new("some").join("root");
        let p = Paths::at(&root);
        assert_eq!(p.config_dir, root);
        assert_eq!(p.profiles_dir, root.join("profiles"));
        assert_eq!(p.logs_dir, root.join("logs"));
        assert_eq!(p.config_file(), root.join("config.toml"));
        assert_eq!(p.hardware_state_file(), root.join("hardware-state.toml"));
        assert_eq!(p.device_memory_file(), root.join("device-memory.toml"));
        assert_eq!(p.legacy_hardware_state_file(), root.join("hardware-state.toml"));
    }

    #[test]
    fn machine_state_lives_next_to_the_logs() {
        let roaming = Path::new("u").join("Roaming").join("CaseControl");
        let local = Path::new("u").join("Local").join("CaseControl");
        let p = Paths {
            config_dir: roaming.clone(),
            profiles_dir: roaming.join("profiles"),
            logs_dir: local.join("logs"),
        };
        assert_eq!(p.state_dir(), local);
        assert_eq!(p.hardware_state_file(), local.join("hardware-state.toml"));
        assert_eq!(p.device_memory_file(), local.join("device-memory.toml"));
        assert_eq!(
            p.legacy_hardware_state_file(),
            roaming.join("hardware-state.toml")
        );
        let flat = Paths {
            logs_dir: PathBuf::from("logs"),
            ..p
        };
        assert_eq!(flat.state_dir(), roaming);
    }

    #[test]
    fn from_env_uses_app_dir() {
        let p = Paths::from_env().expect("user folders available");
        assert!(p.config_dir.ends_with("CaseControl"));
        assert!(
            p.profiles_dir
                .ends_with(Path::new("CaseControl").join("profiles"))
        );
        assert!(p.logs_dir.ends_with(Path::new("CaseControl").join("logs")));
    }

    #[test]
    fn ensure_creates_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = Paths::at(&tmp.path().join("nested"));
        p.ensure().expect("ensure");
        assert!(p.config_dir.is_dir() && p.profiles_dir.is_dir() && p.logs_dir.is_dir());
        p.ensure().expect("ensure is idempotent");
    }
}
