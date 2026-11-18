//! Machine state remembered between runs (ADR 0001 #38): `%APPDATA%\CaseControl\hardware-state.toml`.
//!
//! It holds two things per device id:
//!
//! - the "last good own lighting": driver-opaque bytes a controller reported when it found the device showing its own
//!   lighting (`Controller::own_lighting`), stored hex-encoded so any driver can use the file; only the driver that
//!   wrote them interprets (and validates) them;
//! - the "unreleased" marker: CaseControl has written to the device and not yet handed it back. Set before the first
//!   write after a takeover, cleared after a successful release. A device that is still marked at the next open was
//!   left in a state CaseControl wrote (crash, sleep, re-enumeration), so its readback is not its own lighting.
//!
//! ```toml
//! # CaseControl machine state (ADR 0001 #38). Written by CaseControl; not meant to be edited.
//! unreleased = ["hid:1462:7d75:7D7522120106"]
//!
//! [own_lighting]
//! "hid:1462:7d75:7D7522120106" = "52010a..."
//! ```
//!
//! The file is not user configuration and lives apart from `config.toml`: it is rewritten whole (atomically, like
//! the settings file) and only when a value changes; the in-memory state changes only after the write succeeded.
//! Content that cannot be parsed is ignored with a warning and replaced on the next write; unknown keys are dropped
//! then. A file that exists but cannot be read (an I/O error, or content that is not UTF-8) makes the state read-only
//! for the session, so a file that could not be read is never overwritten.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cc_core::DeviceId;

use crate::error::{ConfigError, Result};
use crate::fsutil::write_atomic;
use crate::paths::Paths;

/// The table that maps device ids to their last good own lighting.
const OWN_LIGHTING: &str = "own_lighting";
/// The array of device ids CaseControl wrote to and did not release yet.
const UNRELEASED: &str = "unreleased";
/// First line of every written file.
const HEADER: &str =
    "# CaseControl machine state (ADR 0001 #38). Written by CaseControl; not meant to be edited.\n";

/// What [`HardwareState`] remembers, as plain data. A snapshot cannot write the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HardwareSnapshot {
    /// The last good own lighting of each device (driver-opaque bytes).
    pub own_lighting: BTreeMap<DeviceId, Vec<u8>>,
    /// Devices CaseControl wrote to and did not release yet.
    pub unreleased: BTreeSet<DeviceId>,
}

impl HardwareSnapshot {
    /// The remembered own lighting of `id`.
    pub fn get(&self, id: &DeviceId) -> Option<&[u8]> {
        self.own_lighting.get(id).map(Vec::as_slice)
    }

    /// Whether `id` carries the "unreleased" marker.
    pub fn is_unreleased(&self, id: &DeviceId) -> bool {
        self.unreleased.contains(id)
    }
}

/// Where a [`HardwareState`] writes its changes.
#[derive(Debug, PartialEq, Eq)]
enum Backing {
    /// Nowhere: tests, or no storage this session.
    Memory,
    /// The file the state was loaded from.
    File(PathBuf),
    /// A file that exists but could not be read: never written this session.
    ReadOnly(PathBuf),
}

/// The remembered machine state, optionally backed by a file. Deliberately not `Clone`: one owner writes the file;
/// others get a [`HardwareSnapshot`].
#[derive(Debug, PartialEq, Eq)]
pub struct HardwareState {
    backing: Backing,
    data: HardwareSnapshot,
}

impl Default for HardwareState {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl HardwareState {
    /// An empty state that is never written anywhere.
    pub fn in_memory() -> Self {
        Self {
            backing: Backing::Memory,
            data: HardwareSnapshot::default(),
        }
    }

    /// Loads `hardware-state.toml` from `paths`; see [`load_from`](Self::load_from).
    pub fn load(paths: &Paths) -> Self {
        Self::load_from(paths.hardware_state_file())
    }

