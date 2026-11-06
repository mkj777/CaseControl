//! Error type of the crate.

use crate::raw::NvStatus;

/// Everything that can go wrong talking to NvAPI.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NvApiError {
    /// `nvapi64.dll` could not be loaded: no NVIDIA driver is installed (or not in `System32`).
    #[error("nvapi64.dll could not be loaded (no NVIDIA display driver installed?)")]
    LibraryNotFound,
    /// `nvapi_QueryInterface` returned null for this function id: the installed driver does not provide it.
    #[error("NvAPI function id {0:#010x} is not provided by the installed driver")]
    SymbolNotFound(u32),
    /// An NvAPI call returned a status other than `NVAPI_OK`.
    #[error("{call} failed with {name} ({code})")]
    Status {
        /// Raw `NV_STATUS` value.
        code: i32,
        /// Symbolic name from [`NvStatus::name`].
        name: &'static str,
        /// The NvAPI function that failed, e.g. `"NvAPI_GPU_ClientIllumZonesGetControl"`.
        call: &'static str,
    },
    /// The platform, the driver's data or the requested change is not supported by this binding.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl NvApiError {
    /// Builds [`NvApiError::Status`] from a status and the failing call's name.
    #[must_use]
    pub fn status(status: NvStatus, call: &'static str) -> Self {
        Self::Status {
            code: status.0,
            name: status.name(),
            call,
        }
    }
}

/// `Ok(())` for `NVAPI_OK`, otherwise [`NvApiError::Status`].
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn check(status: NvStatus, call: &'static str) -> Result<(), NvApiError> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(NvApiError::status(status, call))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_error_carries_name_and_call() {
        let err = check(NvStatus(-9), "NvAPI_GPU_ClientIllumZonesGetControl").expect_err("not ok");
        assert_eq!(
            err,
            NvApiError::Status {
                code: -9,
                name: "NVAPI_INCOMPATIBLE_STRUCT_VERSION",
                call: "NvAPI_GPU_ClientIllumZonesGetControl",
            }
        );
        assert_eq!(
            err.to_string(),
            "NvAPI_GPU_ClientIllumZonesGetControl failed with NVAPI_INCOMPATIBLE_STRUCT_VERSION (-9)"
        );
        assert!(check(NvStatus::OK, "x").is_ok());
        assert_eq!(
            NvApiError::SymbolNotFound(0x3DBF_5764).to_string(),
            "NvAPI function id 0x3dbf5764 is not provided by the installed driver"
        );
    }
}
