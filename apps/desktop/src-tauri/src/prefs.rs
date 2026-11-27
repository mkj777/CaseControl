//! The Settings page's view of `cc_config::Settings`, the patch the page sends back, and the pure functions that
//! apply both (plus zone-size and brightness persistence and the engine configuration derived from settings).
//!
//! Device brightness (ADR 0001 #33, #40) is part of the active profile and saved there with every change. Versions
//! before ADR 0001 #40 kept changes since the last profile apply or save per device in the settings
//! (`cc_config::DeviceOverrides::brightness`); on start those are laid over the active profile's map
//! ([`start_brightness`]), saved into it and cleared ([`clear_brightness_overrides`]).
//!
//! The switches of ADR 0001 #42 ("Keep colours when signing out", "Save colours to the devices", "Apply colours
//! before sign-in") are fields of `cc_config::Settings` ([`PowerSettings`] reads them together); `cc-config` writes
//! each only when it differs from its default and migrates the loose top-level keys earlier versions kept.

use std::collections::BTreeMap;

use cc_config::{DeviceOverrides, FRAME_RATE_RANGE, FULL_BRIGHTNESS, HardwareState, Profile, Settings};
use cc_core::{DeviceId, Rgb, family};
use cc_engine::{DeviceSetting, EngineConfig};
use serde::{Deserialize, Serialize};

/// What the Settings page shows. Every known device family appears in `detectors`, enabled unless switched off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsView {
    /// Start with Windows (a per-user Task Scheduler logon task, see [`crate::autostart`]).
    pub autostart: bool,
    /// Start hidden in the tray.
    pub start_minimized: bool,
    /// "Apply colours before sign-in": the start-up task exists (ADR 0001 #42). Changed only by
    /// `set_apply_before_sign_in`.
    pub apply_before_sign_in: bool,
    /// "Keep colours when signing out" (ADR 0001 #42, on by default).
    pub keep_colors_on_sign_out: bool,
    /// "Save colours to the devices" (ADR 0001 #42). Changed only by `set_save_to_devices`.
    pub save_to_devices: bool,
    /// The one short line under "Save colours to the devices" after it was last switched this session: the devices
    /// that were not saved (or not restored) and why. `None` when there is nothing to say.
    pub save_to_devices_note: Option<String>,
    /// Maximum colour frames per second and device, `1..=60`.
    pub frame_rate: u32,
    /// Detector switch per family id, in `cc_core::family::ALL` order (serialized as an object).
    pub detectors: BTreeMap<String, bool>,
    /// Probe the OpenRGB SDK port during conflict scans.
    pub check_openrgb_port: bool,
    /// Profile applied last (re-applied on start).
    pub last_profile: Option<String>,
    /// The colour a takeover shows on a device with neither a stored setting nor a hardware readback (ADR 0001 #36).
    pub default_color: Rgb,
    /// Why settings are not saved this session, if they are not (shown as a notice).
    pub save_error: Option<String>,
}

impl SettingsView {
    /// The view of `settings`; `save_error` says why changes cannot be saved, if they cannot.
    pub fn of(settings: &Settings, save_error: Option<String>) -> Self {
        let power = PowerSettings::of(settings);
        Self {
            autostart: settings.autostart,
            start_minimized: settings.start_minimized,
            apply_before_sign_in: power.apply_before_sign_in,
            keep_colors_on_sign_out: power.keep_colors_on_sign_out,
            save_to_devices: power.save_to_devices,
            save_to_devices_note: None,
            frame_rate: settings.frame_rate,
            detectors: family::ALL
                .iter()
                .map(|f| ((*f).to_owned(), settings.is_enabled(f)))
                .collect(),
            check_openrgb_port: settings.conflicts.check_openrgb_port,
            last_profile: settings.last_profile.clone(),
            default_color: settings.default_color,
            save_error,
        }
    }
}

/// A partial update from the Settings page; absent fields stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SettingsPatch {
    /// New autostart intent.
    pub autostart: Option<bool>,
    /// New start-minimized flag.
    pub start_minimized: Option<bool>,
    /// New "Keep colours when signing out" flag.
    pub keep_colors_on_sign_out: Option<bool>,
    /// New frame rate (clamped to `1..=60`).
    pub frame_rate: Option<u32>,
    /// Detector switches to change, by family id. Unknown family ids are rejected.
    pub detectors: Option<BTreeMap<String, bool>>,
    /// New OpenRGB port check flag.
    pub check_openrgb_port: Option<bool>,
    /// New takeover default colour.
    pub default_color: Option<Rgb>,
}

