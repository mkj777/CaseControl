//! Machine state remembered between runs (ADR 0001 #38, #42), in two files under `%LOCALAPPDATA%\CaseControl`
//! ([`Paths::state_dir`]):
//!
//! - `hardware-state.toml`, per device id:
//!   - the "last good own lighting": driver-opaque bytes a controller reported when it found the device showing its
//!     own lighting (`Controller::own_lighting`), stored hex-encoded so any driver can use the file; only the driver
//!     that wrote them interprets (and validates) them;
//!   - the "unreleased" marker: CaseControl has written to the device and not yet handed it back. Set before the
//!     first write after a takeover, cleared after a successful release. A device that is still marked at the next
//!     open was left in a state CaseControl wrote (crash, sleep, shutdown that kept the colours, re-enumeration), so
//!     its readback is not its own lighting.
//! - `device-memory.toml`, the records of "save colours to the devices" (ADR 0001 #42), in a file of its own that
//!   builds before it never touch: the backup of each device's original saved state (written once before the first
//!   save, never overwritten while it exists, removed only after it was written back), the fingerprint of the look
//!   saved last, the day's write count (a daily cap guards flash wear), the day's restore attempts, and the devices
//!   whose backup is to be written back when they are next connected.
//!
//! ```toml
//! # hardware-state.toml
//! machine = "6f1d...-..."
//! unreleased = ["hid:1462:7d75:7D7522120106"]
//!
//! [own_lighting]
//! "hid:1462:7d75:7D7522120106" = "52010a..."
//!
//! # device-memory.toml
//! machine = "6f1d...-..."
//! pending_restore = ["hid:046d:0a78"]
//!
//! [saved_backup]
//! "hid:1462:7d75:7D7522120106" = "52190a..."
//!
//! [saved_fingerprint]
//! "hid:1462:7d75:7D7522120106" = "52010a..."
//!
//! [saved_writes."hid:1462:7d75:7D7522120106"]
//! day = 20419
//! count = 2
//!
//! [restore_attempts."hid:1462:7d75:7D7522120106"]
//! day = 20419
//! count = 1
//! ```
//!
//! Machine binding: both files carry the machine's id (`HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`, read
//! only; [`machine_id`](crate::machine_id)). Records written on another machine (a copied profile, a cloned disk) are
//! ignored and logged, and their file is moved aside (`<file>.foreign-<unix seconds>`), so they never reach a device.
//! A machine-state file without an id (written before the binding) is adopted and bound on the next write; a
//! device-memory file without one counts as another machine's. Without a machine id the device-memory records are
//! ignored and nothing is written to them this session.
//!
//! Migration: released builds kept the own lighting and the markers in `%APPDATA%\CaseControl\hardware-state.toml`
//! (roaming). While the new `hardware-state.toml` does not exist, that file's `unreleased` and `own_lighting` are
//! copied into it once, bound to this machine. Device-memory tables in the old file (only development builds wrote
//! them) are ignored with an info line; device memory always starts from `device-memory.toml` alone. An old file that
//! cannot be read (or is larger than [`MAX_STATE_FILE_BYTES`]) or parsed is not migrated that session (the state
//! starts empty) and is tried again on the next start while the new file is still missing. The old file is left where
//! it is, so a downgrade still finds its markers.
//!
//! Writes: a file is rewritten whole (atomically, like the settings file) and only when a value changes; the
//! in-memory state changes only after the write succeeded. Device-memory writes ([`record_saved_backup`],
//! [`record_saved_write`], pending restores, restore attempts) succeed only when the file was written durably
//! (flushed to disk); an in-memory or read-only state refuses them ([`ConfigError::NotDurable`]), so the engine never
//! writes a device whose backup could get lost. Top-level keys this version does not know and entries of known tables
//! that do not parse are kept verbatim and written back, so a newer version's records (a backup above all) survive a
//! rewrite by this one. A backup entry that does not parse blocks a new backup for its device, so it is never
//! replaced and the device is never saved to ([`HardwareState::saved_backup_unreadable`]).
//!
//! A file that cannot be parsed (or whose known keys have the wrong type) is never overwritten: it is moved aside
//! (`<file>.broken-<unix seconds>`, logged) and the session starts empty; the device-memory records stay read-only for
//! the rest of the session (a fresh file is written by the next one). A file that exists but cannot be read (an I/O
//! error, content that is not UTF-8, or more than [`MAX_STATE_FILE_BYTES`]), or a broken one that cannot be moved
//! aside, makes that file read-only for the session. Every read of the two files and the old one is bounded by
//! [`MAX_STATE_FILE_BYTES`]; a larger file is kept as it is and not written this session. Device-memory records are
//! read-only only because of `device-memory.toml` itself (unreadable, too large, broken, or not movable aside) or an
//! unknown machine id, never because of the machine-state file or the old one.
//!
//! [`record_saved_backup`]: HardwareState::record_saved_backup
//! [`record_saved_write`]: HardwareState::record_saved_write

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cc_core::DeviceId;

use crate::error::{ConfigError, Result};
use crate::fsutil::{read_text_bounded, with_suffix, write_atomic, write_durable};
use crate::paths::Paths;

/// The key that binds a file to a machine.
const MACHINE: &str = "machine";
/// The table that maps device ids to their last good own lighting.
const OWN_LIGHTING: &str = "own_lighting";
/// The array of device ids CaseControl wrote to and did not release yet.
const UNRELEASED: &str = "unreleased";
/// The table of each device's original saved state (ADR 0001 #42).
const SAVED_BACKUP: &str = "saved_backup";
/// The table of the fingerprint of the look last saved to each device.
const SAVED_FINGERPRINT: &str = "saved_fingerprint";
/// The table of each device's saves of the day (`{ day, count }`).
const SAVED_WRITES: &str = "saved_writes";
/// The table of each device's restore attempts of the day (`{ day, count }`).
const RESTORE_ATTEMPTS: &str = "restore_attempts";
/// The array of devices whose backup is written back when they are next connected.
const PENDING_RESTORE: &str = "pending_restore";
/// The keys of `hardware-state.toml` (besides [`MACHINE`]).
const STATE_KEYS: &[&str] = &[OWN_LIGHTING, UNRELEASED];
/// The keys of `device-memory.toml` (besides [`MACHINE`]).
const MEMORY_KEYS: &[&str] = &[
    SAVED_BACKUP,
    SAVED_FINGERPRINT,
    SAVED_WRITES,
    RESTORE_ATTEMPTS,
    PENDING_RESTORE,
];
/// The largest machine-state, device-memory or old roaming file read (1 MiB). A larger one is not read: it is kept
/// as it is and not written this session (a device-memory file that large makes device memory read-only).
pub const MAX_STATE_FILE_BYTES: u64 = 1024 * 1024;
/// Keys that hold an array of device ids; the other known keys hold a table keyed by device id.
const ARRAY_KEYS: &[&str] = &[UNRELEASED, PENDING_RESTORE];
/// First line of the machine-state file.
const STATE_HEADER: &str =
    "# CaseControl machine state (ADR 0001 #38, #42). Written by CaseControl; not meant to be edited.\n";
/// First line of the device-memory file.
const MEMORY_HEADER: &str = "# CaseControl device memory records (ADR 0001 #42): backups of the devices' saved state. \
Written by CaseControl; do not edit or delete while saving to the devices is on.\n";

/// How often something happened to a device on one day (writes to its memory, restore attempts; ADR 0001 #42).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SavedWrites {
    /// The day, as days since 1970-01-01 (UTC).
    pub day: u32,
    /// Count on that day.
    pub count: u32,
}

