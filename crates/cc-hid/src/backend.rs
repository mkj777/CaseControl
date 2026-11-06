//! The `hidapi` backend: enumeration and opening of real devices.
//!
//! One process-wide `hidapi::HidApi` lives in a `static Mutex<Option<..>>` and is created lazily,
//! under that lock, without an initial enumeration, so exactly one `HidApi` construction succeeds
//! even when several threads call [`enumerate`] or [`open`] at once. Every [`enumerate`] call
//! refreshes its device list. Devices are opened by path with `FILE_SHARE_READ | FILE_SHARE_WRITE`
//! (the `hidapi` default), never exclusively.

use crate::info::HidDeviceInfo;
use crate::transport::{HidError, HidTransport};

#[cfg(windows)]
mod imp {
    use std::ffi::CString;
    use std::sync::{Mutex, MutexGuard};
    use std::time::Duration;

    use hidapi::{HidApi, HidDevice};
    use windows::Win32::Foundation::{
        ERROR_DEVICE_NOT_CONNECTED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, WIN32_ERROR,
    };

    use super::*;

    /// The process-wide context. `None` until the first successful construction; a failed
    /// construction leaves it `None` so a later call can retry.
    static API: Mutex<Option<HidApi>> = Mutex::new(None);

    /// A locked, initialised context.
    struct ApiGuard(MutexGuard<'static, Option<HidApi>>);

    impl std::ops::Deref for ApiGuard {
        type Target = HidApi;
        fn deref(&self) -> &HidApi {
            self.0
                .as_ref()
                .expect("ApiGuard is only built around an initialised context")
        }
    }

    impl std::ops::DerefMut for ApiGuard {
        fn deref_mut(&mut self) -> &mut HidApi {
            self.0
                .as_mut()
                .expect("ApiGuard is only built around an initialised context")
        }
    }

    fn api() -> Result<ApiGuard, HidError> {
        // The guarded value is only a device list, safe to reuse after a panic elsewhere.
        let mut guard = API.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            // Construction happens under the lock, so exactly one `HidApi` is ever created even
            // when two threads race here.
            //
            // In hidapi 2.6.7 `new_without_enumerate()` flips the process-global device-discovery
            // flag before the first init (it is deprecated in favour of the explicit global
            // `disable_device_discovery()`). That is intended: `enumerate()` refreshes the list
            // itself, and cc-hid is the only hidapi user in the process.
            #[allow(deprecated)]
            let api = HidApi::new_without_enumerate().map_err(map_err)?;
            *guard = Some(api);
        }
        Ok(ApiGuard(guard))
    }

    fn os_error(e: &hidapi::HidError) -> Option<i32> {
        match e {
            hidapi::HidError::IoError { error } => error.raw_os_error(),
            _ => None,
        }
    }

    /// Whether `code` (a raw OS error) is the Win32 error `expected`.
    fn is_win32(code: Option<i32>, expected: WIN32_ERROR) -> bool {
        code.is_some() && code == i32::try_from(expected.0).ok()
    }

    pub(super) fn map_err(e: hidapi::HidError) -> HidError {
        if is_win32(os_error(&e), ERROR_DEVICE_NOT_CONNECTED) {
            HidError::Disconnected
        } else {
            HidError::Io(e.to_string())
        }
    }

    fn non_empty(s: Option<&str>) -> Option<String> {
        s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
    }

    pub fn enumerate() -> Result<Vec<HidDeviceInfo>, HidError> {
        let mut api = api()?;
        api.reset_devices().map_err(map_err)?;
        api.add_devices(0, 0).map_err(map_err)?;
        let list: Vec<HidDeviceInfo> = api
            .device_list()
            .map(|d| HidDeviceInfo {
                path: d.path().to_string_lossy().into_owned(),
                vid: d.vendor_id(),
                pid: d.product_id(),
                interface: d.interface_number(),
                usage_page: d.usage_page(),
                usage: d.usage(),
                serial: non_empty(d.serial_number()),
                product: non_empty(d.product_string()),
                manufacturer: non_empty(d.manufacturer_string()),
                release: d.release_number(),
            })
            .collect();
        tracing::debug!(target: "cc_hid", count = list.len(), "enumerated HID interfaces");
        Ok(list)
    }

