//! Machine state remembered between runs (ADR 0001 #38): `%APPDATA%\CaseControl\hardware-state.toml`.
//!
//! Today it holds one thing: the "last good own lighting" of each device, driver-opaque bytes a controller reported
//! when it found the device showing its own lighting (`Controller::own_lighting`). The bytes are stored hex-encoded,
//! keyed by device id, so any driver can use the file; only the driver that wrote them interprets (and validates)
//! them.
//!
//! ```toml
//! # CaseControl machine state (ADR 0001 #38). Written by CaseControl; not meant to be edited.
//! [own_lighting]
//! "hid:1462:7d75:7D7522120106" = "52010a..."
//! ```
//!
//! The file is not user configuration and lives apart from `config.toml`: it is rewritten whole (atomically, like
//! the settings file) and only when a value changes. A file or entry that cannot be read is ignored with a warning;
//! unknown keys are dropped on the next write.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cc_core::DeviceId;

use crate::error::{ConfigError, Result};
use crate::fsutil::write_atomic;
use crate::paths::Paths;

/// The table that maps device ids to their last good own lighting.
const OWN_LIGHTING: &str = "own_lighting";
/// First line of every written file.
const HEADER: &str =
    "# CaseControl machine state (ADR 0001 #38). Written by CaseControl; not meant to be edited.\n";

/// The remembered machine state, optionally backed by a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HardwareState {
    /// Where changes are written; `None` keeps everything in memory (tests, or no storage this session).
    path: Option<PathBuf>,
    own_lighting: BTreeMap<DeviceId, Vec<u8>>,
}

impl HardwareState {
    /// An empty state that is never written anywhere.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Loads `hardware-state.toml` from `paths`; see [`load_from`](Self::load_from).
    pub fn load(paths: &Paths) -> Self {
        Self::load_from(paths.hardware_state_file())
    }

    /// Loads the state from `path` and writes later changes there. A missing file gives an empty state; a file that
    /// cannot be read or parsed, and entries that are not hex strings, are ignored with a warning (the next change
    /// rewrites the file).
    pub fn load_from(path: PathBuf) -> Self {
        let own_lighting = match std::fs::read_to_string(&path) {
            Ok(text) => parse(&path, &text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot read the hardware state file; ignoring it");
                BTreeMap::new()
            }
        };
        Self {
            path: Some(path),
            own_lighting,
        }
    }

    /// Every remembered own lighting, by device.
    pub fn own_lighting(&self) -> &BTreeMap<DeviceId, Vec<u8>> {
        &self.own_lighting
    }

    /// The remembered own lighting of `id`.
    pub fn get(&self, id: &DeviceId) -> Option<&[u8]> {
        self.own_lighting.get(id).map(Vec::as_slice)
    }

    /// Remembers `bytes` as the own lighting of `id`. Returns `Ok(false)` without writing when that is already what
    /// is remembered, else updates the state, writes the file (if the state has one) and returns `Ok(true)`. On a
    /// write error the new value is kept in memory and the error returned.
    pub fn record_own_lighting(&mut self, id: &DeviceId, bytes: &[u8]) -> Result<bool> {
        if self.get(id) == Some(bytes) {
            return Ok(false);
        }
        self.own_lighting.insert(id.clone(), bytes.to_vec());
        if let Some(path) = &self.path {
            write_atomic(path, &self.to_toml()?)?;
        }
        Ok(true)
    }

    fn to_toml(&self) -> Result<String> {
        let table: toml::Table = self
            .own_lighting
            .iter()
            .map(|(id, bytes)| (id.as_str().to_owned(), toml::Value::String(to_hex(bytes))))
            .collect();
        let mut root = toml::Table::new();
        root.insert(OWN_LIGHTING.to_owned(), toml::Value::Table(table));
        let body = toml::to_string(&root).map_err(|e| ConfigError::Serialize(e.to_string()))?;
        Ok(format!("{HEADER}{body}"))
    }
}

fn parse(path: &Path, text: &str) -> BTreeMap<DeviceId, Vec<u8>> {
    let root: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "hardware state file is not valid TOML; ignoring it");
            return BTreeMap::new();
        }
    };
    let Some(entries) = root.get(OWN_LIGHTING) else {
        return BTreeMap::new();
    };
    let Some(entries) = entries.as_table() else {
        tracing::warn!(path = %path.display(), "hardware state: own_lighting is not a table; ignoring it");
        return BTreeMap::new();
    };
    let entries: BTreeMap<DeviceId, toml::Value> = match toml::Value::Table(entries.clone()).try_into() {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "hardware state: own_lighting is unreadable; ignoring it");
            return BTreeMap::new();
        }
    };
    let mut out = BTreeMap::new();
    for (id, value) in entries {
        match value.as_str().and_then(from_hex) {
            Some(bytes) if !id.as_str().trim().is_empty() => {
                out.insert(id, bytes);
            }
            _ => {
                tracing::warn!(path = %path.display(), %id, "hardware state: entry is not a hex string; ignoring it");
            }
        }
    }
    out
}