/// What [`HardwareState`] remembers, as plain data. A snapshot cannot write the files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HardwareSnapshot {
    /// The last good own lighting of each device (driver-opaque bytes).
    pub own_lighting: BTreeMap<DeviceId, Vec<u8>>,
    /// Devices CaseControl wrote to and did not release yet.
    pub unreleased: BTreeSet<DeviceId>,
    /// The original saved state of each device CaseControl saved to (driver-opaque bytes, ADR 0001 #42): written
    /// once before the first save, never overwritten while it exists, removed only after it was written back.
    pub saved_backup: BTreeMap<DeviceId, Vec<u8>>,
    /// The driver-opaque fingerprint of the look last saved to each device.
    pub saved_fingerprint: BTreeMap<DeviceId, Vec<u8>>,
    /// Each device's writes to its memory on the last day it was written.
    pub saved_writes: BTreeMap<DeviceId, SavedWrites>,
    /// Each device's attempts to write its backup back, on the last day one was made.
    pub restore_attempts: BTreeMap<DeviceId, SavedWrites>,
    /// Devices whose backup is to be written back when they are next connected.
    pub pending_restore: BTreeSet<DeviceId>,
}

fn count_on(map: &BTreeMap<DeviceId, SavedWrites>, id: &DeviceId, day: u32) -> u32 {
    map.get(id).filter(|w| w.day == day).map_or(0, |w| w.count)
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

    /// The backup of `id`'s original saved state.
    pub fn saved_backup(&self, id: &DeviceId) -> Option<&[u8]> {
        self.saved_backup.get(id).map(Vec::as_slice)
    }

    /// The fingerprint of the look last saved to `id`.
    pub fn saved_fingerprint(&self, id: &DeviceId) -> Option<&[u8]> {
        self.saved_fingerprint.get(id).map(Vec::as_slice)
    }

    /// How often `id`'s memory was written on `day` (days since 1970-01-01, UTC).
    pub fn saved_writes(&self, id: &DeviceId, day: u32) -> u32 {
        count_on(&self.saved_writes, id, day)
    }

    /// How often writing `id`'s backup back was attempted on `day` (days since 1970-01-01, UTC).
    pub fn restore_attempts(&self, id: &DeviceId, day: u32) -> u32 {
        count_on(&self.restore_attempts, id, day)
    }

    /// Whether `id`'s backup is to be written back when it is next connected.
    pub fn is_pending_restore(&self, id: &DeviceId) -> bool {
        self.pending_restore.contains(id)
    }
}

/// Where a file's changes go.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Backing {
    /// Nowhere: tests, or no storage this session.
    Memory,
    /// The file the state was loaded from.
    File(PathBuf),
    /// A file that must not be written this session (unreadable, broken and not movable, or no machine id).
    ReadOnly(PathBuf),
}

/// Which of the two files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `hardware-state.toml`: own lighting and markers.
    State,
    /// `device-memory.toml`: the records of saving to the devices.
    Memory,
}

impl Kind {
    fn keys(self) -> &'static [&'static str] {
        match self {
            Self::State => STATE_KEYS,
            Self::Memory => MEMORY_KEYS,
        }
    }

    fn header(self) -> &'static str {
        match self {
            Self::State => STATE_HEADER,
            Self::Memory => MEMORY_HEADER,
        }
    }
}

/// TOML kept verbatim, compared by its text so `Eq` holds.
#[derive(Debug, Clone, Default)]
struct Verbatim(toml::Table);

impl PartialEq for Verbatim {
    fn eq(&self, other: &Self) -> bool {
        toml::to_string(&self.0).ok() == toml::to_string(&other.0).ok()
    }
}

impl Eq for Verbatim {}

/// One of the two files: where it goes, what this version did not understand in it, and the machine it is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Store {
    kind: Kind,
    backing: Backing,
    /// Top-level keys this version does not know.
    unknown: Verbatim,
    /// Per known key, the entries that did not parse: a table of them, or an array for the array keys.
    invalid: Verbatim,
    /// The machine id written into the file.
    machine: Option<String>,
}

impl Store {
    fn new(kind: Kind, backing: Backing, machine: Option<String>) -> Self {
        Self {
            kind,
            backing,
            unknown: Verbatim::default(),
            invalid: Verbatim::default(),
            machine,
        }
    }

    /// Writes `data`'s part of this file. The device-memory file is written durably and refuses an in-memory or
    /// read-only backing; the machine-state file treats an in-memory backing as success and refuses only a write to
    /// a read-only one silently (kept in memory, see [`HardwareState::is_read_only`]).
    fn write(&self, data: &HardwareSnapshot) -> Result<()> {
        match (&self.backing, self.kind) {
            (Backing::File(path), Kind::State) => write_atomic(path, &self.to_toml(data)?),
            (Backing::File(path), Kind::Memory) => write_durable(path, &self.to_toml(data)?),
            (Backing::Memory | Backing::ReadOnly(_), Kind::State) => Ok(()),
            (Backing::Memory, Kind::Memory) => Err(ConfigError::NotDurable(
                "the device memory records are kept in memory only".into(),
            )),
            (Backing::ReadOnly(path), Kind::Memory) => Err(ConfigError::NotDurable(format!(
                "{} is read-only this session",
                path.display()
            ))),
        }
    }

    fn to_toml(&self, data: &HardwareSnapshot) -> Result<String> {
        let mut root = toml::Table::new();
        // Unknown keys first, so a known key of this version always wins.
        for (k, v) in &self.unknown.0 {
            root.insert(k.clone(), v.clone());
        }
        if let Some(machine) = &self.machine {
            root.insert(MACHINE.to_owned(), toml::Value::String(machine.clone()));
        }
        let mut put = |key: &str, value: toml::Value| {
            let value = with_invalid(value, self.invalid.0.get(key));
            let empty = match &value {
                toml::Value::Array(a) => a.is_empty(),
                toml::Value::Table(t) => t.is_empty(),
                _ => false,
            };
            if !empty || key == OWN_LIGHTING {
                root.insert(key.to_owned(), value);
            }
        };
        match self.kind {
            Kind::State => {
                put(UNRELEASED, id_array(&data.unreleased));
                put(OWN_LIGHTING, toml::Value::Table(hex_table(&data.own_lighting)));
            }
            Kind::Memory => {
                put(PENDING_RESTORE, id_array(&data.pending_restore));
                put(SAVED_BACKUP, toml::Value::Table(hex_table(&data.saved_backup)));
                put(
                    SAVED_FINGERPRINT,
                    toml::Value::Table(hex_table(&data.saved_fingerprint)),
                );
                put(SAVED_WRITES, toml::Value::Table(count_table(&data.saved_writes)));
                put(
                    RESTORE_ATTEMPTS,
                    toml::Value::Table(count_table(&data.restore_attempts)),
                );
            }
        }
        let body = toml::to_string(&root).map_err(|e| ConfigError::Serialize(e.to_string()))?;
        Ok(format!("{}{body}", self.kind.header()))
    }
}

/// `value` with the invalid entries kept for its key added back (valid entries win on a key clash).
fn with_invalid(value: toml::Value, invalid: Option<&toml::Value>) -> toml::Value {
    match (value, invalid) {
        (toml::Value::Table(mut t), Some(toml::Value::Table(bad))) => {
            for (k, v) in bad {
                t.entry(k.clone()).or_insert_with(|| v.clone());
            }
            toml::Value::Table(t)
        }
        (toml::Value::Array(mut a), Some(toml::Value::Array(bad))) => {
            a.extend(bad.iter().cloned());
            toml::Value::Array(a)
        }
        (value, _) => value,
    }
}

/// The remembered machine state, backed by its two files (or by nothing). Deliberately not `Clone`: one owner writes
/// the files; others get a [`HardwareSnapshot`].
#[derive(Debug, PartialEq, Eq)]
pub struct HardwareState {
    state: Store,
    memory: Store,
    data: HardwareSnapshot,
}

impl Default for HardwareState {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl HardwareState {
    /// An empty state that is never written anywhere. Own lighting and markers are kept in memory; device-memory
    /// writes are refused (nothing could keep a backup), so the engine never saves to a device with it.
    pub fn in_memory() -> Self {
        Self {
            state: Store::new(Kind::State, Backing::Memory, None),
            memory: Store::new(Kind::Memory, Backing::Memory, None),
            data: HardwareSnapshot::default(),
        }
    }

    /// Loads the machine state of `paths` for this machine ([`machine_id`](crate::machine_id)); see
    /// [`load_with`](Self::load_with).
    pub fn load(paths: &Paths) -> Self {
        Self::load_with(paths, crate::machine_id().as_deref())
    }

