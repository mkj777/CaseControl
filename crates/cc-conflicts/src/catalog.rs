//! The embedded catalog of conflicting software and its validation.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use cc_core::family;

/// The catalog text compiled into the binary.
const EMBEDDED: &str = include_str!("../catalog.toml");

/// Id of the entry that represents Windows Dynamic Lighting; the registry scanner attaches its evidence here.
pub const DYNAMIC_LIGHTING_ENTRY: &str = "windows_dynamic_lighting";
/// Id of the entry that receives the OpenRGB SDK port evidence.
pub const OPENRGB_ENTRY: &str = "openrgb";
/// The value of `affects` that means "every device family".
pub const AFFECTS_ALL: &str = "all";

/// What kind of software an entry describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Multi-vendor RGB suite (SignalRGB, OpenRGB).
    RgbSuite,
    /// A hardware vendor's own RGB software.
    VendorRgb,
    /// Sensor / monitoring tool that reads SMBus, Super I/O or GPU I2C.
    HardwareMonitor,
    /// Fan control software.
    FanControl,
    /// A Windows feature.
    OsFeature,
    /// A kernel driver that stays loaded independently of any app.
    Driver,
}

/// How the start dialog treats an entry (ADR 0001 #31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// A full RGB suite (SignalRGB, OpenRGB, iCUE, Armoury Crate / Aura, ...). May appear in the start dialog when
    /// it affects one of our visible devices.
    Suite,
    /// Any other software (vendor tools such as G HUB, Wootility, MSI Center, the LampArray service, monitors,
    /// drivers). Listed on the Conflicts page only, never in the start dialog.
    VendorTool,
}

/// How the software reaches the hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// USB HID with a vendor protocol.
    Hid,
    /// SMBus / I2C through the chipset controller.
    Smbus,
    /// Super I/O or other port I/O.
    Superio,
    /// Ships or loads its own kernel driver.
    KernelDriver,
    /// Windows HID LampArray / Dynamic Lighting.
    Lamparray,
    /// GPU I2C bus.
    GpuI2c,
}

/// How serious a finding of this entry is. Ordered from least to most severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Present but cooperative; shown for completeness.
    Info,
    /// Unlikely to interfere with CaseControl's devices.
    Low,
    /// Can interfere in some setups.
    Medium,
    /// Known to fight over devices or to load a risky kernel driver.
    High,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        })
    }
}

/// One catalog entry. See the header of `catalog.toml` for the matching rules of each list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Stable snake_case id.
    pub id: String,
    /// Product name shown in the UI.
    pub name: String,
    /// Vendor name.
    pub vendor: String,
    /// Kind of software.
    pub category: Category,
    /// Suite or vendor tool; decides whether the entry can appear in the start dialog.
    pub class: Class,
    /// Executable names (case-insensitive, `*` wildcard). Also matched against the executable of autostart
    /// entries.
    #[serde(default)]
    pub processes: Vec<String>,
    /// Win32 service names (case-insensitive, `*` wildcard, also matched against the display name).
    #[serde(default)]
    pub services: Vec<String>,
    /// Kernel driver service names (same matching as `services`).
    #[serde(default)]
    pub drivers: Vec<String>,
    /// Case-insensitive substrings of an Uninstall key `DisplayName`.
    #[serde(default)]
    pub uninstall_names: Vec<String>,
    /// Directories that indicate an install, with `%VAR%` placeholders.
    #[serde(default)]
    pub install_paths: Vec<String>,
    /// Case-insensitive prefixes of Windows Dynamic Lighting provider names.
    #[serde(default)]
    pub lighting_providers: Vec<String>,
    /// Hardware access paths.
    pub access: Vec<Access>,
    /// `cc_core::family` ids this software competes for, or `"all"`.
    #[serde(default)]
    pub affects: Vec<String>,
    /// Severity of a finding.
    pub severity: Severity,
    /// `false` when the research could not confirm the listed names.
    pub verified: bool,
    /// One or two sentences for the UI.
    pub note: String,
}

impl Entry {
    /// Whether the entry has anything a scanner can match on.
    pub fn has_detection_key(&self) -> bool {
        !(self.processes.is_empty()
            && self.services.is_empty()
            && self.drivers.is_empty()
            && self.uninstall_names.is_empty()
            && self.install_paths.is_empty()
            && self.lighting_providers.is_empty())
            || self.id == DYNAMIC_LIGHTING_ENTRY
            || self.id == OPENRGB_ENTRY
    }
}

/// A validated set of entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    entry: Vec<Entry>,
}

