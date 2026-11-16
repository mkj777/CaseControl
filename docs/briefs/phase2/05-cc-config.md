# Brief: `crates/cc-config` (Phase 2)

Read `00-common-rules.md` first. You own `crates/cc-config/**` except that `src/profile.rs` types (`DeviceSetting`,
`Profile`) keep their shape: you may add methods, docs and tests, not change fields, variants or serde attributes.
The engine worker builds against them at the same time.

## Goal

Settings and profiles as TOML files, per architecture "Config". No I/O outside the paths below. No registry
(autostart lives in the desktop app via `tauri-plugin-autostart`; the `autostart` flag here only records intent).

## Contracts

- `Paths { config_dir, profiles_dir, logs_dir }` with `Paths::from_env()` (`%APPDATA%\CaseControl`,
  `%APPDATA%\CaseControl\profiles`, `%LOCALAPPDATA%\CaseControl\logs`, via `directories` or the env vars) and
  `Paths::at(root: &Path)` for tests (everything under `root`). `ensure()` creates the directories.
- `Settings` (serde, `Default`, every field `#[serde(default)]` so old or partial files load):
  `autostart: bool` (false), `start_minimized: bool` (false), `frame_rate: u32` (30, clamped to 1..=60 on load),
  `detectors: BTreeMap<String, bool>` (missing family = enabled; helper `disabled_families() -> HashSet<String>` and
  `is_enabled(family)`), `conflicts: ConflictSettings { check_openrgb_port: bool }` (false),
  `last_profile: Option<String>`, `devices: BTreeMap<DeviceId, DeviceOverrides>` with
  `DeviceOverrides { zone_sizes: BTreeMap<String, usize> }` keyed by zone name (zone names are stable; indices are not
  guaranteed to be) and `#[serde(flatten)] extra: toml::Table` on `Settings` so unknown keys survive a load/save cycle.
- `Settings::load(&Paths) -> Result<Settings, ConfigError>`: missing file = defaults (and no file is written).
  A file that fails to parse is renamed to `config.toml.broken-<unix seconds>` and defaults are returned, with a
  `tracing::warn!` (the app must start even with a broken file). `Settings::save(&self, &Paths)`: atomic write
  (write `config.toml.tmp`, then rename over the target).
- Comments: if editing in place with `toml_edit` (preserving comments and key order of an existing file) fits in a
  small, well-tested module, do it and add `toml_edit` to your Cargo.toml; otherwise plain `toml` with unknown keys
  preserved is acceptable. Report which you chose.
- `ProfileStore::new(&Paths)`: `list() -> Result<Vec<String>>` (profile names sorted case-insensitively, read from the
  `name` field, files that fail to parse are skipped with a warning), `load(name)`, `save(&Profile)` (atomic, file name
  derived from the name: keep `[A-Za-z0-9 _-]`, replace the rest with `_`, max 64 chars, collision with a different
  existing name gets a numeric suffix), `delete(name)`, `rename(old, new)`. Names are trimmed, non-empty, at most 64
  chars, unique case-insensitively (`ConfigError::InvalidName`, `ConfigError::Exists`, `ConfigError::NotFound`).
- `DeviceSetting::fit_to(&self, led_count: usize) -> DeviceSetting` implementing the pad/truncate rule documented on
  the type (empty buffer -> black).
- `ConfigError` via thiserror (`Io { path, source }`, `Parse { path, message }`, `InvalidName`, `Exists`, `NotFound`).

## Tests

Defaults; round trip of a full `Settings` and of a `Profile` with both setting kinds and a `ModeState` with every
optional field; unknown keys survive; broken file is moved aside; frame-rate clamp; profile store CRUD with name
sanitizing and collisions in a temp dir (`tempfile` as a dev-dependency is fine); a TOML snapshot of a sample profile
so the on-disk format is visible in the test file.