    /// Loads [`Paths::hardware_state_file`] and [`Paths::device_memory_file`] bound to `machine`, after migrating the
    /// own lighting and markers of the old roaming file ([`Paths::legacy_hardware_state_file`]) while the new
    /// machine-state file does not exist (see the module docs). The migration never touches device memory.
    pub fn load_with(paths: &Paths, machine: Option<&str>) -> Self {
        let state = paths.hardware_state_file();
        let memory = paths.device_memory_file();
        let legacy = paths.legacy_hardware_state_file();
        if legacy != state && !state.exists() && legacy.is_file() {
            migrate(&legacy, &state, machine);
        }
        Self::load_files(state, memory, machine)
    }

    /// Loads the machine state from `state` (own lighting, markers) and the device-memory records from
    /// `memory`, both bound to `machine`, and writes later changes there (see the module docs). A file larger than
    /// [`MAX_STATE_FILE_BYTES`] is not read and not written this session.
    pub fn load_files(state: PathBuf, memory: PathBuf, machine: Option<&str>) -> Self {
        let (mut state, state_root) = load_store(Kind::State, state, machine);
        let (mut memory, memory_root) = load_store(Kind::Memory, memory, machine);
        let path = |s: &Store| match &s.backing {
            Backing::File(p) | Backing::ReadOnly(p) => p.clone(),
            Backing::Memory => PathBuf::new(),
        };
        let (sp, mp) = (path(&state), path(&memory));
        let data = HardwareSnapshot {
            own_lighting: parse_hex_table(&sp, &state_root, OWN_LIGHTING, &mut state.invalid),
            unreleased: parse_id_array(&sp, &state_root, UNRELEASED, &mut state.invalid),
            saved_backup: parse_hex_table(&mp, &memory_root, SAVED_BACKUP, &mut memory.invalid),
            saved_fingerprint: parse_hex_table(&mp, &memory_root, SAVED_FINGERPRINT, &mut memory.invalid),
            saved_writes: parse_counts(&mp, &memory_root, SAVED_WRITES, &mut memory.invalid),
            restore_attempts: parse_counts(&mp, &memory_root, RESTORE_ATTEMPTS, &mut memory.invalid),
            pending_restore: parse_id_array(&mp, &memory_root, PENDING_RESTORE, &mut memory.invalid),
        };
        Self { state, memory, data }
    }

    /// Loads `path` as the machine-state file and its sibling `device-memory.toml` as the device-memory file, bound
    /// to this machine ([`machine_id`](crate::machine_id)), without migration.
    pub fn load_from(path: PathBuf) -> Self {
        let memory = path.with_file_name(crate::paths::DEVICE_MEMORY_FILE);
        Self::load_files(path, memory, crate::machine_id().as_deref())
    }

    /// Whether changes to the own lighting and the markers are kept in memory only because their file could not be
    /// read.
    pub fn is_read_only(&self) -> bool {
        matches!(self.state.backing, Backing::ReadOnly(_))
    }

    /// Whether device-memory records can be written durably this session; when not, saving to and restoring devices
    /// is refused.
    pub fn device_memory_writable(&self) -> bool {
        matches!(self.memory.backing, Backing::File(_))
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

    /// Every backup of a device's original saved state, by device.
    pub fn saved_backups(&self) -> &BTreeMap<DeviceId, Vec<u8>> {
        &self.data.saved_backup
    }

    /// The backup of `id`'s original saved state.
    pub fn saved_backup(&self, id: &DeviceId) -> Option<&[u8]> {
        self.data.saved_backup(id)
    }

    /// The fingerprint of the look last saved to `id`.
    pub fn saved_fingerprint(&self, id: &DeviceId) -> Option<&[u8]> {
        self.data.saved_fingerprint(id)
    }

    /// How often `id`'s memory was written on `day` (days since 1970-01-01, UTC).
    pub fn saved_writes(&self, id: &DeviceId, day: u32) -> u32 {
        self.data.saved_writes(id, day)
    }

    /// How often writing `id`'s backup back was attempted on `day` (days since 1970-01-01, UTC).
    pub fn restore_attempts(&self, id: &DeviceId, day: u32) -> u32 {
        self.data.restore_attempts(id, day)
    }

    /// Every device whose backup is written back when it is next connected.
    pub fn pending_restores(&self) -> &BTreeSet<DeviceId> {
        &self.data.pending_restore
    }

    /// Whether `id`'s backup is written back when it is next connected.
    pub fn is_pending_restore(&self, id: &DeviceId) -> bool {
        self.data.is_pending_restore(id)
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
        self.commit(Kind::State, next)?;
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
        self.commit(Kind::State, next)?;
        Ok(true)
    }

    /// Stores `bytes` as the backup of `id`'s original saved state, unless one exists: a backup is never
    /// overwritten (`Ok(false)`, nothing written). Returns `Ok(true)` once the file durably holds it. On a write
    /// error, or when the records cannot be written durably, nothing changes and the error is returned; the caller
    /// must then not write to the device. Refused ([`ConfigError::BackupUnreadable`]) while the file holds an entry
    /// for `id`'s backup that cannot be parsed ([`saved_backup_unreadable`](Self::saved_backup_unreadable)).
    pub fn record_saved_backup(&mut self, id: &DeviceId, bytes: &[u8]) -> Result<bool> {
        if self.saved_backup(id).is_some() {
            return Ok(false);
        }
        if self.saved_backup_unreadable(id) {
            return Err(ConfigError::BackupUnreadable(id.as_str().to_owned()));
        }
        let mut next = self.data.clone();
        next.saved_backup.insert(id.clone(), bytes.to_vec());
        self.commit(Kind::Memory, next)?;
        Ok(true)
    }

    /// Drops the backup of `id` after it was written back to the device, together with the last saved fingerprint
    /// and a pending restore. The day's counts stay. `Ok(false)` without writing when there is nothing to drop.
    pub fn remove_saved_backup(&mut self, id: &DeviceId) -> Result<bool> {
        if self.saved_backup(id).is_none()
            && self.saved_fingerprint(id).is_none()
            && !self.is_pending_restore(id)
        {
            return Ok(false);
        }
        let mut next = self.data.clone();
        next.saved_backup.remove(id);
        next.saved_fingerprint.remove(id);
        next.pending_restore.remove(id);
        self.commit(Kind::Memory, next)?;
        Ok(true)
    }

    /// Forgets the fingerprint of the look last saved to `id`, because a write may have reached its memory without
    /// being recorded (a failed save, a restore, a manual save): the next save writes again. `Ok(false)` without
    /// writing when there is none.
    pub fn clear_saved_fingerprint(&mut self, id: &DeviceId) -> Result<bool> {
        if self.saved_fingerprint(id).is_none() {
            return Ok(false);
        }
        let mut next = self.data.clone();
        next.saved_fingerprint.remove(id);
        self.commit(Kind::Memory, next)?;
        Ok(true)
    }

    /// Forgets the fingerprint of `id` in memory only, for when a write reached the device's memory and neither
    /// recording it nor [`clear_saved_fingerprint`](Self::clear_saved_fingerprint) could write the file: the
    /// fingerprint no longer says what the memory holds, so this session must not report the next save of that look
    /// as unchanged. The next successful write of the file leaves it out too. Returns whether there was one.
    pub fn forget_saved_fingerprint(&mut self, id: &DeviceId) -> bool {
        self.data.saved_fingerprint.remove(id).is_some()
    }

    /// Whether the device-memory file holds an entry for `id`'s backup that cannot be parsed (kept verbatim). Such a
    /// backup may be the device's original saved state: no new backup is stored for `id`
    /// ([`record_saved_backup`](Self::record_saved_backup) refuses), so the device is never written.
    pub fn saved_backup_unreadable(&self, id: &DeviceId) -> bool {
        self.memory
            .invalid
            .0
            .get(SAVED_BACKUP)
            .and_then(toml::Value::as_table)
            .is_some_and(|t| t.contains_key(id.as_str()))
    }

    /// Records one write to `id`'s memory on `day` (days since 1970-01-01, UTC) and, with `fingerprint`, what was
    /// written (without one, the last fingerprint is dropped, since it no longer says what the memory holds).
    /// Returns the day's count afterwards. On a write error nothing changes and the error is returned.
    pub fn record_saved_write(&mut self, id: &DeviceId, fingerprint: Option<&[u8]>, day: u32) -> Result<u32> {
        let count = self.saved_writes(id, day).saturating_add(1);
        let mut next = self.data.clone();
        next.saved_writes.insert(id.clone(), SavedWrites { day, count });
        match fingerprint {
            Some(fp) => next.saved_fingerprint.insert(id.clone(), fp.to_vec()),
            None => next.saved_fingerprint.remove(id),
        };
        self.commit(Kind::Memory, next)?;
        Ok(count)
    }

    /// Records one attempt to write `id`'s backup back on `day`. Returns the day's count afterwards. On a write error
    /// nothing changes and the error is returned.
    pub fn record_restore_attempt(&mut self, id: &DeviceId, day: u32) -> Result<u32> {
        let count = self.restore_attempts(id, day).saturating_add(1);
        let mut next = self.data.clone();
        next.restore_attempts
            .insert(id.clone(), SavedWrites { day, count });
        self.commit(Kind::Memory, next)?;
        Ok(count)
    }

    /// Marks (`true`) or unmarks `id` for a restore when it is next connected. `Ok(false)` without writing when it
    /// already has that value. Fails when the records cannot be written durably.
    pub fn set_pending_restore(&mut self, id: &DeviceId, pending: bool) -> Result<bool> {
        if self.is_pending_restore(id) == pending {
            return Ok(false);
        }
        let mut next = self.data.clone();
        if pending {
            next.pending_restore.insert(id.clone());
        } else {
            next.pending_restore.remove(id);
        }
        self.commit(Kind::Memory, next)?;
        Ok(true)
    }

    /// Unmarks every pending restore. `Ok(false)` without writing when there is none.
    pub fn clear_pending_restores(&mut self) -> Result<bool> {
        if self.data.pending_restore.is_empty() {
            return Ok(false);
        }
        let mut next = self.data.clone();
        next.pending_restore.clear();
        self.commit(Kind::Memory, next)?;
        Ok(true)
    }

    /// Writes `kind`'s file with `next`, then makes it the state.
    fn commit(&mut self, kind: Kind, next: HardwareSnapshot) -> Result<()> {
        match kind {
            Kind::State => self.state.write(&next)?,
            Kind::Memory => self.memory.write(&next)?,
        }
        self.data = next;
        Ok(())
    }
}

/// Seconds since 1970, for the names of files moved aside.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Moves `path` to `<path>.<tag>-<unix seconds>`; returns where it went.
fn set_aside(path: &Path, tag: &str) -> std::io::Result<PathBuf> {
    let aside = with_suffix(path, &format!(".{tag}-{}", now_secs()));
    std::fs::rename(path, &aside)?;
    Ok(aside)
}

/// Why a file's content is not used.
#[derive(Debug)]
enum Rejected {
    /// Not TOML, or a known key of the wrong type.
    Broken(String),
    /// Bound to another machine (or, for the device-memory file, to none).
    Foreign(Option<String>),
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Broken(why) => write!(f, "cannot be parsed: {why}"),
            Self::Foreign(Some(m)) => write!(f, "written on another machine ({m})"),
            Self::Foreign(None) => write!(f, "not bound to a machine"),
        }
    }
}

