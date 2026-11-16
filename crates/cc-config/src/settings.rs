//! Application settings (`config.toml`).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use cc_core::DeviceId;
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{ConfigError, Result};
use crate::fsutil::{with_suffix, write_atomic};
use crate::merge;
use crate::paths::Paths;

/// Default frame rate of the engine's frame scheduler, in Hz.
pub const DEFAULT_FRAME_RATE: u32 = 30;
/// Allowed frame rates; values outside are clamped on load.
pub const FRAME_RATE_RANGE: std::ops::RangeInclusive<u32> = 1..=60;

/// Application settings.
///
/// Every field has a default, so old or partial files load. Keys this version does not know are kept in
/// [`Settings::extra`] (and in the `extra` of the nested tables) and written back on save.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The user wants the app to start with Windows. Only records intent: the desktop app applies it through
    /// `tauri-plugin-autostart`; this crate never touches the registry.
    pub autostart: bool,
    /// Start hidden in the tray.
    pub start_minimized: bool,
    /// Maximum updates per second and device; clamped to [`FRAME_RATE_RANGE`] on load.
    #[serde(deserialize_with = "clamped_frame_rate")]
    pub frame_rate: u32,
    /// Detector switch per device family id (see `cc_core::family`). A missing family is enabled.
    pub detectors: BTreeMap<String, bool>,
    /// Conflict scanner options.
    pub conflicts: ConflictSettings,
    /// Name of the profile applied last, re-applied on start.
    pub last_profile: Option<String>,
    /// Per-device overrides, keyed by device id.
    pub devices: BTreeMap<DeviceId, DeviceOverrides>,
    /// Unknown top-level keys, preserved across a load/save cycle.
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autostart: false,
            start_minimized: false,
            frame_rate: DEFAULT_FRAME_RATE,
            detectors: BTreeMap::new(),
            conflicts: ConflictSettings::default(),
            last_profile: None,
            devices: BTreeMap::new(),
            extra: toml::Table::new(),
        }
    }
}

/// Options of the conflict scanner.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConflictSettings {
    /// Probe `127.0.0.1:6742` for a running OpenRGB SDK server. Off by default (the only socket use in the app).
    pub check_openrgb_port: bool,
    /// Catalogue ids of the programs the start dialog no longer asks about (ADR 0001 #31). They still show on the
    /// Conflicts page. Written only when not empty.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub dont_ask: BTreeSet<String>,
    /// Start type a service had before CaseControl set it to Manual ("Turn off autostart"), keyed by service name,
    /// so "Re-enable" restores exactly it. Values are lower-case start type names (`"auto"`, ...). An entry is
    /// removed once the service was re-enabled. Written only when not empty.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub previous_start_types: BTreeMap<String, String>,
    /// Unknown keys of the `[conflicts]` table.
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// User overrides for one device.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceOverrides {
    /// LED count per resizable zone, keyed by zone name (names are stable, indices are not guaranteed to be).
    pub zone_sizes: BTreeMap<String, usize>,
    /// The user hid this device: the app does not control it (ADR 0001 #26). Written only when `true`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// Names of the zones the user hid; they are kept black in direct colour (ADR 0001 #26). Written only when
    /// not empty.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub hidden_zones: BTreeSet<String>,
    /// Unknown keys of this device's table.
    #[serde(flatten)]
    pub extra: toml::Table,
}

fn clamped_frame_rate<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u32, D::Error> {
    let raw = i64::deserialize(d)?;
    let clamped = raw.clamp(
        i64::from(*FRAME_RATE_RANGE.start()),
        i64::from(*FRAME_RATE_RANGE.end()),
    );
    if clamped != raw {
        tracing::warn!(raw, clamped, "frame_rate out of range, clamped");
    }
    Ok(u32::try_from(clamped).unwrap_or(DEFAULT_FRAME_RATE))
}

impl Settings {
    /// Families whose detectors are switched off, in the form `cc_core::DetectContext::disabled_families` takes.
    pub fn disabled_families(&self) -> HashSet<String> {
        self.detectors
            .iter()
            .filter(|(_, on)| !**on)
            .map(|(family, _)| family.clone())
            .collect()
    }

    /// Whether the detectors of `family` may run. Families not listed are enabled.
    pub fn is_enabled(&self, family: &str) -> bool {
        self.detectors.get(family).copied().unwrap_or(true)
    }