/// Which parts of the running app must follow a patch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchEffects {
    /// The engine's frame rate changed.
    pub frame_rate: Option<u32>,
    /// The set of disabled families changed.
    pub detectors_changed: bool,
    /// A family went from disabled to enabled, so a rescan can pick its devices up.
    pub family_enabled: bool,
    /// The autostart intent changed to this value.
    pub autostart: Option<bool>,
    /// The takeover default colour changed to this value.
    pub default_color: Option<Rgb>,
}

/// Applies `patch` to `settings` and reports what changed. Fails without touching `settings` if the patch names an
/// unknown family.
pub fn apply_patch(settings: &mut Settings, patch: &SettingsPatch) -> Result<PatchEffects, String> {
    if let Some(detectors) = &patch.detectors
        && let Some(unknown) = detectors.keys().find(|f| !family::ALL.contains(&f.as_str()))
    {
        return Err(format!("unknown device family {unknown:?}"));
    }
    let mut effects = PatchEffects::default();
    if let Some(on) = patch.autostart
        && on != settings.autostart
    {
        settings.autostart = on;
        effects.autostart = Some(on);
    }
    if let Some(on) = patch.start_minimized {
        settings.start_minimized = on;
    }
    if let Some(on) = patch.keep_colors_on_sign_out {
        settings.keep_colors_on_sign_out = on;
    }
    if let Some(fps) = patch.frame_rate {
        let fps = fps.clamp(*FRAME_RATE_RANGE.start(), *FRAME_RATE_RANGE.end());
        if fps != settings.frame_rate {
            settings.frame_rate = fps;
            effects.frame_rate = Some(fps);
        }
    }
    if let Some(detectors) = &patch.detectors {
        for (fam, &on) in detectors {
            let was = settings.is_enabled(fam);
            if was != on {
                effects.detectors_changed = true;
                effects.family_enabled |= on;
            }
            settings.detectors.insert(fam.clone(), on);
        }
    }
    if let Some(on) = patch.check_openrgb_port {
        settings.conflicts.check_openrgb_port = on;
    }
    if let Some(color) = patch.default_color
        && color != settings.default_color
    {
        settings.default_color = color;
        effects.default_color = Some(color);
    }
    Ok(effects)
}

/// Remembers that zone `zone_name` of device `id` has `len` LEDs, so the size is restored on the next start.
pub fn record_zone_size(settings: &mut Settings, id: &DeviceId, zone_name: &str, len: usize) {
    settings
        .devices
        .entry(id.clone())
        .or_default()
        .zone_sizes
        .insert(zone_name.to_owned(), len);
}

/// Forgets every per-device brightness change an older version kept (they were saved into the active profile).
/// Device entries left with nothing in them are removed. Returns whether anything changed.
pub fn clear_brightness_overrides(settings: &mut Settings) -> bool {
    let mut changed = false;
    for ov in settings.devices.values_mut() {
        changed |= ov.brightness.take().is_some();
    }
    settings.devices.retain(|_, ov| *ov != DeviceOverrides::default());
    changed
}

/// Device brightness at start: the map of `last_profile` (if it loaded) overlaid with the per-device changes an older
/// version kept in the settings. Devices in neither have 100.
pub fn start_brightness(last_profile: Option<&Profile>, settings: &Settings) -> BTreeMap<DeviceId, u8> {
    let mut map = last_profile.map(|p| p.brightness.clone()).unwrap_or_default();
    for (id, ov) in &settings.devices {
        if let Some(b) = ov.brightness {
            map.insert(id.clone(), b);
        }
    }
    map
}

/// The engine configuration for `settings` and the active profile (ADR 0001 #42: early apply): everything of
/// [`engine_config`], the device brightness of `profile` with the per-device changes an older version kept laid over
/// it ([`start_brightness`]), and as initial settings the visible part of `profile` ([`initial_settings`]), so every
/// device gets its setting the moment it is found.
pub fn engine_config_for(
    settings: &Settings,
    hardware_state: HardwareState,
    profile: Option<&Profile>,
) -> EngineConfig {
    EngineConfig {
        brightness: start_brightness(profile, settings),
        initial_settings: profile.map(|p| initial_settings(settings, p)).unwrap_or_default(),
        ..engine_config(settings, hardware_state)
    }
}

