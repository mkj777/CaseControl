//! The engine's error type.

use cc_core::{DeviceError, DeviceId};

/// Why an engine call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// No device with this id is under control.
    #[error("unknown device {0}")]
    UnknownDevice(DeviceId),
    /// The call was rejected before anything was sent to the device (wrong length, bad zone, invalid mode state).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The device thread did not answer within the acknowledgement timeout.
    #[error("device did not answer in time")]
    Timeout,
    /// The device was taken out of service after a fatal error or too many consecutive failures.
    #[error("device {id} is faulted: {error}")]
    Faulted {
        /// The device.
        id: DeviceId,
        /// The error that faulted it.
        error: String,
    },
    /// The driver reported an error.
    #[error(transparent)]
    Device(#[from] DeviceError),
    /// The engine has been shut down.
    #[error("engine is shut down")]
    ShutDown,
    /// A manual save did not write the device's memory, for a reason that is not a device error (no backup, daily
    /// cap, writes switched off, a safety gate).
    #[error("not saved to the device: {0}")]
    NotSaved(String),
    /// A worker thread could not be started.
    #[error("could not start a device thread: {0}")]
    Spawn(String),
}