    /// Parses settings from TOML text. The frame rate is clamped; unknown keys land in `extra`.
    pub fn from_toml(text: &str) -> std::result::Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Serializes the settings to TOML text (without the comment-preserving merge [`Settings::save`] does).
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string(self).map_err(|e| ConfigError::Serialize(e.to_string()))
    }

    /// Loads `config.toml`.
    ///
    /// A missing file gives the defaults (and writes nothing). A file that does not parse is renamed to
    /// `config.toml.broken-<unix seconds>` and the defaults are returned, so the app always starts. Only a failure
    /// to read an existing file (or to move a broken one aside) is an error.
    pub fn load(paths: &Paths) -> Result<Self> {
        let path = paths.config_file();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(ConfigError::io(path, e)),
        };
        match Self::from_toml(&text) {
            Ok(settings) => Ok(settings),
            Err(err) => {
                let secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let broken = with_suffix(&path, &format!(".broken-{secs}"));
                std::fs::rename(&path, &broken).map_err(|e| ConfigError::io(&path, e))?;
                tracing::warn!(
                    path = %path.display(),
                    moved_to = %broken.display(),
                    error = %err,
                    "config file does not parse; moved aside, using defaults"
                );
                Ok(Self::default())
            }
        }
    }

    /// Writes `config.toml` atomically (`config.toml.tmp`, then rename over the target).
    ///
    /// If a file already exists, it is updated in place: comments, key order and the spelling of unchanged values
    /// are kept. If that is not possible the file is rewritten from scratch.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        let path = paths.config_file();
        let fresh = self.to_toml()?;
        let contents = match std::fs::read_to_string(&path) {
            Ok(existing) => match merge::update_document(&existing, &fresh) {
                Some(merged) if Self::from_toml(&merged).ok().as_ref() == Some(self) => merged,
                _ => {
                    tracing::warn!(path = %path.display(), "cannot update config in place, rewriting it");
                    fresh
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => fresh,
            Err(e) => return Err(ConfigError::io(path, e)),
        };
        write_atomic(&path, &contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::at(tmp.path());
        (tmp, paths)
    }

    fn full_settings() -> Settings {
        let mut s = Settings {
            autostart: true,
            start_minimized: true,
            frame_rate: 45,
            last_profile: Some("Evening".into()),
            ..Settings::default()
        };
        s.detectors.insert("logitech".into(), false);
        s.detectors.insert("wooting".into(), true);
        s.conflicts.check_openrgb_port = true;
        s.conflicts.dont_ask.insert("openrgb".into());
        s.conflicts
            .previous_start_types
            .insert("SignalRgb.Service".into(), "auto".into());
        let mut ov = DeviceOverrides::default();
        ov.zone_sizes.insert("JRAINBOW1".into(), 60);
        ov.zone_sizes.insert("JRAINBOW2".into(), 8);
        ov.hidden_zones.insert("JRGB1".into());
        ov.hidden_zones.insert("JRAINBOW2".into());
        s.devices.insert(DeviceId::hid(0x1462, 0x7D75, None), ov);
        s.devices.insert(
            DeviceId::hid(0x046D, 0xC33C, None),
            DeviceOverrides {
                hidden: true,
                ..DeviceOverrides::default()
            },
        );
        s
    }

    #[test]
    fn defaults() {
        let s = Settings::default();
        assert!(!s.autostart);
        assert!(!s.start_minimized);
        assert_eq!(s.frame_rate, 30);
        assert!(s.detectors.is_empty());
        assert!(!s.conflicts.check_openrgb_port);
        assert_eq!(s.last_profile, None);
        assert!(s.devices.is_empty());
        assert!(s.extra.is_empty());
        assert_eq!(Settings::from_toml("").expect("empty parses"), s);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let s = Settings::from_toml("autostart = true\n[detectors]\nwooting = false\n").expect("parse");
        assert!(s.autostart);
        assert_eq!(s.frame_rate, 30);
        assert!(!s.is_enabled("wooting"));
        assert!(s.is_enabled("logitech"));
    }

    #[test]
    fn detector_helpers() {
        let s = full_settings();
        assert_eq!(s.disabled_families(), HashSet::from(["logitech".to_owned()]));
        assert!(!s.is_enabled("logitech"));
        assert!(s.is_enabled("wooting"));
        assert!(s.is_enabled("msi_mystic_light"));
    }

    #[test]
    fn frame_rate_is_clamped() {
        for (raw, expected) in [
            (0, 1),
            (-5, 1),
            (1, 1),
            (30, 30),
            (60, 60),
            (61, 60),
            (100000, 60),
        ] {
            let s = Settings::from_toml(&format!("frame_rate = {raw}")).expect("parse");
            assert_eq!(s.frame_rate, expected, "raw {raw}");
        }
    }

    #[test]
    fn full_round_trip() {
        let s = full_settings();
        let text = s.to_toml().expect("serialize");
        assert_eq!(Settings::from_toml(&text).expect("parse"), s);
    }

    #[test]
    fn unknown_keys_survive() {
        let text = "future_flag = 3\nautostart = true\n\n[conflicts]\ncheck_openrgb_port = true\nnew_scan = \"x\"\n\n\
                    [devices.\"hid:1462:7d75\"]\ncolor_order = \"GRB\"\n\n[devices.\"hid:1462:7d75\".zone_sizes]\nJRAINBOW1 = 30\n\n\
                    [future_table]\nk = [1, 2]\n";
        let s = Settings::from_toml(text).expect("parse");
        assert_eq!(s.extra.get("future_flag"), Some(&toml::Value::Integer(3)));
        assert!(s.extra.contains_key("future_table"));
        assert!(s.conflicts.extra.contains_key("new_scan"));
        let dev = &s.devices[&DeviceId::hid(0x1462, 0x7D75, None)];
        assert_eq!(dev.zone_sizes["JRAINBOW1"], 30);
        assert!(dev.extra.contains_key("color_order"));

        let again = Settings::from_toml(&s.to_toml().expect("serialize")).expect("reparse");
        assert_eq!(again, s);
    }

    #[test]
    fn hidden_flags_round_trip() {
        let s = full_settings();
        let text = s.to_toml().expect("serialize");
        assert!(text.contains("hidden = true"), "{text}");
        assert!(text.contains("hidden_zones = ["), "{text}");
        let back = Settings::from_toml(&text).expect("parse");
        let board = &back.devices[&DeviceId::hid(0x1462, 0x7D75, None)];
        assert!(!board.hidden);
        assert_eq!(
            board.hidden_zones,
            BTreeSet::from(["JRAINBOW2".to_owned(), "JRGB1".to_owned()])
        );
        assert!(back.devices[&DeviceId::hid(0x046D, 0xC33C, None)].hidden);
        assert_eq!(back, s);
    }

    #[test]
    fn old_device_tables_load_without_hidden_flags() {
        let text = "[devices.\"hid:1462:7d75\".zone_sizes]
JRAINBOW1 = 30
";
        let s = Settings::from_toml(text).expect("parse");
        let dev = &s.devices[&DeviceId::hid(0x1462, 0x7D75, None)];
        assert!(!dev.hidden);
        assert!(dev.hidden_zones.is_empty());
        assert!(dev.extra.is_empty());
    }

    #[test]
    fn unhidden_devices_write_no_hidden_keys() {
        let mut s = Settings::default();
        let mut ov = DeviceOverrides::default();
        ov.zone_sizes.insert("JRAINBOW1".into(), 30);
        s.devices.insert(DeviceId::hid(0x1462, 0x7D75, None), ov);
        let text = s.to_toml().expect("serialize");
        assert!(!text.contains("hidden"), "{text}");
    }

    #[test]
    fn existing_file_with_device_table_stays_identical() {
        let (_tmp, paths) = temp_paths();
        let original = "autostart = false
start_minimized = false
frame_rate = 30

[conflicts]
                        check_openrgb_port = false

[detectors]

[devices.\"hid:1462:7d75\".zone_sizes]
                        JRAINBOW1 = 30
";
        std::fs::write(paths.config_file(), original).expect("write");
        Settings::load(&paths).expect("load").save(&paths).expect("save");
        assert_eq!(
            std::fs::read_to_string(paths.config_file()).expect("read"),
            original
        );
    }

    #[test]
    fn unhiding_removes_the_keys_from_the_file() {
        let (_tmp, paths) = temp_paths();
        let id = DeviceId::hid(0x1462, 0x7D75, None);
        let mut s = Settings::default();
        let ov = s.devices.entry(id.clone()).or_default();
        ov.hidden = true;
        ov.hidden_zones.insert("JRGB1".into());
        s.save(&paths).expect("save hidden");
        let ov = s.devices.get_mut(&id).expect("device entry");
        ov.hidden = false;
        ov.hidden_zones.clear();
        s.save(&paths).expect("save unhidden");
        let text = std::fs::read_to_string(paths.config_file()).expect("read");
        assert!(!text.contains("hidden"), "{text}");
        assert_eq!(Settings::load(&paths).expect("reload"), s);
    }

    #[test]
    fn conflict_memory_round_trips_and_is_omitted_when_empty() {
        let s = full_settings();
        let text = s.to_toml().expect("serialize");
        assert!(text.contains("dont_ask = [\"openrgb\"]"), "{text}");
        assert!(text.contains("previous_start_types"), "{text}");
        let back = Settings::from_toml(&text).expect("parse");
        assert!(back.conflicts.dont_ask.contains("openrgb"));
        assert_eq!(back.conflicts.previous_start_types["SignalRgb.Service"], "auto");
        assert_eq!(back, s);

        let text = Settings::default().to_toml().expect("serialize");
        assert!(!text.contains("dont_ask"), "{text}");
        assert!(!text.contains("previous_start_types"), "{text}");
    }

    #[test]
    fn old_conflicts_table_loads_with_empty_memory() {
        let s = Settings::from_toml(
            "[conflicts]
check_openrgb_port = true
",
        )
        .expect("parse");
        assert!(s.conflicts.dont_ask.is_empty());
        assert!(s.conflicts.previous_start_types.is_empty());
        assert!(s.conflicts.extra.is_empty());
    }

    #[test]
    fn clearing_dont_ask_removes_the_key_from_the_file() {
        let (_tmp, paths) = temp_paths();
        let mut s = Settings::default();
        s.conflicts.dont_ask.insert("signalrgb".into());
        s.save(&paths).expect("save");
        assert!(
            std::fs::read_to_string(paths.config_file())
                .expect("read")
                .contains("dont_ask")
        );
        s.conflicts.dont_ask.clear();
        s.save(&paths).expect("save");
        let text = std::fs::read_to_string(paths.config_file()).expect("read");
        assert!(!text.contains("dont_ask"), "{text}");
        assert_eq!(Settings::load(&paths).expect("reload"), s);
    }

    #[test]
    fn missing_file_gives_defaults_and_writes_nothing() {
        let (_tmp, paths) = temp_paths();
        assert_eq!(Settings::load(&paths).expect("load"), Settings::default());
        assert!(!paths.config_file().exists());
    }

    #[test]
    fn save_then_load() {
        let (_tmp, paths) = temp_paths();
        let s = full_settings();
        s.save(&paths).expect("save");
        assert_eq!(Settings::load(&paths).expect("load"), s);
        assert!(!with_suffix(&paths.config_file(), ".tmp").exists());
    }

    #[test]
    fn broken_file_is_moved_aside() {
        let (_tmp, paths) = temp_paths();
        std::fs::write(paths.config_file(), "autostart = = nope").expect("write");
        assert_eq!(Settings::load(&paths).expect("load"), Settings::default());
        assert!(!paths.config_file().exists());
        let moved: Vec<String> = std::fs::read_dir(&paths.config_dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("config.toml.broken-"))
            .collect();
        assert_eq!(moved.len(), 1, "{moved:?}");
        let secs = &moved[0]["config.toml.broken-".len()..];
        assert!(secs.parse::<u64>().is_ok(), "{secs}");
    }

    #[test]
    fn wrong_types_count_as_broken() {
        let (_tmp, paths) = temp_paths();
        std::fs::write(paths.config_file(), "autostart = \"yes\"").expect("write");
        assert_eq!(Settings::load(&paths).expect("load"), Settings::default());
        assert!(!paths.config_file().exists());
    }

    #[test]
    fn save_preserves_comments_and_unknown_keys() {
        let (_tmp, paths) = temp_paths();
        let original = "# CaseControl settings\nautostart = false # flip me\nframe_rate = 30\nmystery = 1\n\n\
                        # per family\n[detectors]\nlogitech = true\n";
        std::fs::write(paths.config_file(), original).expect("write");
        let mut s = Settings::load(&paths).expect("load");
        s.autostart = true;
        s.detectors.insert("logitech".into(), false);
        s.save(&paths).expect("save");
        let text = std::fs::read_to_string(paths.config_file()).expect("read");
        assert!(
            text.starts_with(
                "# CaseControl settings\nautostart = true # flip me\nframe_rate = 30\nmystery = 1\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("# per family\n[detectors]\nlogitech = false\n"),
            "{text}"
        );
        assert_eq!(Settings::load(&paths).expect("reload"), s);
    }

    #[test]
    fn saving_unchanged_settings_keeps_file_identical() {
        let (_tmp, paths) = temp_paths();
        // Contains every key a default-ish Settings serializes to, so nothing is appended.
        let original = "# hand-written\nautostart = false\nstart_minimized = true   # aligned comment\n\
                        frame_rate = 0x1E\n\n[conflicts]\ncheck_openrgb_port = false # off\n\n[detectors]\n\n[devices]\n";
        std::fs::write(paths.config_file(), original).expect("write");
        Settings::load(&paths).expect("load").save(&paths).expect("save");
        assert_eq!(
            std::fs::read_to_string(paths.config_file()).expect("read"),
            original
        );
    }
}
