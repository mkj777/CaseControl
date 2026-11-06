//! The transport trait and its error type.

use std::fmt::Write as _;
use std::time::Duration;

/// Errors from any HID transport (real, fake or recording).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HidError {
    /// No device at the given path, or the path is malformed.
    #[error("HID device not found: {0}")]
    NotFound(String),
    /// Any other I/O or OS error, with the OS message.
    #[error("HID I/O error: {0}")]
    Io(String),
    /// A read did not produce data within the timeout.
    #[error("HID read timed out")]
    Timeout,
    /// The device went away (unplugged, driver reset).
    #[error("HID device disconnected")]
    Disconnected,
    /// A report had the wrong length.
    #[error("invalid HID report: expected {expected} bytes, got {got}")]
    InvalidReport {
        /// Length the caller or protocol expected.
        expected: usize,
        /// Length actually seen.
        got: usize,
    },
    /// The operation is not available on this platform or backend.
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    /// A [`FakeTransport`](crate::FakeTransport) script mismatch (tests only).
    #[error("fake transport script mismatch: {0}")]
    Script(String),
}

impl From<HidError> for cc_core::DeviceError {
    fn from(e: HidError) -> Self {
        match e {
            HidError::Disconnected => cc_core::DeviceError::Disconnected,
            HidError::Timeout => cc_core::DeviceError::Timeout,
            other => cc_core::DeviceError::Transport(other.to_string()),
        }
    }
}

/// Byte-level access to one opened HID interface.
///
/// In every buffer byte 0 is the report id (0 for devices without numbered reports).
/// Implementations are `Send` but not required to be `Sync`: one controller thread owns the
/// transport and is the only caller.
pub trait HidTransport: Send {
    /// Writes an output report; `data[0]` is the report id.
    ///
    /// On success the returned count is always the full report length (`data.len()`); a partial
    /// write is reported as an error, never as a short count.
    fn write(&mut self, data: &[u8]) -> Result<usize, HidError>;
    /// Reads one input report into `buf`, waiting at most `timeout`. Returns the number of bytes
    /// read; no data within the timeout is [`HidError::Timeout`].
    fn read_timeout(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, HidError>;
    /// Sends a feature report; `data[0]` is the report id.
    fn send_feature_report(&mut self, data: &[u8]) -> Result<(), HidError>;
    /// Gets a feature report. `buf[0]` holds the requested report id on entry; on return `buf`
    /// holds the report including the id byte. Returns the number of bytes filled.
    fn get_feature_report(&mut self, buf: &mut [u8]) -> Result<usize, HidError>;
}

impl<T: HidTransport + ?Sized> HidTransport for Box<T> {
    fn write(&mut self, data: &[u8]) -> Result<usize, HidError> {
        (**self).write(data)
    }
    fn read_timeout(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, HidError> {
        (**self).read_timeout(buf, timeout)
    }
    fn send_feature_report(&mut self, data: &[u8]) -> Result<(), HidError> {
        (**self).send_feature_report(data)
    }
    fn get_feature_report(&mut self, buf: &mut [u8]) -> Result<usize, HidError> {
        (**self).get_feature_report(buf)
    }
}

/// Formats bytes as lower-case hex pairs separated by spaces (`"01 ff 00"`).
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        // Writing into a String cannot fail.
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_format() {
        assert_eq!(hex(&[]), "");
        assert_eq!(hex(&[0x01, 0xff, 0x00]), "01 ff 00");
    }

    #[test]
    fn device_error_mapping() {
        use cc_core::DeviceError;
        assert!(matches!(
            DeviceError::from(HidError::Disconnected),
            DeviceError::Disconnected
        ));
        assert!(matches!(
            DeviceError::from(HidError::Timeout),
            DeviceError::Timeout
        ));
        assert!(matches!(
            DeviceError::from(HidError::Io("boom".into())),
            DeviceError::Transport(m) if m.contains("boom")
        ));
        assert!(matches!(
            DeviceError::from(HidError::InvalidReport { expected: 2, got: 1 }),
            DeviceError::Transport(_)
        ));
        assert!(matches!(
            DeviceError::from(HidError::NotFound("p".into())),
            DeviceError::Transport(_)
        ));
    }
}