    /// Loads the state from `path` and writes later changes there. A missing file gives an empty state; content that
    /// does not parse, and entries that are not valid, are ignored with a warning (the next change rewrites the file).
    /// A file that cannot be read (I/O error, not UTF-8) gives an empty state that is never written (warning).
    pub fn load_from(path: PathBuf) -> Self {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let data = parse(&path, &text);
                Self {
                    backing: Backing::File(path),
                    data,
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self {
                backing: Backing::File(path),
                data: HardwareSnapshot::default(),
            },
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "cannot read the hardware state file; using an empty state and not writing it this session"
                );
                Self {
                    backing: Backing::ReadOnly(path),
                    data: HardwareSnapshot::default(),
                }
            }
        }
    }

    /// Whether changes are kept in memory only because the file could not be read.
    pub fn is_read_only(&self) -> bool {
        matches!(self.backing, Backing::ReadOnly(_))
    }

    /// A copy of what is remembered.
    pub fn snapshot(&self) -> HardwareSnapshot {
        self.data.clone()
    }

    /// Every remembered own lighting, by device.
    pub fn own_lighting(&self) -> &BTreeMap<DeviceId, Vec<u8>> {
        &self.data.own_lighting
    }

    /// Every device that carries the "unreleased" marker.
    pub fn unreleased(&self) -> &BTreeSet<DeviceId> {
        &self.data.unreleased
    }

    /// The remembered own lighting of `id`.
    pub fn get(&self, id: &DeviceId) -> Option<&[u8]> {
        self.data.get(id)
    }

    /// Whether `id` carries the "unreleased" marker.
    pub fn is_unreleased(&self, id: &DeviceId) -> bool {
        self.data.is_unreleased(id)
    }

    /// Remembers `bytes` as the own lighting of `id`. Returns `Ok(false)` without writing when that is already what
    /// is remembered, else writes the file (if the state has a writable one), then updates the state and returns
    /// `Ok(true)`. On a write error nothing changes and the error is returned (the next record tries again).
    pub fn record_own_lighting(&mut self, id: &DeviceId, bytes: &[u8]) -> Result<bool> {
        if self.get(id) == Some(bytes) {
            return Ok(false);
        }
        let mut next = self.data.clone();
        next.own_lighting.insert(id.clone(), bytes.to_vec());
        self.commit(next)?;
        Ok(true)
    }

    /// Sets (`true`) or clears the "unreleased" marker of `id`. Returns `Ok(false)` without writing when it already
    /// has that value, else writes the file (if writable), then updates the state and returns `Ok(true)`. On a write
    /// error nothing changes and the error is returned.
    pub fn set_unreleased(&mut self, id: &DeviceId, unreleased: bool) -> Result<bool> {
        if self.is_unreleased(id) == unreleased {
            return Ok(false);
        }
        let mut next = self.data.clone();
        if unreleased {
            next.unreleased.insert(id.clone());
        } else {
            next.unreleased.remove(id);
        }
        self.commit(next)?;
        Ok(true)
    }

    /// Writes `next` (when backed by a writable file), then makes it the state.
    fn commit(&mut self, next: HardwareSnapshot) -> Result<()> {
        if let Backing::File(path) = &self.backing {
            write_atomic(path, &to_toml(&next)?)?;
        }
        self.data = next;
        Ok(())
    }
}

fn to_toml(data: &HardwareSnapshot) -> Result<String> {
    let own: toml::Table = data
        .own_lighting
        .iter()
        .map(|(id, bytes)| (id.as_str().to_owned(), toml::Value::String(to_hex(bytes))))
        .collect();
    let mut root = toml::Table::new();
    if !data.unreleased.is_empty() {
        let ids = data
            .unreleased
            .iter()
            .map(|id| toml::Value::String(id.as_str().to_owned()))
            .collect();
        root.insert(UNRELEASED.to_owned(), toml::Value::Array(ids));
    }
    root.insert(OWN_LIGHTING.to_owned(), toml::Value::Table(own));
    let body = toml::to_string(&root).map_err(|e| ConfigError::Serialize(e.to_string()))?;
    Ok(format!("{HEADER}{body}"))
}

