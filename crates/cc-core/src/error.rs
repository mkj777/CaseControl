//! Error types: runtime device errors and model validation errors.

use serde::{Deserialize, Serialize};

use crate::mode::Direction;

/// An error from a device operation.
///
/// Serializable so it can cross the IPC boundary; all payloads are plain strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum DeviceError {
    /// The device or driver does not support the operation.
    #[error("operation not supported by this device")]
    Unsupported,
    /// An argument was rejected before anything was sent to the device.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The transport (HID, NvAPI, ...) reported an error.
    #[error("transport error: {0}")]
    Transport(String),
    /// The device is gone.
    #[error("device disconnected")]
    Disconnected,
    /// The device did not answer in time.
    #[error("device timed out")]
    Timeout,
    /// The device answered with something the protocol does not allow.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// The device is in a state where the driver must stop talking to it.
    #[error("device faulted: {0}")]
    Faulted(String),
    /// A safety gate refused a write to the device's own memory (ADR 0001 #42) before anything reached that memory.
    /// The device may have been sent a volatile state on the way (and is driven again afterwards); its memory is
    /// unchanged, so the write does not count toward the daily cap.
    #[error("refused: {0}")]
    Refused(String),
}

impl DeviceError {
    /// Whether the controller must be taken out of service (`Disconnected`, `Faulted`). Other errors may be retried.
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Disconnected | Self::Faulted(_))
    }
}

/// A violation of the device model's invariants, found by
/// [`DeviceDescription::validate`](crate::DeviceDescription::validate) or one of the `validate_state` methods.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// Zones are not sorted by `start`.
    #[error(
        "zone {zone} starts at {start}, before zone {prev} which starts at {prev_start}: zones must be sorted by start"
    )]
    ZoneUnsorted {
        /// Index of the offending zone.
        zone: usize,
        /// Its start.
        start: usize,
        /// Index of the preceding zone.
        prev: usize,
        /// The preceding zone's start.
        prev_start: usize,
    },
    /// A zone starts after the previous zone ended, leaving LEDs uncovered.
    #[error("gap before zone {zone}: it starts at {start} but the previous zone ends at {expected}")]
    ZoneGap {
        /// Index of the offending zone.
        zone: usize,
        /// Its start.
        start: usize,
        /// Where it should start.
        expected: usize,
    },
    /// A zone starts before the previous zone ended.
    #[error(
        "zone {zone} overlaps the previous zone: it starts at {start} but the previous zone ends at {expected}"
    )]
    ZoneOverlap {
        /// Index of the offending zone.
        zone: usize,
        /// Its start.
        start: usize,
        /// Where it should start.
        expected: usize,
    },
    /// A zone extends past the end of the LED buffer.
    #[error("zone {zone} ends at {end}, past the LED count {led_count}")]
    ZonePastEnd {
        /// Index of the offending zone.
        zone: usize,
        /// Its end (exclusive).
        end: usize,
        /// Number of LEDs.
        led_count: usize,
    },
    /// The zones end before the LED buffer does.
    #[error("zones cover LEDs 0..{covered} but the device has {led_count} LEDs")]
    ZonesIncomplete {
        /// End of the covered range.
        covered: usize,
        /// Number of LEDs.
        led_count: usize,
    },
    /// A matrix map does not have `width * height` cells.
    #[error("matrix zone {zone} is {width}x{height} but its map has {cells} cells")]
    MatrixMapSize {
        /// Index of the zone.
        zone: usize,
        /// Matrix width.
        width: u32,
        /// Matrix height.
        height: u32,
        /// Actual number of cells.
        cells: usize,
    },
    /// A matrix map cell points past the zone's LEDs.
    #[error("matrix zone {zone}: cell {cell} maps to LED {index}, but the zone has {len} LEDs")]
    MatrixIndexOutOfRange {
        /// Index of the zone.
        zone: usize,
        /// Index of the cell in the map (row-major).
        cell: usize,
        /// The zone-relative LED index in the cell.
        index: u32,
        /// Zone length.
        len: usize,
    },
    /// Two modes share an id.
    #[error("duplicate mode id {id} (modes {first} and {second})")]
    DuplicateModeId {
        /// The duplicated id.
        id: u32,
        /// Index of the first mode with this id.
        first: usize,
        /// Index of the second mode with this id.
        second: usize,
    },
    /// A resizable zone's length lies outside its `resizable` range.
    #[error("zone {zone} has {len} LEDs, outside its resizable range {min}..={max}")]
    LenOutsideResizable {
        /// Index of the zone.
        zone: usize,
        /// Its length.
        len: usize,
        /// Lowest allowed length.
        min: usize,
        /// Highest allowed length.
        max: usize,
    },
    /// A mode's color mode lacks its matching capability (`PerLed` needs `PER_LED_COLOR`, `ModeSpecific` needs
    /// `MODE_SPECIFIC_COLOR`, `Random` needs `RANDOM_COLOR`).
    #[error("mode {mode}: color mode lacks its matching capability")]
    ColorModeCapMismatch {
        /// The mode's id.
        mode: u32,
    },
    /// A mode id does not name an existing mode.
    #[error("unknown mode id {id}")]
    UnknownMode {
        /// The id that was looked up.
        id: u32,
    },
    /// `direct_mode` names a mode whose color mode is not per-LED.
    #[error("direct mode {id} must have color mode PerLed")]
    DirectModeNotPerLed {
        /// The direct mode's id.
        id: u32,
    },
    /// A mode-specific-color mode has no `colors` range.
    #[error("mode {mode} uses mode-specific colors but declares no color count range")]
    MissingColorRange {
        /// The mode's id.
        mode: u32,
    },
    /// A mode state was checked against a different mode.
    #[error("state is for mode {state_mode}, checked against mode {mode}")]
    ModeMismatch {
        /// The mode the state was checked against.
        mode: u32,
        /// The mode the state names.
        state_mode: u32,
    },
    /// A state sets a parameter the mode does not support.
    #[error("mode {mode} does not support {param}")]
    ParamUnsupported {
        /// The mode's id.
        mode: u32,
        /// `"speed"` or `"brightness"`.
        param: &'static str,
    },
    /// A state sets a parameter outside the mode's range.
    #[error("mode {mode}: {param} {value} outside {min}..={max}")]
    ParamOutOfRange {
        /// The mode's id.
        mode: u32,
        /// `"speed"` or `"brightness"`.
        param: &'static str,
        /// The rejected value.
        value: u32,
        /// Lowest allowed value.
        min: u32,
        /// Highest allowed value.
        max: u32,
    },
    /// A state sets a direction the mode's capabilities do not allow.
    #[error("mode {mode} does not support direction {direction:?}")]
    DirectionUnsupported {
        /// The mode's id.
        mode: u32,
        /// The rejected direction.
        direction: Direction,
    },
    /// A state carries the wrong number of colors.
    #[error("mode {mode}: {actual} colors given, expected {min}..={max}")]
    ColorCount {
        /// The mode's id.
        mode: u32,
        /// Number of colors in the state.
        actual: usize,
        /// Lowest allowed count.
        min: usize,
        /// Highest allowed count.
        max: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fatal_errors() {
        assert!(DeviceError::Disconnected.is_fatal());
        assert!(DeviceError::Faulted("x".into()).is_fatal());
        for e in [
            DeviceError::Unsupported,
            DeviceError::InvalidArgument("x".into()),
            DeviceError::Transport("x".into()),
            DeviceError::Timeout,
            DeviceError::Protocol("x".into()),
            DeviceError::Refused("x".into()),
        ] {
            assert!(!e.is_fatal(), "{e}");
        }
    }
}