/// The root table of a file's text, checked: TOML, known keys of the right type, bound to `machine`.
fn check_root(kind: Kind, text: &str, machine: Option<&str>) -> std::result::Result<toml::Table, Rejected> {
    let root: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| Rejected::Broken(e.to_string()))?;
    for key in kind.keys() {
        let ok = match root.get(*key) {
            None => true,
            Some(v) if ARRAY_KEYS.contains(key) => v.is_array(),
            Some(v) => v.is_table(),
        };
        if !ok {
            return Err(Rejected::Broken(format!("`{key}` has the wrong type")));
        }
    }
    let bound = match root.get(MACHINE) {
        None => None,
        Some(toml::Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(Rejected::Broken(format!("`{MACHINE}` is not a string"))),
    };
    let ours = match (kind, &bound, machine) {
        (_, Some(b), Some(m)) => b == m,
        // Written before the binding: adopted (it is bound on the next write).
        (Kind::State, None, _) => true,
        // No machine id this session: own lighting is validated by the drivers anyway.
        (Kind::State, Some(_), None) => true,
        (Kind::Memory, _, _) => false,
    };
    if ours {
        Ok(root)
    } else {
        Err(Rejected::Foreign(bound))
    }
}

/// Reads one of the machine-state files (also the old roaming one) as text, bounded by [`MAX_STATE_FILE_BYTES`].
fn read_state_text(path: &Path) -> std::io::Result<String> {
    read_text_bounded(path, MAX_STATE_FILE_BYTES)
}

/// Opens one of the two files (see the module docs) and returns it with the root table to read records from.
fn load_store(kind: Kind, path: PathBuf, machine: Option<&str>) -> (Store, toml::Table) {
    let fresh = |backing| Store::new(kind, backing, machine.map(str::to_owned));
    if kind == Kind::Memory && machine.is_none() {
        tracing::warn!(
            path = %path.display(),
            "the machine id is unknown; device memory records are ignored and not written this session"
        );
        return (fresh(Backing::ReadOnly(path)), toml::Table::new());
    }
    let text = match read_state_text(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (fresh(Backing::File(path)), toml::Table::new());
        }
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "cannot read the machine state file; using an empty state and not writing it this session"
            );
            return (fresh(Backing::ReadOnly(path)), toml::Table::new());
        }
    };
    match check_root(kind, &text, machine) {
        Ok(mut root) => {
            let bound = match root.remove(MACHINE) {
                Some(toml::Value::String(s)) => Some(s),
                _ => None,
            };
            let unknown: toml::Table = root
                .iter()
                .filter(|(k, _)| !kind.keys().contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let mut store = Store::new(kind, Backing::File(path), machine.map(str::to_owned).or(bound));
            store.unknown = Verbatim(unknown);
            (store, root)
        }
        Err(why) => {
            let tag = match &why {
                Rejected::Broken(_) => "broken",
                Rejected::Foreign(_) => "foreign",
            };
            match set_aside(&path, tag) {
                Ok(aside) => {
                    tracing::warn!(
                        path = %path.display(),
                        moved_to = %aside.display(),
                        reason = %why,
                        "machine state file not used; moved aside, starting empty"
                    );
                    // A broken device-memory file may have held backups: no device-memory writes this session.
                    let backing = match (kind, &why) {
                        (Kind::Memory, Rejected::Broken(_)) => Backing::ReadOnly(path),
                        _ => Backing::File(path),
                    };
                    (fresh(backing), toml::Table::new())
                }
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        reason = %why,
                        error = %e,
                        "machine state file not used and cannot be moved aside; not writing it this session"
                    );
                    (fresh(Backing::ReadOnly(path)), toml::Table::new())
                }
            }
        }
    }
}

