#![allow(unsafe_code)]
//! CaseControl HID transport layer.
//!
//! What this crate does:
//!
//! - [`HidTransport`]: the byte-level trait every HID driver talks to (`write`, `read_timeout`,
//!   `send_feature_report`, `get_feature_report`; byte 0 of every buffer is the report id).
//! - [`enumerate`] and [`open`]: the `hidapi` backend (`windows-native`, pure Rust). On other
//!   platforms both return [`HidError::Unsupported`].
//! - [`HidDeviceInfo`] and [`HidMatch`]: enumeration results and the vid/pid/interface/usage
//!   matcher modelled on OpenRGB's `REGISTER_HID_DETECTOR*` macros.
//! - [`FakeTransport`]: a scripted, byte-exact transport for driver tests without hardware.
//! - [`RecordingTransport`]: wraps any transport and records every exchange, so transcripts from
//!   real devices can be turned into fake scripts ([`Record::to_exchange`]).
//! - `hotplug` (Windows): debounced HID arrival/removal notifications via `CM_Register_Notification`.
//!   Its event paths are lower-cased while [`HidDeviceInfo::path`] keeps the mixed case `hidapi`
//!   returns; Windows device paths are case-insensitive, so compare them with
//!   `eq_ignore_ascii_case` and pass [`HidDeviceInfo::path`] to [`open`] unchanged.
//! - `mutex` (Windows): named-mutex guard for the shared bus mutexes other tools use.
//! - `power` (Windows): suspend and resume notifications via `PowerRegisterSuspendResumeNotification`, so the
//!   engine can re-send device state after the PC wakes (devices fall back to their firmware state in sleep).
//!
//! # Shared access, never exclusive
//!
//! Devices are never opened exclusively. `hidapi` opens every path with
//! `FILE_SHARE_READ | FILE_SHARE_WRITE`, which is what lets CaseControl run next to FanControl,
//! LibreHardwareMonitor and vendor tools that keep the same HID interface open
//! (see `docs/research/02-fancontrol-coexistence.md`). Nothing in this crate requests exclusive access.
//!
//! # Error behaviour
//!
//! - `hidapi`'s windows-native backend waits at most 1000 ms for an overlapped write to finish; a
//!   write that exceeds that internal wait surfaces as [`HidError::Io`], not
//!   [`HidError::Timeout`] (which is reserved for reads).
//! - `ERROR_DEVICE_NOT_CONNECTED` maps to [`HidError::Disconnected`]. Which further Win32 error
//!   codes an unplug produces mid-write or mid-read is confirmed during the hardware checklist;
//!   until then they surface as [`HidError::Io`].
//!
//! # Not in scope
//!
//! No device protocols (those live in `cc-drivers`), no detection policy or caching across runs,
//! no `get_input_report` (the `windows-native` backend lacks it and no driver needs it), no SMBus
//! or port I/O, and no network access.

mod backend;
pub mod fake;
#[cfg(windows)]
pub mod hotplug;
pub mod info;
#[cfg(windows)]
pub mod mutex;
#[cfg(windows)]
pub mod power;
pub mod recording;
pub mod transport;

pub use backend::{enumerate, open};
pub use fake::{Exchange, FakeTransport, ScriptBuilder};
pub use info::{HidDeviceInfo, HidMatch};
pub use recording::{Record, RecordKind, RecordLog, RecordingTransport};
pub use transport::{HidError, HidTransport, hex};
