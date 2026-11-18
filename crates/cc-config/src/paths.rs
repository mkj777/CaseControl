//! Where settings, profiles and logs live.

use std::path::{Path, PathBuf};

use crate::error::{ConfigError, Result};

/// Name of the application directory under the roaming and local app data folders.
const APP_DIR: &str = "CaseControl";
/// File name of the settings file inside [`Paths::config_dir`].
const CONFIG_FILE: &str = "config.toml";
/// File name of the machine state file inside [`Paths::config_dir`] (ADR 0001 #38).
const HARDWARE_STATE_FILE: &str = "hardware-state.toml";

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

    /// Full path of the machine state file ([`HardwareState`](crate::HardwareState)).
    pub fn hardware_state_file(&self) -> PathBuf {
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