/// Copies the own lighting and the markers (`own_lighting`, `unreleased`) of the old roaming file `legacy` into the
/// new machine-state file `state`, bound to `machine` (see the module docs). Device-memory tables in the old file are
/// ignored; the old file is left as it is. When the old file cannot be read or parsed, nothing is written, so the next
/// start tries again.
fn migrate(legacy: &Path, state: &Path, machine: Option<&str>) {
    let text = match read_state_text(legacy) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(path = %legacy.display(), error = %e, "cannot read the old machine state file; not migrating it this session");
            return;
        }
    };
    let mut root: toml::Table = match text.parse() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(path = %legacy.display(), error = %e, "the old machine state file does not parse; not migrating it this session");
            return;
        }
    };
    if MEMORY_KEYS.iter().any(|k| root.contains_key(*k)) {
        tracing::info!(path = %legacy.display(), "the old machine state file holds device memory records; ignoring them");
    }
    let mut out = toml::Table::new();
    if let Some(m) = machine {
        out.insert(MACHINE.to_owned(), toml::Value::String(m.to_owned()));
    }
    for key in STATE_KEYS {
        match root.remove(*key) {
            Some(v) if v.is_array() == ARRAY_KEYS.contains(key) && (v.is_array() || v.is_table()) => {
                out.insert((*key).to_owned(), v);
            }
            Some(_) => {
                tracing::warn!(path = %legacy.display(), key = *key, "old machine state: wrong type; not migrating it");
            }
            None => {}
        }
    }
    let written = toml::to_string(&out)
        .map_err(|e| e.to_string())
        .and_then(|body| write_atomic(state, &format!("{STATE_HEADER}{body}")).map_err(|e| e.to_string()));
    match written {
        Ok(()) => tracing::info!(from = %legacy.display(), to = %state.display(), "machine state migrated"),
        Err(e) => {
            tracing::warn!(to = %state.display(), error = %e, "cannot write the migrated machine state")
        }
    }
}

fn hex_table(map: &BTreeMap<DeviceId, Vec<u8>>) -> toml::Table {
    map.iter()
        .map(|(id, bytes)| (id.as_str().to_owned(), toml::Value::String(to_hex(bytes))))
        .collect()
}

fn count_table(map: &BTreeMap<DeviceId, SavedWrites>) -> toml::Table {
    map.iter()
        .map(|(id, w)| {
            let mut t = toml::Table::new();
            t.insert("day".into(), toml::Value::Integer(i64::from(w.day)));
            t.insert("count".into(), toml::Value::Integer(i64::from(w.count)));
            (id.as_str().to_owned(), toml::Value::Table(t))
        })
        .collect()
}

fn id_array(ids: &BTreeSet<DeviceId>) -> toml::Value {
    toml::Value::Array(
        ids.iter()
            .map(|id| toml::Value::String(id.as_str().to_owned()))
            .collect(),
    )
}

/// A device id from a key or value of the file; `None` for an empty or whitespace-only string.
fn device_id(s: &str) -> Option<DeviceId> {
    if s.trim().is_empty() {
        return None;
    }
    toml::Value::String(s.to_owned()).try_into().ok()
}

/// Keeps an entry of table `key` that did not parse, to write it back verbatim.
fn keep_invalid_entry(invalid: &mut Verbatim, key: &str, id: &str, value: &toml::Value) {
    if let toml::Value::Table(t) = invalid
        .0
        .entry(key.to_owned())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
    {
        t.insert(id.to_owned(), value.clone());
    }
}

/// A table of `id = "hex"` entries; entries that are not valid are kept verbatim with a warning.
fn parse_hex_table(
    path: &Path,
    root: &toml::Table,
    key: &str,
    invalid: &mut Verbatim,
) -> BTreeMap<DeviceId, Vec<u8>> {
    let mut out = BTreeMap::new();
    let Some(entries) = root.get(key).and_then(toml::Value::as_table) else {
        return out;
    };
    for (id, value) in entries {
        match (device_id(id), value.as_str().and_then(from_hex)) {
            (Some(device), Some(bytes)) => {
                out.insert(device, bytes);
            }
            _ => {
                tracing::warn!(path = %path.display(), table = key, %id, "machine state: entry is not a hex string; keeping it as is");
                keep_invalid_entry(invalid, key, id, value);
            }
        }
    }
    out
}

/// An array of device ids; entries that are not valid are kept verbatim with a warning.
fn parse_id_array(path: &Path, root: &toml::Table, key: &str, invalid: &mut Verbatim) -> BTreeSet<DeviceId> {
    let mut out = BTreeSet::new();
    let Some(ids) = root.get(key).and_then(toml::Value::as_array) else {
        return out;
    };
    for value in ids {
        match value.as_str().and_then(device_id) {
            Some(id) => {
                out.insert(id);
            }
            None => {
                tracing::warn!(path = %path.display(), array = key, "machine state: entry is not a device id; keeping it as is");
                if let toml::Value::Array(a) = invalid
                    .0
                    .entry(key.to_owned())
                    .or_insert_with(|| toml::Value::Array(Vec::new()))
                {
                    a.push(value.clone());
                }
            }
        }
    }
    out
}

