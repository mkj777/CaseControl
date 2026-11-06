//! Non-Windows stand-in for [`NvApi`]: same API, `load()` always returns
//! [`NvApiError::Unsupported`], and no value of the type can exist.

use crate::error::NvApiError;
use crate::model::{GpuHandle, PciIds, ZoneControlParams, ZoneInfo};

/// NvAPI is Windows-only in this project; on other platforms this type is uninhabited.
#[derive(Debug)]
pub struct NvApi {
    never: core::convert::Infallible,
}

impl NvApi {
    /// Always fails with [`NvApiError::Unsupported`].
    ///
    /// # Errors
    /// Always.
    pub fn load() -> Result<Self, NvApiError> {
        Err(NvApiError::Unsupported(
            "NvAPI is only available on Windows".into(),
        ))
    }

    /// Unreachable: no `NvApi` exists on this platform.
    ///
    /// # Errors
    /// Never returns.
    pub fn physical_gpus(&self) -> Result<Vec<GpuHandle>, NvApiError> {
        match self.never {}
    }

    /// Unreachable: no `NvApi` exists on this platform.
    ///
    /// # Errors
    /// Never returns.
    pub fn pci_ids(&self, _gpu: &GpuHandle) -> Result<PciIds, NvApiError> {
        match self.never {}
    }

    /// Unreachable: no `NvApi` exists on this platform.
    ///
    /// # Errors
    /// Never returns.
    pub fn full_name(&self, _gpu: &GpuHandle) -> Result<String, NvApiError> {
        match self.never {}
    }

    /// Unreachable: no `NvApi` exists on this platform.
    ///
    /// # Errors
    /// Never returns.
    pub fn illum_zones_info(&self, _gpu: &GpuHandle) -> Result<Vec<ZoneInfo>, NvApiError> {
        match self.never {}
    }

    /// Unreachable: no `NvApi` exists on this platform.
    ///
    /// # Errors
    /// Never returns.
    pub fn illum_zones_get_control(
        &self,
        _gpu: &GpuHandle,
        _default_values: bool,
    ) -> Result<ZoneControlParams, NvApiError> {
        match self.never {}
    }

    /// Unreachable: no `NvApi` exists on this platform.
    ///
    /// # Errors
    /// Never returns.
    pub fn illum_zones_set_control(
        &self,
        _gpu: &GpuHandle,
        _params: &ZoneControlParams,
    ) -> Result<(), NvApiError> {
        match self.never {}
    }
}
