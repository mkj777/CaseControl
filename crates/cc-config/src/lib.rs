//! CaseControl settings and profiles, stored as TOML files.
//!
//! - [`Paths`]: where things live (`%APPDATA%\CaseControl\config.toml`, `%APPDATA%\CaseControl\profiles\*.toml`,
//!   `%LOCALAPPDATA%\CaseControl\logs`), or everything under one root for tests.
//! - [`Settings`]: application settings. Missing keys take defaults, unknown keys survive a load/save cycle, a
//!   broken file is moved aside instead of stopping the app, and saving updates an existing file in place with
//!   `toml_edit` so the user's comments and key order are kept. Writes are atomic (temp file, then rename).
//! - [`ProfileStore`]: one file per [`Profile`], identified by the profile's `name` (case-insensitive); file names
//!   are derived from it and sanitized.
//! - [`DeviceSetting`] / [`Profile`]: the profile types shared with `cc-engine`.
//!
//! What this crate deliberately does not do: no I/O outside [`Paths`], no registry access (the `autostart` flag only
//! records intent; the desktop app applies it), no device access, no network, no file watching, and no logging setup
//! (it only emits `tracing` events).

mod error;
mod fsutil;
mod merge;
mod paths;
pub mod profile;
mod settings;
mod store;

pub use error::{ConfigError, Result};
pub use paths::Paths;
pub use profile::{DeviceSetting, Profile};
pub use settings::{ConflictSettings, DEFAULT_FRAME_RATE, DeviceOverrides, FRAME_RATE_RANGE, Settings};
pub use store::{MAX_NAME_LEN, ProfileStore, file_stem_for, validate_name};