/// A table of `id = { day, count }` entries; entries that are not valid are kept verbatim with a warning.
fn parse_counts(
    path: &Path,
    root: &toml::Table,
    key: &str,
    invalid: &mut Verbatim,
) -> BTreeMap<DeviceId, SavedWrites> {
    let mut out = BTreeMap::new();
    let Some(entries) = root.get(key).and_then(toml::Value::as_table) else {
        return out;
    };
    for (id, value) in entries {
        let field = |name: &str| {
            value
                .get(name)
                .and_then(toml::Value::as_integer)
                .and_then(|v| u32::try_from(v).ok())
        };
        match (device_id(id), field("day"), field("count")) {
            (Some(device), Some(day), Some(count)) => {
                out.insert(device, SavedWrites { day, count });
            }
            _ => {
                tracing::warn!(path = %path.display(), table = key, %id, "machine state: entry is not a table of day and count; keeping it as is");
                keep_invalid_entry(invalid, key, id, value);
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

    const M: &str = "machine-one";

    fn id() -> DeviceId {
        DeviceId::hid(0x1462, 0x7D75, Some("7D7522120106"))
    }

    fn temp() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::at(tmp.path());
        (tmp, paths)
    }

    fn load(paths: &Paths) -> HardwareState {
        HardwareState::load_with(paths, Some(M))
    }

    /// Files in `dir` whose name starts with `prefix` and is longer (the ones moved aside).
    fn aside(dir: &Path, prefix: &str) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .expect("read dir")
            .map(|e| e.expect("entry").path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.starts_with(prefix) && name.len() > prefix.len()
            })
            .collect()
    }

    #[test]
    fn hex_round_trip_and_rejects() {
        assert_eq!(to_hex(&[0x52, 0x00, 0xAB]), "5200ab");
        assert_eq!(from_hex("5200AB"), Some(vec![0x52, 0x00, 0xAB]));
        assert_eq!(from_hex(""), None);
        assert_eq!(from_hex("520"), None);
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("\u{e9}1"), None);
        // `from_str_radix` alone would take a leading sign.
        assert_eq!(from_hex("+1"), None);
        assert_eq!(from_hex("0a+1"), None);
        assert_eq!(from_hex("-1"), None);
        assert_eq!(from_hex(" 1"), None);
    }

    #[test]
    fn missing_files_are_empty_and_round_trip_bound_to_the_machine() {
        let (_tmp, paths) = temp();
        let mut state = load(&paths);
        assert!(state.own_lighting().is_empty());
        assert!(!state.is_read_only());
        assert!(state.device_memory_writable());
        assert!(!paths.hardware_state_file().exists());

        let blob: Vec<u8> = (0..185u8).collect();
        assert!(state.record_own_lighting(&id(), &blob).expect("written"));
        assert!(state.set_unreleased(&id(), true).expect("marked"));
        let text = std::fs::read_to_string(paths.hardware_state_file()).expect("read");
        assert!(text.starts_with(STATE_HEADER), "{text}");
        assert!(text.contains("machine = \"machine-one\""), "{text}");
        assert!(text.contains("[own_lighting]"), "{text}");
        assert!(
            text.contains("\"hid:1462:7d75:7D7522120106\" = \"000102"),
            "{text}"
        );
        assert!(
            text.contains("unreleased = [\"hid:1462:7d75:7D7522120106\"]"),
            "{text}"
        );
        assert!(
            !paths.device_memory_file().exists(),
            "no device memory record yet"
        );

        let back = load(&paths);
        assert_eq!(back.get(&id()), Some(blob.as_slice()));
        assert!(back.is_unreleased(&id()));
        assert_eq!(back, state);
        assert_eq!(back.snapshot(), state.snapshot());
    }

    #[test]
    fn unreleased_marker_is_set_and_cleared_only_on_change() {
        let (_tmp, paths) = temp();
        let mut state = load(&paths);
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
        let back = load(&paths);
        assert!(!back.is_unreleased(&id()));
        assert!(back.unreleased().is_empty());
    }

    #[test]
    fn unchanged_value_is_not_rewritten() {
        let (_tmp, paths) = temp();
        let mut state = load(&paths);
        assert!(state.record_own_lighting(&id(), &[1, 2, 3]).expect("first"));
        std::fs::write(paths.hardware_state_file(), "marker").expect("write");
        assert!(!state.record_own_lighting(&id(), &[1, 2, 3]).expect("same"));
        assert_eq!(
            std::fs::read_to_string(paths.hardware_state_file()).expect("read"),
            "marker"
        );
        assert!(state.record_own_lighting(&id(), &[1, 2, 4]).expect("changed"));
        assert_eq!(load(&paths).get(&id()), Some(&[1u8, 2, 4][..]));
    }

    #[test]
    fn unparsable_file_is_moved_aside_not_overwritten() {
        let (tmp, paths) = temp();
        let file = paths.hardware_state_file();
        std::fs::write(&file, "this is [ not toml").expect("write");
        let mut state = load(&paths);
        assert!(state.own_lighting().is_empty());
        assert!(!file.exists());
        let moved = aside(tmp.path(), "hardware-state.toml.broken-");
        assert_eq!(moved.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&moved[0]).expect("read"),
            "this is [ not toml"
        );
        assert!(!state.is_read_only(), "the machine state starts fresh");
        assert!(state.record_own_lighting(&id(), &[1]).expect("written"));
        assert_eq!(
            std::fs::read_to_string(&moved[0]).expect("read"),
            "this is [ not toml"
        );

        // A known key of the wrong type counts as broken too.
        std::fs::remove_file(&moved[0]).expect("remove");
        std::fs::write(&file, "own_lighting = 5\n").expect("write");
        let state = load(&paths);
        assert!(state.own_lighting().is_empty());
        assert!(!file.exists());
        assert_eq!(aside(tmp.path(), "hardware-state.toml.broken-").len(), 1);
    }

    #[test]
    fn broken_device_memory_is_kept_and_read_only_this_session() {
        let (tmp, paths) = temp();
        let file = paths.device_memory_file();
        std::fs::write(&file, "machine = \"machine-one\"\n[saved_backup\n").expect("write");
        let mut state = load(&paths);
        assert!(!state.device_memory_writable());
        assert!(state.saved_backups().is_empty());
        assert!(matches!(
            state.record_saved_backup(&id(), &[1]),
            Err(ConfigError::NotDurable(_))
        ));
        assert!(state.set_pending_restore(&id(), true).is_err());
        assert_eq!(state.saved_backup(&id()), None);
        assert!(!file.exists(), "not recreated this session");
        assert_eq!(aside(tmp.path(), "device-memory.toml.broken-").len(), 1);
        // The next session starts with a fresh file.
        let mut next = load(&paths);
        assert!(next.device_memory_writable());
        assert!(next.record_saved_backup(&id(), &[2]).expect("written"));
        assert_eq!(aside(tmp.path(), "device-memory.toml.broken-").len(), 1);
    }

    #[test]
    fn invalid_entries_of_known_tables_are_kept_verbatim() {
        let (_tmp, paths) = temp();
        let file = paths.hardware_state_file();
        std::fs::write(
            &file,
            "machine = \"machine-one\"\nfuture = true\nunreleased = [\"hid:0001:0002\", 7, \" \"]\n[own_lighting]\n\
             \"hid:0001:0002\" = \"0a0b\"\n\"hid:0001:0003\" = \"xyz\"\n\
             \"hid:0001:0004\" = 7\n\"hid:0001:0005\" = \"\"\n\"hid:0001:0006\" = \"+1\"\n\" \" = \"0a\"\n",
        )
        .expect("write");
        let mut state = load(&paths);
        assert_eq!(
            state
                .own_lighting()
                .keys()
                .map(DeviceId::as_str)
                .collect::<Vec<_>>(),
            ["hid:0001:0002"]
        );
        assert!(state.is_unreleased(&DeviceId::hid(1, 2, None)));
        assert_eq!(state.unreleased().len(), 1);

        assert!(state.record_own_lighting(&id(), &[9]).expect("written"));
        let text = std::fs::read_to_string(&file).expect("read");
        for kept in [
            "xyz",
            "\"hid:0001:0004\" = 7",
            "\"+1\"",
            "future = true",
            "\" \" = \"0a\"",
        ] {
            assert!(text.contains(kept), "{kept} in {text}");
        }
        assert!(
            text.contains("unreleased = [\"hid:0001:0002\", 7, \" \"]"),
            "{text}"
        );
        let back = load(&paths);
        assert_eq!(back.own_lighting().len(), 2);
        assert_eq!(back.unreleased().len(), 1);
        assert_eq!(back, state);
    }

    #[test]
    fn save_records_round_trip_in_their_own_file() {
        let (_tmp, paths) = temp();
        let other = DeviceId::hid(0x046D, 0x0A78, None);
        let mut state = load(&paths);
        assert!(state.record_saved_backup(&id(), &[1, 2, 3]).expect("backup"));
        assert_eq!(
            state
                .record_saved_write(&id(), Some(&[7, 7]), 100)
                .expect("write"),
            1
        );
        assert_eq!(state.record_restore_attempt(&id(), 100).expect("attempt"), 1);
        assert!(state.set_pending_restore(&other, true).expect("pending"));
        assert!(!state.set_pending_restore(&other, true).expect("already"));
        assert!(
            !paths.hardware_state_file().exists(),
            "the machine state file is not touched"
        );

        let text = std::fs::read_to_string(paths.device_memory_file()).expect("read");
        assert!(text.starts_with(MEMORY_HEADER), "{text}");
        assert!(text.contains("machine = \"machine-one\""), "{text}");
        assert!(text.contains("[saved_backup]"), "{text}");
        assert!(text.contains("pending_restore = [\"hid:046d:0a78\"]"), "{text}");

        let back = load(&paths);
        assert_eq!(back, state);
        assert_eq!(back.saved_backup(&id()), Some(&[1u8, 2, 3][..]));
        assert_eq!(back.saved_fingerprint(&id()), Some(&[7u8, 7][..]));
        assert_eq!(back.saved_writes(&id(), 100), 1);
        assert_eq!(back.saved_writes(&id(), 101), 0, "another day starts at 0");
        assert_eq!(back.restore_attempts(&id(), 100), 1);
        assert_eq!(back.snapshot().restore_attempts(&id(), 101), 0);
        assert!(back.is_pending_restore(&other));

        // A write without a fingerprint drops the old one; a new day restarts the count.
        let mut state = back;
        assert_eq!(state.record_saved_write(&id(), None, 101).expect("write"), 1);
        assert_eq!(state.saved_fingerprint(&id()), None);
        assert_eq!(state.saved_writes(&id(), 100), 0);
        assert!(state.clear_pending_restores().expect("cleared"));
        assert!(!state.clear_pending_restores().expect("nothing left"));
        state.record_saved_write(&id(), Some(&[1]), 101).expect("write");
        assert!(state.clear_saved_fingerprint(&id()).expect("cleared"));
        assert!(!state.clear_saved_fingerprint(&id()).expect("nothing left"));
        assert_eq!(load(&paths).saved_fingerprint(&id()), None);
    }

    #[test]
    fn backup_is_never_overwritten_and_dropped_only_on_request() {
        let (_tmp, paths) = temp();
        let mut state = load(&paths);
        assert!(state.record_saved_backup(&id(), &[1]).expect("first"));
        assert!(!state.record_saved_backup(&id(), &[2]).expect("kept"));
        assert_eq!(state.saved_backup(&id()), Some(&[1u8][..]));
        state.record_saved_write(&id(), Some(&[9]), 5).expect("write");
        state.set_pending_restore(&id(), true).expect("pending");
        assert!(state.remove_saved_backup(&id()).expect("removed"));
        assert_eq!(state.saved_backup(&id()), None);
        assert_eq!(state.saved_fingerprint(&id()), None);
        assert!(!state.is_pending_restore(&id()));
        assert_eq!(state.saved_writes(&id(), 5), 1, "the day's count stays");
        assert!(!state.remove_saved_backup(&id()).expect("nothing to drop"));
        assert!(
            state
                .record_saved_backup(&id(), &[2])
                .expect("a new backup after the restore")
        );
    }

    #[test]
    fn unparsable_backup_blocks_a_new_backup_and_survives_every_rewrite() {
        let (_tmp, paths) = temp();
        let file = paths.device_memory_file();
        let entry = "\"hid:1462:7d75:7D7522120106\" = \"not hex\"";
        std::fs::write(
            &file,
            format!("machine = \"machine-one\"\n[saved_backup]\n{entry}\n"),
        )
        .expect("write");
        let mut state = load(&paths);
        let other = DeviceId::hid(1, 2, None);
        assert_eq!(state.saved_backup(&id()), None);
        assert!(state.saved_backup_unreadable(&id()));
        assert!(!state.saved_backup_unreadable(&other));
        assert!(matches!(
            state.record_saved_backup(&id(), &[1]),
            Err(ConfigError::BackupUnreadable(_))
        ));
        assert_eq!(state.saved_backup(&id()), None);

        // Every rewrite keeps it: another device's backup, its removal, the device's own counts and fingerprint.
        assert!(state.record_saved_backup(&other, &[2]).expect("other device"));
        state.record_saved_write(&id(), Some(&[3]), 1).expect("count");
        state.set_pending_restore(&id(), true).expect("pending");
        assert!(
            state
                .remove_saved_backup(&id())
                .expect("fingerprint and pending dropped")
        );
        assert!(state.remove_saved_backup(&other).expect("removed"));
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(text.contains(entry), "{text}");
        let mut back = load(&paths);
        assert!(back.saved_backup_unreadable(&id()));
        assert!(back.record_saved_backup(&id(), &[1]).is_err());
    }

    #[test]
    fn forgotten_fingerprint_is_gone_in_memory_and_from_the_next_write() {
        let (_tmp, paths) = temp();
        let mut state = load(&paths);
        state.record_saved_write(&id(), Some(&[1]), 1).expect("write");
        assert!(state.forget_saved_fingerprint(&id()));
        assert!(!state.forget_saved_fingerprint(&id()));
        assert_eq!(state.saved_fingerprint(&id()), None);
        assert_eq!(
            load(&paths).saved_fingerprint(&id()),
            Some(&[1u8][..]),
            "the file is untouched"
        );
        state.record_restore_attempt(&id(), 1).expect("another write");
        assert_eq!(load(&paths).saved_fingerprint(&id()), None);
    }

    #[test]
    fn in_memory_state_keeps_own_lighting_but_refuses_device_memory() {
        let mut state = HardwareState::in_memory();
        assert!(state.record_own_lighting(&id(), &[1]).expect("recorded"));
        assert!(!state.record_own_lighting(&id(), &[1]).expect("unchanged"));
        assert!(state.set_unreleased(&id(), true).expect("marked"));
        assert_eq!(state.get(&id()), Some(&[1u8][..]));
        assert!(!state.device_memory_writable());
        assert!(matches!(
            state.record_saved_backup(&id(), &[1]),
            Err(ConfigError::NotDurable(_))
        ));
        assert!(state.record_saved_write(&id(), None, 1).is_err());
        assert!(state.record_restore_attempt(&id(), 1).is_err());
        assert!(state.set_pending_restore(&id(), true).is_err());
        assert_eq!(state.saved_backup(&id()), None);
        assert!(!state.is_pending_restore(&id()));
    }

    #[test]
    fn failed_backup_write_changes_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "x").expect("write");
        let mut state = HardwareState::load_files(
            blocker.join("hardware-state.toml"),
            blocker.join("device-memory.toml"),
            Some(M),
        );
        assert!(state.device_memory_writable(), "a missing file is writable");
        assert!(state.record_saved_backup(&id(), &[1]).is_err());
        assert_eq!(state.saved_backup(&id()), None);
        assert!(state.record_saved_write(&id(), Some(&[1]), 1).is_err());
        assert_eq!(state.saved_writes(&id(), 1), 0);
    }

    #[test]
    fn unknown_keys_survive_a_rewrite() {
        let (_tmp, paths) = temp();
        let file = paths.device_memory_file();
        std::fs::write(
            &file,
            "machine = \"machine-one\"\nnewer = 3\n[saved_backup]\n\"hid:0001:0002\" = \"0a\"\n[future_table]\n\
             \"hid:0001:0002\" = \"ff\"\n[saved_writes]\n\"hid:0001:0002\" = { day = 4, count = 2 }\n\
             \"hid:0001:0003\" = { day = -1 }\n",
        )
        .expect("write");
        let mut state = load(&paths);
        assert_eq!(
            state.saved_backup(&DeviceId::hid(1, 2, None)),
            Some(&[0x0A_u8][..])
        );
        assert_eq!(state.saved_writes(&DeviceId::hid(1, 2, None), 4), 2);
        assert_eq!(state.snapshot().saved_writes.len(), 1, "invalid entry not used");
        assert!(state.set_pending_restore(&id(), true).expect("written"));
        let back = std::fs::read_to_string(&file).expect("read");
        assert!(back.contains("newer = 3"), "{back}");
        assert!(back.contains("[future_table]"), "{back}");
        assert!(back.contains("\"hid:0001:0002\" = \"0a\""), "{back}");
        assert!(back.contains("day = -1"), "invalid entry kept: {back}");
        assert_eq!(load(&paths), state);
    }

    #[test]
    fn unreadable_file_is_never_overwritten() {
        let (_tmp, paths) = temp();
        let file = paths.hardware_state_file();
        // Not UTF-8: read_to_string fails with InvalidData.
        let garbage = [0xFF_u8, 0xFE, b'[', 0x80];
        std::fs::write(&file, garbage).expect("write");
        std::fs::write(paths.device_memory_file(), garbage).expect("write");
        let mut state = load(&paths);
        assert!(state.is_read_only());
        assert!(!state.device_memory_writable());
        assert!(state.own_lighting().is_empty());
        assert!(state.record_own_lighting(&id(), &[1]).expect("kept in memory"));
        assert!(state.set_unreleased(&id(), true).expect("kept in memory"));
        assert!(state.record_saved_backup(&id(), &[1]).is_err());
        assert_eq!(state.get(&id()), Some(&[1u8][..]));
        assert_eq!(std::fs::read(&file).expect("read"), garbage, "file untouched");
        assert_eq!(std::fs::read(paths.device_memory_file()).expect("read"), garbage);
    }

    #[test]
    fn write_error_changes_nothing_and_is_retried() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "x").expect("write");
        let mut state = HardwareState::load_files(
            blocker.join("hardware-state.toml"),
            blocker.join("device-memory.toml"),
            Some(M),
        );
        assert!(!state.is_read_only(), "a missing file is writable");
        assert!(state.record_own_lighting(&id(), &[1]).is_err());
        assert_eq!(state.get(&id()), None, "memory unchanged after a failed write");
        assert!(state.set_unreleased(&id(), true).is_err());
        assert!(!state.is_unreleased(&id()));
        assert!(state.record_own_lighting(&id(), &[1]).is_err());
    }

    #[test]
    fn records_of_another_machine_are_ignored_and_moved_aside() {
        let (tmp, paths) = temp();
        std::fs::write(
            paths.device_memory_file(),
            "machine = \"machine-two\"\n[saved_backup]\n\"hid:1462:7d75:7D7522120106\" = \"52\"\n",
        )
        .expect("write");
        std::fs::write(
            paths.hardware_state_file(),
            "machine = \"machine-two\"\nunreleased = [\"hid:1462:7d75:7D7522120106\"]\n",
        )
        .expect("write");
        let mut state = load(&paths);
        assert_eq!(state.saved_backup(&id()), None);
        assert!(!state.is_unreleased(&id()));
        assert!(
            state.device_memory_writable(),
            "this machine starts its own records"
        );
        assert_eq!(aside(tmp.path(), "device-memory.toml.foreign-").len(), 1);
        assert_eq!(aside(tmp.path(), "hardware-state.toml.foreign-").len(), 1);
        assert!(state.record_saved_backup(&id(), &[9]).expect("written"));
        let text = std::fs::read_to_string(paths.device_memory_file()).expect("read");
        assert!(text.contains("machine = \"machine-one\""), "{text}");

        // A device-memory file without a machine id is not trusted; a machine-state file without one is adopted.
        std::fs::write(
            paths.device_memory_file(),
            "[saved_backup]\n\"hid:0001:0002\" = \"0a\"\n",
        )
        .expect("write");
        std::fs::write(paths.hardware_state_file(), "unreleased = [\"hid:0001:0002\"]\n").expect("write");
        let state = load(&paths);
        assert!(state.saved_backups().is_empty());
        assert!(state.is_unreleased(&DeviceId::hid(1, 2, None)));
    }

    #[test]
    fn without_a_machine_id_device_memory_is_read_only() {
        let (_tmp, paths) = temp();
        let text = "machine = \"machine-one\"\n[saved_backup]\n\"hid:0001:0002\" = \"0a\"\n";
        std::fs::write(paths.device_memory_file(), text).expect("write");
        let mut state = HardwareState::load_with(&paths, None);
        assert!(!state.device_memory_writable());
        assert!(state.saved_backups().is_empty());
        assert!(state.record_saved_backup(&id(), &[1]).is_err());
        assert!(
            state
                .record_own_lighting(&id(), &[1])
                .expect("own lighting still works")
        );
        assert_eq!(
            std::fs::read_to_string(paths.device_memory_file()).expect("read"),
            text
        );
    }

    /// Paths whose old roaming file is apart from the new files, and the old file with records of both kinds.
    fn roaming() -> (tempfile::TempDir, Paths, String) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let roaming = tmp.path().join("Roaming").join("CaseControl");
        let local = tmp.path().join("Local").join("CaseControl");
        std::fs::create_dir_all(&roaming).expect("dir");
        let paths = Paths {
            config_dir: roaming.clone(),
            profiles_dir: roaming.join("profiles"),
            logs_dir: local.join("logs"),
        };
        let old = "future = 1
