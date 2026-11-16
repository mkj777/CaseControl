//! The Settings page's view of `cc_config::Settings`, the patch the page sends back, and the pure functions that
//! apply both (plus zone-size persistence and the engine configuration derived from settings).

use std::collections::BTreeMap;

use cc_config::{FRAME_RATE_RANGE, Settings};
use cc_core::{DeviceId, family};
use cc_engine::EngineConfig;
use serde::{Deserialize, Serialize};

/// What the Settings page shows. Every known device family appears in `detectors`, enabled unless switched off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsView {
    /// Start with Windows (HKCU Run key via `tauri-plugin-autostart`).
    pub autostart: bool,
    /// Start hidden in the tray.
    pub start_minimized: bool,
    /// Maximum colour frames per second and device, `1..=60`.
    pub frame_rate: u32,
    /// Detector switch per family id, in `cc_core::family::ALL` order (serialized as an object).
    pub detectors: BTreeMap<String, bool>,
    /// Probe the OpenRGB SDK port during conflict scans.
    pub check_openrgb_port: bool,
    /// Profile applied last (re-applied on start).
    pub last_profile: Option<String>,
    /// Why settings are not saved this session, if they are not (shown as a notice).
    pub save_error: Option<String>,
}

impl SettingsView {
    /// The view of `settings`; `save_error` says why changes cannot be saved, if they cannot.
    pub fn of(settings: &Settings, save_error: Option<String>) -> Self {
        Self {
            autostart: settings.autostart,
            start_minimized: settings.start_minimized,
            frame_rate: settings.frame_rate,
            detectors: family::ALL
                .iter()
                .map(|f| ((*f).to_owned(), settings.is_enabled(f)))
                .collect(),
            check_openrgb_port: settings.conflicts.check_openrgb_port,
            last_profile: settings.last_profile.clone(),
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
    /// New frame rate (clamped to `1..=60`).
    pub frame_rate: Option<u32>,
    /// Detector switches to change, by family id. Unknown family ids are rejected.
    pub detectors: Option<BTreeMap<String, bool>>,
    /// New OpenRGB port check flag.
    pub check_openrgb_port: Option<bool>,
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

/// The engine configuration for `settings`: frame rate, disabled families, remembered zone sizes, hot-plug on.
pub fn engine_config(settings: &Settings) -> EngineConfig {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msi() -> DeviceId {
        DeviceId::hid(0x1462, 0x7D75, None)
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
        let cfg = engine_config(&s);
        assert_eq!(cfg.frame_rate, 45);
        assert!(cfg.hotplug);
        assert_eq!(
            cfg.disabled_families,
            std::collections::HashSet::from([family::LOGITECH.to_owned()])
        );
        assert_eq!(cfg.zone_sizes.len(), 1, "devices without zone sizes are left out");
        assert_eq!(cfg.zone_sizes[&msi()]["JRAINBOW1"], 60);
    }
}