/// The settings the engine seeds before detection (ADR 0001 #42): `profile`'s device settings without the hidden
/// devices. Hidden zones are not masked here (no device is described yet); the engine's dark zones keep them black
/// in every direct output.
pub fn initial_settings(settings: &Settings, profile: &Profile) -> BTreeMap<DeviceId, DeviceSetting> {
    crate::hidden::visible_profile(settings, &profile.devices, |_| None).0
}

/// The engine configuration for `settings` without an active profile: frame rate, disabled families, remembered
/// zone sizes, master brightness, the per-device brightness changes an older version kept, the hidden devices (never
/// taken over by brightness changes), the takeover default colour and the hidden zones it leaves black, hot-plug and
/// resume handling on, no initial settings. `hardware_state` is the machine state (`hardware-state.toml`, ADR 0001
/// #38), which is not part of the settings.
pub fn engine_config(settings: &Settings, hardware_state: HardwareState) -> EngineConfig {
    EngineConfig {
        frame_rate: settings
            .frame_rate
            .clamp(*FRAME_RATE_RANGE.start(), *FRAME_RATE_RANGE.end()),
        disabled_families: settings.disabled_families(),
        zone_sizes: settings
            .devices
            .iter()
            .filter(|(_, ov)| !ov.zone_sizes.is_empty())
            .map(|(id, ov)| (id.clone(), ov.zone_sizes.clone()))
            .collect(),
        hotplug: true,
        resume: true,
        master_brightness: settings.master_brightness.min(FULL_BRIGHTNESS),
        brightness: start_brightness(None, settings),
        excluded: crate::hidden::hidden_devices(settings),
        default_color: settings.default_color,
        dark_zones: crate::hidden::hidden_zone_map(settings),
        hardware_state,
        initial_settings: BTreeMap::new(),
        device_memory_writes: true,
    }
}

/// The device family a device id belongs to, from its transport and vendor id (`nvapi:` NVIDIA; `hid:` MSI 1462 or
/// 0DB0, Logitech 046D, Wooting 31E3); `None` for any other id. Used where no detection has run yet (the
/// start-up task's wait, the uninstaller's clean-up).
pub fn family_of(id: &DeviceId) -> Option<&'static str> {
    let text = id.as_str().to_ascii_lowercase();
    if text.starts_with("nvapi:") {
        return Some(family::NVIDIA_ILLUMINATION);
    }
    let vendor = text.strip_prefix("hid:")?.split(':').next()?;
    match vendor {
        "1462" | "0db0" => Some(family::MSI_MYSTIC_LIGHT),
        "046d" => Some(family::LOGITECH),
        "31e3" => Some(family::WOOTING),
        _ => None,
    }
}

/// The switches of ADR 0001 #42 as stored in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerSettings {
    /// Keep CaseControl's colours when the user signs out (default on).
    pub keep_colors_on_sign_out: bool,
    /// Save the colours to the devices that can store them (default off).
    pub save_to_devices: bool,
    /// The start-up task that applies the colours before sign-in exists (default off).
    pub apply_before_sign_in: bool,
}