/// A device id from a key or value of the file; `None` for an empty or whitespace-only string.
fn device_id(s: &str) -> Option<DeviceId> {
    if s.trim().is_empty() {
        return None;
    }
    toml::Value::String(s.to_owned()).try_into().ok()
}

fn parse(path: &Path, text: &str) -> HardwareSnapshot {
    let mut out = HardwareSnapshot::default();
    let root: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "hardware state file is not valid TOML; ignoring it");
            return out;
        }
    };
    match root.get(OWN_LIGHTING) {
        None => {}
        Some(toml::Value::Table(entries)) => {
            for (key, value) in entries {
                match (device_id(key), value.as_str().and_then(from_hex)) {
                    (Some(id), Some(bytes)) => {
                        out.own_lighting.insert(id, bytes);
                    }
                    _ => {
                        tracing::warn!(path = %path.display(), id = %key, "hardware state: entry is not a hex string; ignoring it");
                    }
                }
            }
        }
        Some(_) => {
            tracing::warn!(path = %path.display(), "hardware state: own_lighting is not a table; ignoring it");
        }
    }
    match root.get(UNRELEASED) {
        None => {}
        Some(toml::Value::Array(ids)) => {
            for value in ids {
                match value.as_str().and_then(device_id) {
                    Some(id) => {
                        out.unreleased.insert(id);
                    }
                    None => {
                        tracing::warn!(path = %path.display(), "hardware state: unreleased entry is not a device id; ignoring it");
                    }
                }
            }
        }
        Some(_) => {
            tracing::warn!(path = %path.display(), "hardware state: unreleased is not an array; ignoring it");
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

/// Parses [`to_hex`] output (either case). `None` for an empty string, an odd length or anything but `[0-9a-fA-F]`.
fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
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
        // `from_str_radix` alone would take a leading sign.
        assert_eq!(from_hex("+1"), None);
        assert_eq!(from_hex("0a+1"), None);
        assert_eq!(from_hex("-1"), None);
        assert_eq!(from_hex(" 1"), None);
    }

    #[test]
    fn missing_file_is_empty_and_round_trips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::at(tmp.path());
        let mut state = HardwareState::load(&paths);
        assert!(state.own_lighting().is_empty());
        assert!(!state.is_read_only());
        assert!(!paths.hardware_state_file().exists());

        let blob: Vec<u8> = (0..185u8).collect();
        assert!(state.record_own_lighting(&id(), &blob).expect("written"));
        assert!(state.set_unreleased(&id(), true).expect("marked"));
        let text = std::fs::read_to_string(paths.hardware_state_file()).expect("read");
        assert!(text.starts_with(HEADER), "{text}");
        assert!(text.contains("[own_lighting]"), "{text}");
        assert!(
            text.contains("\"hid:1462:7d75:7D7522120106\" = \"000102"),
            "{text}"
        );
        assert!(
            text.contains("unreleased = [\"hid:1462:7d75:7D7522120106\"]"),
            "{text}"
        );

        let back = HardwareState::load(&paths);
        assert_eq!(back.get(&id()), Some(blob.as_slice()));
        assert!(back.is_unreleased(&id()));
        assert_eq!(back, state);
        assert_eq!(back.snapshot(), state.snapshot());
    }

    #[test]
    fn unreleased_marker_is_set_and_cleared_only_on_change() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::at(tmp.path());
        let mut state = HardwareState::load(&paths);
        assert!(!state.set_unreleased(&id(), false).expect("already clear"));
        assert!(!paths.hardware_state_file().exists(), "nothing written");
        assert!(state.set_unreleased(&id(), true).expect("set"));
        std::fs::write(paths.hardware_state_file(), "marker").expect("write");
        assert!(!state.set_unreleased(&id(), true).expect("already set"));
        assert_eq!(
            std::fs::read_to_string(paths.hardware_state_file()).expect("read"),
            "marker"
        );
        assert!(state.set_unreleased(&id(), false).expect("cleared"));
        let back = HardwareState::load(&paths);
        assert!(!back.is_unreleased(&id()));
        assert!(back.unreleased().is_empty());
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

        std::fs::write(&file, "own_lighting = 5\nunreleased = \"x\"\n").expect("write");
        let state = HardwareState::load(&paths);
        assert!(state.own_lighting().is_empty());
        assert!(state.unreleased().is_empty());
        assert!(!state.is_read_only(), "garbage is replaced, not protected");

        std::fs::write(
            &file,
            "future = true\nunreleased = [\"hid:0001:0002\", 7, \" \"]\n[own_lighting]\n\
             \"hid:0001:0002\" = \"0a0b\"\n\"hid:0001:0003\" = \"xyz\"\n\
             \"hid:0001:0004\" = 7\n\"hid:0001:0005\" = \"\"\n\"hid:0001:0006\" = \"+1\"\n\" \" = \"0a\"\n",
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
        assert!(state.is_unreleased(&DeviceId::hid(1, 2, None)));
        assert_eq!(state.unreleased().len(), 1);

        // A record rewrites the file cleanly, dropping what could not be read.
        let mut state = state;
        assert!(state.record_own_lighting(&id(), &[9]).expect("written"));
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(!text.contains("future") && !text.contains("xyz"), "{text}");
        let back = HardwareState::load(&paths);
        assert_eq!(back.own_lighting().len(), 2);
        assert_eq!(back.unreleased().len(), 1);
    }

    #[test]
    fn unreadable_file_is_never_overwritten() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::at(tmp.path());
        let file = paths.hardware_state_file();
        // Not UTF-8: read_to_string fails with InvalidData.
        let garbage = [0xFF_u8, 0xFE, b'[', 0x80];
        std::fs::write(&file, garbage).expect("write");
        let mut state = HardwareState::load(&paths);
        assert!(state.is_read_only());
        assert!(state.own_lighting().is_empty());
        assert!(state.record_own_lighting(&id(), &[1]).expect("kept in memory"));
        assert!(state.set_unreleased(&id(), true).expect("kept in memory"));
        assert_eq!(state.get(&id()), Some(&[1u8][..]));
        assert!(state.is_unreleased(&id()));
        assert_eq!(std::fs::read(&file).expect("read"), garbage, "file untouched");

        // A path that cannot be read as a file at all (a directory): read-only too.
        let dir = tmp.path().join("dir-state");
        std::fs::create_dir(&dir).expect("dir");
        let mut state = HardwareState::load_from(dir.clone());
        assert!(state.is_read_only());
        assert!(state.record_own_lighting(&id(), &[2]).expect("kept in memory"));
        assert!(dir.is_dir());
    }

    #[test]
    fn in_memory_state_writes_nothing() {
        let mut state = HardwareState::in_memory();
        assert!(state.record_own_lighting(&id(), &[1]).expect("recorded"));
        assert!(!state.record_own_lighting(&id(), &[1]).expect("unchanged"));
        assert_eq!(state.get(&id()), Some(&[1u8][..]));
    }

    #[test]
    fn write_error_changes_nothing_and_is_retried() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // The parent "directory" is a file, so the write fails.
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "x").expect("write");
        let mut state = HardwareState::load_from(blocker.join("hardware-state.toml"));
        assert!(!state.is_read_only(), "a missing file is writable");
        assert!(state.record_own_lighting(&id(), &[1]).is_err());
        assert_eq!(state.get(&id()), None, "memory unchanged after a failed write");
        assert!(state.set_unreleased(&id(), true).is_err());
        assert!(!state.is_unreleased(&id()));
        // The same record tries the write again (it is not taken as "unchanged").
        assert!(state.record_own_lighting(&id(), &[1]).is_err());
    }
}
