//! Saving a look to a device's own memory (ADR 0001 #34, #42): what a driver can save, what it is asked to save and
//! what came of it. See the persistence methods of [`Controller`](crate::Controller).
//!
//! The engine decides what to save (one device-agnostic setting, never on live edits, a backup of the device's
//! original saved state before the first write); the driver decides how, and enforces its device's safety gates.

use serde::{Deserialize, Serialize};

use crate::color::Rgb;
use crate::mode::ModeState;

/// What a driver can save to its device's memory ([`Controller::persist_caps`](crate::Controller::persist_caps)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PersistCaps {
    /// One colour per zone ([`PersistLook::Zones`]), and the device's off state ([`PersistLook::Off`]).
    pub zone_colors: bool,
    /// Hardware modes ([`PersistLook::Mode`]), and the device's off state ([`PersistLook::Off`]).
    pub hardware_modes: bool,
}

/// The look to save, already at the brightness the device shows (ADR 0001 #42: brightness baked in).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PersistLook {
    /// One colour per zone of the description, in zone order: a zone's colours averaged in linear light
    /// ([`average_linear`](crate::color::average_linear)) and scaled by the brightness. `None` leaves the zone as
    /// the device has it stored (hidden and empty zones).
    Zones(Vec<Option<Rgb>>),
    /// A hardware mode exactly as the device is sent it (its brightness parameter already scaled).
    Mode(ModeState),
    /// The device's off state.
    Off,
}

/// What [`Controller::persist_look`](crate::Controller::persist_look) did.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PersistOutcome {
    /// The look was written to the device's memory. `fingerprint` is driver-opaque; passed back as `last` on the
    /// next call, it lets the driver skip a write of the same look.
    Written {
        /// Identifies what was written.
        fingerprint: Vec<u8>,
    },
    /// The look equals the one written last (`last`): nothing was sent.
    Unchanged,
}