impl PowerSettings {
    /// The switches in `settings`.
    pub fn of(settings: &Settings) -> Self {
        Self {
            keep_colors_on_sign_out: settings.keep_colors_on_sign_out,
            save_to_devices: settings.save_to_devices,
            apply_before_sign_in: settings.apply_before_sign_in,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msi() -> DeviceId {
        DeviceId::hid(0x1462, 0x7D75, None)
    }

    /// A device brightness change as versions before ADR 0001 #40 kept it in the settings.
    fn record_brightness(settings: &mut Settings, id: &DeviceId, percent: u8) {
        settings.devices.entry(id.clone()).or_default().brightness = Some(percent.min(FULL_BRIGHTNESS));
    }

    #[test]
    fn view_lists_every_family() {
        let mut s = Settings::default();
        s.detectors.insert(family::LOGITECH.into(), false);
        let v = SettingsView::of(&s, None);
        assert_eq!(v.detectors.len(), family::ALL.len());
        assert!(!v.detectors[family::LOGITECH]);
        assert!(v.detectors[family::WOOTING]);
        assert_eq!(v.frame_rate, 30);
    }

    #[test]
    fn view_serializes_to_frontend_shape() {
        let json = serde_json::to_value(SettingsView::of(&Settings::default(), Some("x".into())))
            .expect("serializes");
        assert_eq!(json["autostart"], serde_json::json!(false));
        assert_eq!(json["frame_rate"], serde_json::json!(30));
        assert_eq!(json["detectors"]["wooting"], serde_json::json!(true));
        assert_eq!(json["last_profile"], serde_json::Value::Null);
        assert_eq!(json["default_color"], serde_json::json!("#FFFFFF"));
        assert_eq!(json["save_error"], serde_json::json!("x"));
    }

    #[test]
    fn patch_deserializes_partially() {
        let p: SettingsPatch =
            serde_json::from_value(serde_json::json!({ "frame_rate": 45 })).expect("parses");
        assert_eq!(
            p,
            SettingsPatch {
                frame_rate: Some(45),
                ..SettingsPatch::default()
            }
        );
    }

    #[test]
    fn default_color_patch_changes_settings_and_reports_only_a_change() {
        let mut s = Settings::default();
        let p: SettingsPatch =
            serde_json::from_value(serde_json::json!({ "default_color": "#ff8800" })).expect("parses");
        let orange = Rgb::new(0xFF, 0x88, 0);
        assert_eq!(p.default_color, Some(orange));
        let fx = apply_patch(&mut s, &p).expect("valid patch");
        assert_eq!(fx.default_color, Some(orange));
        assert_eq!(s.default_color, orange);
        assert_eq!(SettingsView::of(&s, None).default_color, orange);
        assert_eq!(
            engine_config(&s, HardwareState::in_memory()).default_color,
            orange
        );
        let fx = apply_patch(&mut s, &p).expect("valid patch");
        assert_eq!(fx.default_color, None, "unchanged colour is not an effect");
        assert!(
            serde_json::from_value::<SettingsPatch>(serde_json::json!({ "default_color": "red" })).is_err()
        );
    }

    #[test]
    fn patch_reports_effects() {
        let mut s = Settings::default();
        let fx = apply_patch(
            &mut s,
            &SettingsPatch {
                autostart: Some(true),
                frame_rate: Some(200),
                detectors: Some(BTreeMap::from([(family::WOOTING.into(), false)])),
                check_openrgb_port: Some(true),
                start_minimized: Some(true),
                keep_colors_on_sign_out: None,
                default_color: None,
            },
        )
        .expect("valid patch");
        assert_eq!(fx.autostart, Some(true));
        assert_eq!(fx.frame_rate, Some(60));
        assert!(fx.detectors_changed);
        assert!(!fx.family_enabled);
        assert!(s.autostart && s.start_minimized && s.conflicts.check_openrgb_port);
        assert_eq!(s.frame_rate, 60);
        assert!(!s.is_enabled(family::WOOTING));

        let fx = apply_patch(
            &mut s,
            &SettingsPatch {
                autostart: Some(true),
                frame_rate: Some(60),
                detectors: Some(BTreeMap::from([(family::WOOTING.into(), true)])),
                ..SettingsPatch::default()
            },
        )
        .expect("valid patch");
        assert_eq!(fx.autostart, None, "unchanged autostart is not an effect");
        assert_eq!(fx.frame_rate, None);
        assert!(fx.detectors_changed && fx.family_enabled);
    }

    #[test]
    fn patch_rejects_unknown_family() {
        let mut s = Settings::default();
        let before = s.clone();
        let err = apply_patch(
            &mut s,
            &SettingsPatch {
                frame_rate: Some(10),
                detectors: Some(BTreeMap::from([("razer".into(), false)])),
                ..SettingsPatch::default()
            },
        );
        assert!(err.is_err());
        assert_eq!(s, before);
    }

    #[test]
    fn zone_sizes_are_recorded_by_name() {
        let mut s = Settings::default();
        record_zone_size(&mut s, &msi(), "JRAINBOW1", 60);
        record_zone_size(&mut s, &msi(), "JRAINBOW1", 30);
        record_zone_size(&mut s, &msi(), "JRAINBOW2", 8);
        let zones = &s.devices[&msi()].zone_sizes;
        assert_eq!(zones["JRAINBOW1"], 30);
        assert_eq!(zones["JRAINBOW2"], 8);
    }

    #[test]
    fn engine_config_follows_settings() {
        let mut s = Settings {
            frame_rate: 45,
            ..Settings::default()
        };
        s.detectors.insert(family::LOGITECH.into(), false);
        s.detectors.insert(family::WOOTING.into(), true);
        record_zone_size(&mut s, &msi(), "JRAINBOW1", 60);
        s.devices
            .insert(DeviceId::hid(0x31E3, 0x1310, None), Default::default());
        let cfg = engine_config(&s, HardwareState::in_memory());
        assert_eq!(cfg.frame_rate, 45);
        assert!(cfg.hotplug);
        assert!(
            cfg.device_memory_writes,
            "the signed-in app may save to the devices"
        );
        assert_eq!(
            cfg.disabled_families,
            std::collections::HashSet::from([family::LOGITECH.to_owned()])
        );
        assert_eq!(cfg.zone_sizes.len(), 1, "devices without zone sizes are left out");
        assert_eq!(cfg.zone_sizes[&msi()]["JRAINBOW1"], 60);
        assert_eq!(cfg.master_brightness, 100);
        assert!(cfg.brightness.is_empty());

        s.master_brightness = 40;
        record_brightness(&mut s, &msi(), 70);
        let cfg = engine_config(&s, HardwareState::in_memory());
        assert_eq!(cfg.master_brightness, 40);
        assert_eq!(cfg.brightness, BTreeMap::from([(msi(), 70)]));
    }

    #[test]
    fn start_brightness_overlays_changes_on_the_last_profile() {
        let other = DeviceId::hid(0x31E3, 0x1310, None);
        let third = DeviceId::hid(0x046D, 0xC33C, None);
        let profile = Profile {
            name: "Evening".into(),
            devices: BTreeMap::new(),
            brightness: BTreeMap::from([(msi(), 20), (other.clone(), 60)]),
        };
        let mut s = Settings::default();
        record_brightness(&mut s, &other, 90);
        record_brightness(&mut s, &third, 150);
        assert_eq!(
            start_brightness(Some(&profile), &s),
            BTreeMap::from([(msi(), 20), (other.clone(), 90), (third.clone(), 100)])
        );
        assert_eq!(
            start_brightness(None, &s),
            BTreeMap::from([(other, 90), (third, 100)])
        );
    }

    #[test]
    fn clearing_brightness_keeps_other_overrides_and_drops_empty_entries() {
        let mut s = Settings::default();
        record_zone_size(&mut s, &msi(), "JRAINBOW1", 60);
        record_brightness(&mut s, &msi(), 50);
        let lamp = DeviceId::hid(0x31E3, 0x1310, None);
        record_brightness(&mut s, &lamp, 10);
        assert!(clear_brightness_overrides(&mut s));
        assert_eq!(s.devices.len(), 1, "the brightness-only entry is gone");
        assert_eq!(s.devices[&msi()].brightness, None);
        assert_eq!(s.devices[&msi()].zone_sizes["JRAINBOW1"], 60);
        assert!(!clear_brightness_overrides(&mut s), "nothing left to clear");
    }

    #[test]
    fn power_switches_default_and_read_the_settings_fields() {
        let mut s = Settings::default();
        assert_eq!(
            PowerSettings::of(&s),
            PowerSettings {
                keep_colors_on_sign_out: true,
                save_to_devices: false,
                apply_before_sign_in: false
            }
        );
        s.save_to_devices = true;
        s.apply_before_sign_in = true;
        s.keep_colors_on_sign_out = false;
        let power = PowerSettings::of(&s);
        assert!(power.save_to_devices && power.apply_before_sign_in && !power.keep_colors_on_sign_out);
        assert!(s.extra.is_empty(), "no loose keys any more");
    }

    #[test]
    fn power_switches_written_by_an_earlier_version_are_read() {
        // Earlier builds kept the switches as loose top-level keys; they load into the fields (ADR 0001 #42, E9).
        let root = std::env::temp_dir().join(format!("cc-prefs-power-{}", std::process::id()));
        let paths = cc_config::Paths::at(&root);
        paths.ensure().expect("dirs");
        std::fs::write(
            paths.config_file(),
            "frame_rate = 45
save_to_devices = true
keep_colors_on_sign_out = false
",
        )
        .expect("write");
        let loaded = Settings::load(&paths).expect("loads");
        let power = PowerSettings::of(&loaded);
        assert!(power.save_to_devices && !power.keep_colors_on_sign_out && !power.apply_before_sign_in);
        assert_eq!(loaded.frame_rate, 45);
        // Back to the defaults: saving removes the keys from the file.
        let mut back = loaded;
        back.save_to_devices = false;
        back.keep_colors_on_sign_out = true;
        back.save(&paths).expect("saves");
        let text = std::fs::read_to_string(paths.config_file()).expect("reads");
        assert!(
            !text.contains("save_to_devices") && !text.contains("keep_colors_on_sign_out"),
            "{text}"
        );
        assert_eq!(Settings::load(&paths).expect("loads"), back);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn keep_colors_patch_changes_only_that_switch() {
        let mut s = Settings::default();
        let p: SettingsPatch =
            serde_json::from_value(serde_json::json!({ "keep_colors_on_sign_out": false })).expect("parses");
        assert_eq!(p.keep_colors_on_sign_out, Some(false));
        let fx = apply_patch(&mut s, &p).expect("valid patch");
        assert_eq!(
            fx,
            PatchEffects::default(),
            "nothing in the running app follows at once"
        );
        assert!(!PowerSettings::of(&s).keep_colors_on_sign_out);
        assert!(!SettingsView::of(&s, None).keep_colors_on_sign_out);
        // The other two switches cannot be patched (they have their own commands).
        let p: SettingsPatch = serde_json::from_value(
            serde_json::json!({ "save_to_devices": true, "apply_before_sign_in": true }),
        )
        .expect("unknown keys are ignored");
        apply_patch(&mut s, &p).expect("valid patch");
        let power = PowerSettings::of(&s);
        assert!(!power.save_to_devices && !power.apply_before_sign_in);
    }

    #[test]
    fn view_carries_the_power_switches() {
        let s = Settings {
            save_to_devices: true,
            ..Settings::default()
        };
        let json = serde_json::to_value(SettingsView {
            save_to_devices_note: Some("Not saved: G560 (original state unknown).".into()),
            ..SettingsView::of(&s, None)
        })
        .expect("serializes");
        assert_eq!(json["keep_colors_on_sign_out"], serde_json::json!(true));
        assert_eq!(json["save_to_devices"], serde_json::json!(true));
        assert_eq!(json["apply_before_sign_in"], serde_json::json!(false));
        assert_eq!(
            json["save_to_devices_note"],
            serde_json::json!("Not saved: G560 (original state unknown).")
        );
        assert_eq!(
            serde_json::to_value(SettingsView::of(&s, None)).expect("serializes")["save_to_devices_note"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn device_ids_map_to_their_family() {
        assert_eq!(family_of(&msi()), Some(family::MSI_MYSTIC_LIGHT));
        assert_eq!(
            family_of(&DeviceId::hid(0x0DB0, 0x0076, None)),
            Some(family::MSI_MYSTIC_LIGHT)
        );
        assert_eq!(
            family_of(&DeviceId::hid(0x046D, 0x0A78, Some("abc"))),
            Some(family::LOGITECH)
        );
        assert_eq!(
            family_of(&DeviceId::hid(0x31E3, 0x1310, None)),
            Some(family::WOOTING)
        );
        assert_eq!(
            family_of(&DeviceId::nvapi(0x2702_10DE, 0x1612_10DE)),
            Some(family::NVIDIA_ILLUMINATION)
        );
        assert_eq!(family_of(&DeviceId::hid(0x1234, 0x5678, None)), None);
    }

    #[test]
    fn engine_starts_with_the_visible_active_profile() {
        let lamp = DeviceId::hid(0x31E3, 0x1310, None);
        let red = DeviceSetting::Direct {
            colors: vec![Rgb::new(255, 0, 0)],
        };
        let profile = Profile {
            name: "Evening".into(),
            devices: BTreeMap::from([(msi(), red.clone()), (lamp.clone(), red.clone())]),
            brightness: BTreeMap::from([(msi(), 30)]),
        };
        let mut s = Settings::default();
        crate::hidden::set_device_hidden(&mut s, &lamp, true);
        crate::hidden::set_zone_hidden(&mut s, &msi(), "JRAINBOW1", true);
        let cfg = engine_config_for(&s, HardwareState::in_memory(), Some(&profile));
        assert_eq!(
            cfg.initial_settings,
            BTreeMap::from([(msi(), red)]),
            "hidden devices are left out; hidden zones stay to the engine's dark zones"
        );
        assert_eq!(cfg.brightness, BTreeMap::from([(msi(), 30)]));
        assert!(cfg.dark_zones[&msi()].contains("JRAINBOW1"));
        assert!(cfg.excluded.contains(&lamp));
        // Without a profile nothing is seeded.
        let cfg = engine_config_for(&s, HardwareState::in_memory(), None);
        assert!(cfg.initial_settings.is_empty() && cfg.brightness.is_empty());
    }
}