/// Lower-case hex, two digits per byte, no separators.
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Parses [`to_hex`] output (either case). `None` for an empty string, an odd length or a non-hex digit.
fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> DeviceId {
        DeviceId::hid(0x1462, 0x7D75, Some("7D7522120106"))
    }

    #[test]
    fn hex_round_trip_and_rejects() {
        assert_eq!(to_hex(&[0x52, 0x00, 0xAB]), "5200ab");
        assert_eq!(from_hex("5200AB"), Some(vec![0x52, 0x00, 0xAB]));
        assert_eq!(from_hex(""), None);
        assert_eq!(from_hex("520"), None);
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("é1"), None);
    }

    #[test]
    fn missing_file_is_empty_and_round_trips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::at(tmp.path());
        let mut state = HardwareState::load(&paths);
        assert!(state.own_lighting().is_empty());
        assert!(!paths.hardware_state_file().exists());

        let blob: Vec<u8> = (0..185u8).collect();
        assert!(state.record_own_lighting(&id(), &blob).expect("written"));
        let text = std::fs::read_to_string(paths.hardware_state_file()).expect("read");
        assert!(text.starts_with(HEADER), "{text}");
        assert!(text.contains("[own_lighting]"), "{text}");
        assert!(
            text.contains("\"hid:1462:7d75:7D7522120106\" = \"000102"),
            "{text}"
        );

        let back = HardwareState::load(&paths);
        assert_eq!(back.get(&id()), Some(blob.as_slice()));
        assert_eq!(back, state);
    }

    #[test]
    fn unchanged_value_is_not_rewritten() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::at(tmp.path());
        let mut state = HardwareState::load(&paths);
        assert!(state.record_own_lighting(&id(), &[1, 2, 3]).expect("first"));
        // Replace the file behind the state's back: an unchanged record must not touch it.
        std::fs::write(paths.hardware_state_file(), "marker").expect("write");
        assert!(!state.record_own_lighting(&id(), &[1, 2, 3]).expect("same"));
        assert_eq!(
            std::fs::read_to_string(paths.hardware_state_file()).expect("read"),
            "marker"
        );
        assert!(state.record_own_lighting(&id(), &[1, 2, 4]).expect("changed"));
        assert_eq!(HardwareState::load(&paths).get(&id()), Some(&[1u8, 2, 4][..]));
    }

    #[test]
    fn garbage_is_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::at(tmp.path());
        let file = paths.hardware_state_file();

        std::fs::write(&file, "this is [ not toml").expect("write");
        assert!(HardwareState::load(&paths).own_lighting().is_empty());

        std::fs::write(&file, "own_lighting = 5\n").expect("write");
        assert!(HardwareState::load(&paths).own_lighting().is_empty());

        std::fs::write(
            &file,
            "future = true\n[own_lighting]\n\"hid:0001:0002\" = \"0a0b\"\n\"hid:0001:0003\" = \"xyz\"\n\
             \"hid:0001:0004\" = 7\n\"hid:0001:0005\" = \"\"\n",
        )
        .expect("write");
        let state = HardwareState::load(&paths);
        assert_eq!(
            state
                .own_lighting()
                .keys()
                .map(DeviceId::as_str)
                .collect::<Vec<_>>(),
            ["hid:0001:0002"]
        );
        assert_eq!(state.get(&DeviceId::hid(1, 2, None)), Some(&[0x0A_u8, 0x0B][..]));

        // A record rewrites the file cleanly, dropping what could not be read.
        let mut state = state;
        assert!(state.record_own_lighting(&id(), &[9]).expect("written"));
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(!text.contains("future") && !text.contains("xyz"), "{text}");
        assert_eq!(HardwareState::load(&paths).own_lighting().len(), 2);
    }

    #[test]
    fn in_memory_state_writes_nothing() {
        let mut state = HardwareState::in_memory();
        assert!(state.record_own_lighting(&id(), &[1]).expect("recorded"));
        assert!(!state.record_own_lighting(&id(), &[1]).expect("unchanged"));
        assert_eq!(state.get(&id()), Some(&[1u8][..]));
    }

    #[test]
    fn write_error_keeps_the_value_in_memory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // The parent "directory" is a file, so the write fails.
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "x").expect("write");
        let mut state = HardwareState::load_from(blocker.join("hardware-state.toml"));
        assert!(state.record_own_lighting(&id(), &[1]).is_err());
        assert_eq!(state.get(&id()), Some(&[1u8][..]));
    }
}