/// Why a catalog failed to load.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// TOML syntax error, unknown field or unknown enum value.
    #[error("catalog parse error: {0}")]
    Parse(#[from] toml::de::Error),
    /// Two entries share an id.
    #[error("duplicate catalog id `{0}`")]
    DuplicateId(String),
    /// An id is empty or not snake_case.
    #[error("catalog id `{0}` is not snake_case")]
    InvalidId(String),
    /// An `affects` value is neither a family id nor `"all"`.
    #[error("entry `{entry}` affects unknown family `{family}`")]
    UnknownFamily {
        /// Entry id.
        entry: String,
        /// The offending value.
        family: String,
    },
    /// The entry cannot be detected by any scanner.
    #[error("entry `{0}` has no detection key")]
    NoDetectionKey(String),
    /// A required special entry is missing.
    #[error("required entry `{0}` is missing")]
    MissingEntry(&'static str),
    /// A match pattern is empty.
    #[error("entry `{0}` contains an empty pattern")]
    EmptyPattern(String),
}

impl Catalog {
    /// The catalog compiled into the binary.
    ///
    /// # Panics
    /// Never in a released build: the embedded text is validated by a unit test.
    pub fn embedded() -> Catalog {
        Self::parse(EMBEDDED).expect("embedded catalog.toml is validated by the test suite")
    }

    /// Parses and validates a catalog in the `catalog.toml` format.
    pub fn parse(text: &str) -> Result<Catalog, CatalogError> {
        let file: CatalogFile = toml::from_str(text)?;
        let catalog = Catalog { entries: file.entry };
        catalog.validate()?;
        Ok(catalog)
    }

    /// Builds a catalog from entries, validating them. Mostly useful for tests.
    pub fn from_entries(entries: Vec<Entry>) -> Result<Catalog, CatalogError> {
        let catalog = Catalog { entries };
        catalog.validate()?;
        Ok(catalog)
    }

    /// All entries in file order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Looks up an entry by id.
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    fn validate(&self) -> Result<(), CatalogError> {
        let mut seen = HashSet::new();
        for entry in &self.entries {
            let id_ok = !entry.id.is_empty()
                && entry
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                && entry.id.starts_with(|c: char| c.is_ascii_lowercase());
            if !id_ok {
                return Err(CatalogError::InvalidId(entry.id.clone()));
            }
            if !seen.insert(entry.id.as_str()) {
                return Err(CatalogError::DuplicateId(entry.id.clone()));
            }
            for value in &entry.affects {
                if value != AFFECTS_ALL && !family::ALL.contains(&value.as_str()) {
                    return Err(CatalogError::UnknownFamily {
                        entry: entry.id.clone(),
                        family: value.clone(),
                    });
                }
            }
            let patterns = entry
                .processes
                .iter()
                .chain(&entry.services)
                .chain(&entry.drivers)
                .chain(&entry.uninstall_names)
                .chain(&entry.install_paths)
                .chain(&entry.lighting_providers);
            if patterns.into_iter().any(|p| p.trim().is_empty()) {
                return Err(CatalogError::EmptyPattern(entry.id.clone()));
            }
            if !entry.has_detection_key() {
                return Err(CatalogError::NoDetectionKey(entry.id.clone()));
            }
        }
        for required in [DYNAMIC_LIGHTING_ENTRY, OPENRGB_ENTRY] {
            if !seen.contains(required) {
                return Err(CatalogError::MissingEntry(required));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_validates() {
        let catalog = Catalog::parse(EMBEDDED).expect("embedded catalog must validate");
        assert!(
            catalog.entries().len() >= 25,
            "only {} entries",
            catalog.entries().len()
        );
        for entry in catalog.entries() {
            assert!(entry.has_detection_key(), "{} has no detection key", entry.id);
            assert!(!entry.access.is_empty(), "{} has no access path", entry.id);
            assert!(!entry.note.is_empty(), "{} has no note", entry.id);
        }
    }

    #[test]
    fn embedded_catalog_has_required_entries() {
        let catalog = Catalog::embedded();
        for id in [
            "signalrgb",
            "openrgb",
            "inpoutx64",
            "pawnio",
            "fancontrol",
            "winring0",
            "windows_dynamic_lighting",
            "corsair_icue",
            "asus_armoury_crate",
            "msi_center",
            "logitech_g_hub",
        ] {
            assert!(catalog.get(id).is_some(), "missing {id}");
        }
        let pawnio = catalog.get("pawnio").expect("pawnio present");
        assert_eq!(pawnio.severity, Severity::Info);
        assert_eq!(pawnio.category, Category::Driver);
        let inpout = catalog.get("inpoutx64").expect("inpoutx64 present");
        assert_eq!(inpout.severity, Severity::High);
        let winring = catalog.get("winring0").expect("winring0 present");
        assert!(winring.drivers.iter().any(|d| d == "WinRing0x64"));
        assert!(winring.drivers.iter().any(|d| d == "WinRing0_1_2_0"));
        let dl = catalog
            .get(DYNAMIC_LIGHTING_ENTRY)
            .expect("dynamic lighting present");
        assert_eq!(dl.category, Category::OsFeature);
        let msi = catalog.get("msi_center").expect("msi present");
        assert!(!msi.verified);
    }

    #[test]
    fn suites_and_vendor_tools() {
        let catalog = Catalog::embedded();
        for id in [
            "signalrgb",
            "openrgb",
            "corsair_icue",
            "asus_armoury_crate",
            "asrock_polychrome",
            "gigabyte_rgb_fusion",
        ] {
            assert_eq!(catalog.get(id).map(|e| e.class), Some(Class::Suite), "{id}");
        }
        for id in [
            "logitech_g_hub",
            "msi_center",
            "windows_dynamic_lighting",
            "fancontrol",
            "inpoutx64",
        ] {
            assert_eq!(catalog.get(id).map(|e| e.class), Some(Class::VendorTool), "{id}");
        }
        let ghub = catalog.get("logitech_g_hub").expect("g hub");
        assert!(ghub.processes.iter().any(|p| p == "lghub_system_tray.exe"));
    }

    #[test]
    fn rejects_missing_class() {
        let text = with_required(&minimal("a").replace(
            "class = \"vendor_tool\"
",
            "",
        ));
        assert!(matches!(Catalog::parse(&text), Err(CatalogError::Parse(_))));
    }

    #[test]
    fn affects_ids_are_family_ids_or_all() {
        for entry in Catalog::embedded().entries() {
            for value in &entry.affects {
                assert!(
                    value == AFFECTS_ALL || family::ALL.contains(&value.as_str()),
                    "{value}"
                );
            }
        }
    }

    fn minimal(id: &str) -> String {
        format!(
            r#"
[[entry]]
id = "{id}"
name = "X"
vendor = "Y"
category = "driver"
class = "vendor_tool"
drivers = ["x"]
access = ["kernel_driver"]
affects = []
severity = "low"
verified = true
note = "n"
"#
        )
    }

    fn with_required(extra: &str) -> String {
        format!(
            "{}{}{}",
            minimal(DYNAMIC_LIGHTING_ENTRY),
            minimal(OPENRGB_ENTRY),
            extra
        )
    }

    #[test]
    fn rejects_duplicate_ids() {
        let text = with_required(&format!("{}{}", minimal("a"), minimal("a")));
        assert!(matches!(Catalog::parse(&text), Err(CatalogError::DuplicateId(id)) if id == "a"));
    }

    #[test]
    fn rejects_unknown_family() {
        let text = with_required(&minimal("a").replace("affects = []", "affects = [\"corsair\"]"));
        assert!(matches!(
            Catalog::parse(&text),
            Err(CatalogError::UnknownFamily { .. })
        ));
    }

    #[test]
    fn rejects_unknown_enum_value() {
        let text = with_required(&minimal("a").replace("severity = \"low\"", "severity = \"critical\""));
        assert!(matches!(Catalog::parse(&text), Err(CatalogError::Parse(_))));
        let text = with_required(&minimal("b").replace("[\"kernel_driver\"]", "[\"usb\"]"));
        assert!(matches!(Catalog::parse(&text), Err(CatalogError::Parse(_))));
    }

    #[test]
    fn rejects_entry_without_detection_key() {
        let text = with_required(&minimal("a").replace("drivers = [\"x\"]\n", ""));
        assert!(matches!(Catalog::parse(&text), Err(CatalogError::NoDetectionKey(id)) if id == "a"));
    }

    #[test]
    fn rejects_missing_required_entry() {
        let text = format!("{}{}", minimal(OPENRGB_ENTRY), minimal("a"));
        assert!(matches!(
            Catalog::parse(&text),
            Err(CatalogError::MissingEntry(DYNAMIC_LIGHTING_ENTRY))
        ));
    }

    #[test]
    fn rejects_bad_id() {
        let text = with_required(&minimal("Bad-Id"));
        assert!(matches!(Catalog::parse(&text), Err(CatalogError::InvalidId(_))));
    }
}