unreleased = [\"hid:1462:7d75:7D7522120106\"]
pending_restore = [\"hid:046d:0a78\"]
                   [own_lighting]
\"hid:1462:7d75:7D7522120106\" = \"0102\"
                   [saved_backup]
\"hid:1462:7d75:7D7522120106\" = \"0a0b\"
                   [saved_fingerprint]
\"hid:1462:7d75:7D7522120106\" = \"01\"
"
        .to_owned();
        std::fs::write(paths.legacy_hardware_state_file(), &old).expect("write");
        (tmp, paths, old)
    }

    #[test]
    fn old_roaming_file_is_migrated_once_and_left_in_place() {
        let (_tmp, paths, old) = roaming();
        let state = load(&paths);
        assert_eq!(state.get(&id()), Some(&[1u8, 2][..]));
        assert!(state.is_unreleased(&id()));
        assert_eq!(
            std::fs::read_to_string(paths.legacy_hardware_state_file()).expect("read"),
            old
        );
        let migrated = std::fs::read_to_string(paths.hardware_state_file()).expect("read");
        assert!(migrated.starts_with(STATE_HEADER), "{migrated}");
        assert!(migrated.contains("machine = \"machine-one\""), "{migrated}");
        assert!(!migrated.contains("future"), "only the two keys: {migrated}");

        // Once: a changed old file is not read again while the new file exists.
        std::fs::write(
            paths.legacy_hardware_state_file(),
            "unreleased = []
",
        )
        .expect("write");
        let mut state = load(&paths);
        assert!(state.is_unreleased(&id()));
        // Later writes go to the new file only.
        assert!(state.set_unreleased(&id(), false).expect("cleared"));
        assert_eq!(
            std::fs::read_to_string(paths.legacy_hardware_state_file()).expect("read"),
            "unreleased = []
"
        );
    }

    #[test]
    fn device_memory_tables_of_the_old_file_are_not_migrated() {
        let (_tmp, paths, _) = roaming();
        let mut state = load(&paths);
        assert!(state.saved_backups().is_empty());
        assert_eq!(state.saved_fingerprint(&id()), None);
        assert!(state.pending_restores().is_empty());
        assert!(state.device_memory_writable());
        assert!(!paths.device_memory_file().exists());
        let migrated = std::fs::read_to_string(paths.hardware_state_file()).expect("read");
        for key in MEMORY_KEYS {
            assert!(!migrated.contains(key), "{key} in {migrated}");
        }
        assert!(state.record_saved_backup(&id(), &[9]).expect("written"));
        assert_eq!(load(&paths).saved_backup(&id()), Some(&[9u8][..]));

        // Without a machine id the markers still migrate (bound on the next write).
        let (_tmp, paths, _) = roaming();
        let state = HardwareState::load_with(&paths, None);
        assert!(state.is_unreleased(&id()));
        let migrated = std::fs::read_to_string(paths.hardware_state_file()).expect("read");
        assert!(!migrated.contains("machine ="), "{migrated}");
    }

    #[test]
    fn oversized_files_are_kept_and_not_written() {
        let big = |valid: &str| {
            let mut text = valid.to_owned();
            text.push_str(&"#".repeat(MAX_STATE_FILE_BYTES as usize));
            text
        };
        // An oversized machine-state file: not read, not moved, not written; device memory stays writable.
        let (_tmp, paths) = temp();
        let state_text = big("unreleased = [\"hid:1462:7d75:7D7522120106\"]\n");
        std::fs::write(paths.hardware_state_file(), &state_text).expect("write");
        let mut state = load(&paths);
        assert!(state.is_read_only());
        assert!(!state.is_unreleased(&id()));
        assert!(
            state.device_memory_writable(),
            "the machine-state file does not affect device memory"
        );
        assert!(state.record_own_lighting(&id(), &[1]).expect("kept in memory"));
        assert_eq!(
            std::fs::read_to_string(paths.hardware_state_file()).expect("read"),
            state_text
        );
        assert!(!paths.device_memory_file().exists());

        // An oversized device-memory file: kept, and device memory read-only.
        let (_tmp, paths) = temp();
        let memory_text = big("machine = \"machine-one\"\n");
        std::fs::write(paths.device_memory_file(), &memory_text).expect("write");
        let mut state = load(&paths);
        assert!(!state.is_read_only());
        assert!(!state.device_memory_writable());
        assert!(state.record_saved_backup(&id(), &[1]).is_err());
        assert_eq!(
            std::fs::read_to_string(paths.device_memory_file()).expect("read"),
            memory_text
        );

        // An oversized old roaming file: not migrated this session, device memory unaffected; migrated on a later
        // start once it can be read, while the new machine-state file is still missing.
        let (_tmp, paths, _) = roaming();
        let small = "unreleased = [\"hid:1462:7d75:7D7522120106\"]
[saved_backup]
\"hid:1462:7d75:7D7522120106\" = \"0a0b\"
";
        let old = big(small);
        std::fs::write(paths.legacy_hardware_state_file(), &old).expect("write");
        let mut state = load(&paths);
        assert!(!state.is_unreleased(&id()));
        assert!(!state.is_read_only());
        assert!(state.device_memory_writable());
        assert!(state.record_saved_backup(&id(), &[1]).expect("written"));
        assert!(!paths.hardware_state_file().exists());
        assert_eq!(
            std::fs::read_to_string(paths.legacy_hardware_state_file()).expect("read"),
            old
        );
        drop(state);
        std::fs::write(paths.legacy_hardware_state_file(), small).expect("write");
        let state = load(&paths);
        assert!(state.is_unreleased(&id()), "migrated on the later start");
        assert_eq!(
            state.saved_backup(&id()),
            Some(&[1u8][..]),
            "device memory is its own file's"
        );
        assert!(state.device_memory_writable());
    }
}