    pub fn open(path: &str) -> Result<Box<dyn HidTransport>, HidError> {
        let c_path = CString::new(path).map_err(|_| HidError::NotFound(path.to_owned()))?;
        let api = api()?;
        let device = api.open_path(&c_path).map_err(|e| {
            let code = os_error(&e);
            if is_win32(code, ERROR_FILE_NOT_FOUND) || is_win32(code, ERROR_PATH_NOT_FOUND) {
                HidError::NotFound(path.to_owned())
            } else {
                map_err(e)
            }
        })?;
        drop(api);
        tracing::debug!(target: "cc_hid", path, "opened HID device (shared read/write)");
        Ok(Box::new(HidapiTransport { device }))
    }

    /// A device opened through `hidapi`. `Send`, not `Sync`: used from one thread.
    struct HidapiTransport {
        device: HidDevice,
    }

    fn timeout_ms(timeout: Duration) -> i32 {
        i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX)
    }

    impl HidTransport for HidapiTransport {
        fn write(&mut self, data: &[u8]) -> Result<usize, HidError> {
            // The windows-native backend returns Ok(0) when `WriteFile` completes synchronously,
            // so a success is normalised to the full report length.
            self.device.write(data).map_err(map_err)?;
            Ok(data.len())
        }

        fn read_timeout(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, HidError> {
            match self
                .device
                .read_timeout(buf, timeout_ms(timeout))
                .map_err(map_err)?
            {
                0 => Err(HidError::Timeout),
                n => Ok(n),
            }
        }

        fn send_feature_report(&mut self, data: &[u8]) -> Result<(), HidError> {
            self.device.send_feature_report(data).map_err(map_err)
        }

        fn get_feature_report(&mut self, buf: &mut [u8]) -> Result<usize, HidError> {
            self.device.get_feature_report(buf).map_err(map_err)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn timeout_is_capped() {
            assert_eq!(timeout_ms(Duration::from_millis(250)), 250);
            assert_eq!(timeout_ms(Duration::ZERO), 0);
            assert_eq!(timeout_ms(Duration::from_secs(u64::MAX)), i32::MAX);
        }

        #[test]
        fn disconnect_error_maps() {
            let raw = i32::try_from(ERROR_DEVICE_NOT_CONNECTED.0).expect("fits i32");
            let e = hidapi::HidError::IoError {
                error: std::io::Error::from_raw_os_error(raw),
            };
            assert_eq!(map_err(e), HidError::Disconnected);
            let other = hidapi::HidError::IoError {
                error: std::io::Error::from_raw_os_error(5),
            };
            assert!(matches!(map_err(other), HidError::Io(_)));
            assert!(!is_win32(None, ERROR_FILE_NOT_FOUND));
            assert!(is_win32(Some(2), ERROR_FILE_NOT_FOUND));
            assert!(is_win32(Some(3), ERROR_PATH_NOT_FOUND));
        }

        #[test]
        fn concurrent_first_use_is_safe() {
            // Enumerates only; never opens a device.
            let threads: Vec<_> = (0..4)
                .map(|_| std::thread::spawn(|| enumerate().map(|l| l.len())))
                .collect();
            for t in threads {
                t.join().expect("no panic").expect("enumerate returns Ok");
            }
        }

        #[test]
        fn enumerate_smoke() {
            // Enumerates only; never opens a device.
            let list = enumerate().expect("enumerate returns Ok");
            let again = enumerate().expect("second enumerate returns Ok");
            let _ = (list.len(), again.len());
        }

        #[test]
        fn open_bad_path_fails_without_panicking() {
            assert!(open("not\0a path").is_err());
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn enumerate() -> Result<Vec<HidDeviceInfo>, HidError> {
        Err(HidError::Unsupported(
            "HID enumeration is only implemented on Windows",
        ))
    }

    pub fn open(_path: &str) -> Result<Box<dyn HidTransport>, HidError> {
        Err(HidError::Unsupported("HID access is only implemented on Windows"))
    }
}

/// Enumerate every HID interface currently present (one fresh pass; callers cache per detection run).
///
/// Never opens a device. On non-Windows platforms returns [`HidError::Unsupported`].
pub fn enumerate() -> Result<Vec<HidDeviceInfo>, HidError> {
    imp::enumerate()
}

/// Open the HID interface at `path` for shared read/write access (never exclusive).
///
/// Timeouts passed to `read_timeout` are converted to milliseconds and capped at `i32::MAX`;
/// a read that returns no data within the timeout is [`HidError::Timeout`].
/// On non-Windows platforms returns [`HidError::Unsupported`].
pub fn open(path: &str) -> Result<Box<dyn HidTransport>, HidError> {
    imp::open(path)
}
