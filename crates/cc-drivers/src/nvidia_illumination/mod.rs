// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
//! NVIDIA Illumination V1: GPU lighting through NvAPI's undocumented client illumination zones, ported from OpenRGB's
//! `NVIDIAIlluminationController` (target: GeForce RTX 5080 Founders Edition, `10DE:2C02` subsystem `10DE:2095`).
//!
//! # What it does
//!
//! - Detection loads `nvapi64.dll` through [`cc_nvapi::NvApi::load`] (no driver: logs at info, finds nothing),
//!   matches every physical GPU's PCI ids against the reference's V1 table ([`table::SUPPORTED`]) and, for each
//!   match not in `ctx.skip`, reads the current zone control (`GetControl`, active values) and keeps it.
//! - One single-LED zone per NvAPI zone, modes "Off" and "Direct" (per-zone colour plus brightness 0..=100), and the
//!   reference's colour mapping per zone type ([`mapping`]): RGB as RGB, RGBW as RGBW with the reference's
//!   near-grey-to-white rule, SINGLE_COLOR / COLOR_FIXED as brightness scaled by the colour's value (a deviation, see [`mapping::direct_color`]).
//!   SINGLE_COLOR / COLOR_FIXED zones are described as [`brightness_only`](cc_core::Zone::brightness_only): they
//!   show their own fixed colour (white on the RTX 5080 FE) and the UI offers no colour for them.
//! - Direct colours (`set_direct`, `set_zone_direct`) are written at the full mode brightness, so each zone's
//!   brightness comes from its colour alone (per-LED white levels, ADR 0001 #41; the engine applies device and master
//!   brightness to the colours). Only a "Direct" mode state (`set_mode`) carries a brightness of its own, for that
//!   write only. Deviation from the reference, whose direct writes keep the last mode brightness.
//! - Each write sends the whole control struct once and is skipped when identical to the last one sent.
//!   `release()` writes the kept params back; the next write after a release reads `GetControl` again first.
//!
//! # What it deliberately does not do
//!
//! - No `SetControl` before a successful `GetControl`, no writes during detection, no default-values reset.
//! - No hardware effects (the reference exposes none for V1), no save to hardware, no hot-plug (GPUs are fixed).
//! - No pacing of its own: [`cc_nvapi::NvApi`] already spaces illumination calls by [`cc_nvapi::ILLUM_CALL_GAP`],
//!   sleeping only the remainder of the gap.
//!
//! # Threading
//!
//! All controllers from one detection pass share one `NvApi` behind `Arc<Mutex<_>>` ([`NvApiGpu`]): `NvApi` is
//! `Send + !Sync` and a GPU handle is only valid for the instance that enumerated it. The mutex is only contended
//! with more than one supported GPU.

use std::sync::{Arc, Mutex};

use cc_core::{Backend, Bus, DetectContext, Detected, DeviceId, HotplugHandle, HotplugSender, family};
use cc_nvapi::{GpuHandle, NvApi, PciIds};

mod controller;
pub mod mapping;
pub mod table;

pub use controller::{
    IllumApi, IllumController, MODE_DIRECT, MODE_OFF, NvApiGpu, describe, is_brightness_only,
};
pub use table::{SUPPORTED, SupportedGpu, lookup};

/// The backend for this family.
pub fn backend() -> Box<dyn Backend> {
    Box::new(NvidiaIlluminationBackend)
}

/// Detects Illumination V1 GPUs through NvAPI.
#[derive(Debug, Default, Clone, Copy)]
pub struct NvidiaIlluminationBackend;

/// Location string for the GPU at NvAPI enumeration index `index`. `cc-nvapi` exposes no PCI bus location.
#[must_use]
pub fn location(index: usize) -> String {
    format!("NvAPI GPU {index}")
}

/// Device ids for a list of matched GPUs, in enumeration order: [`DeviceId::nvapi`], plus an ordinal `:#n` on every
/// GPU whose ids occur more than once (identical cards have no serial to tell them apart).
#[must_use]
pub fn assign_ids(ids: &[PciIds]) -> Vec<DeviceId> {
    let key = |p: &PciIds| (p.device_id, p.subsystem_id);
    ids.iter()
        .enumerate()
        .map(|(i, p)| {
            let id = DeviceId::nvapi(p.device_id, p.subsystem_id);
            let total = ids.iter().filter(|q| key(q) == key(p)).count();
            if total > 1 {
                let ordinal = ids[..i].iter().filter(|q| key(q) == key(p)).count();
                id.with_ordinal(u32::try_from(ordinal).unwrap_or(u32::MAX))
            } else {
                id
            }
        })
        .collect()
}

struct Candidate {
    index: usize,
    gpu: GpuHandle,
    ids: PciIds,
    entry: &'static SupportedGpu,
}

impl Backend for NvidiaIlluminationBackend {
    fn name(&self) -> &'static str {
        family::NVIDIA_ILLUMINATION
    }

    fn bus(&self) -> Bus {
        Bus::NvApi
    }

    fn detect(&self, ctx: &DetectContext) -> Vec<Detected> {
        if !ctx.is_enabled(family::NVIDIA_ILLUMINATION) {
            return Vec::new();
        }
        let nvapi = match NvApi::load() {
            Ok(n) => n,
            Err(e) => {
                tracing::info!(error = %e, "NvAPI not available, skipping NVIDIA Illumination detection");
                return Vec::new();
            }
        };
        let gpus = match nvapi.physical_gpus() {
            Ok(g) => g,
            Err(e) => {
                tracing::info!(error = %e, "NvAPI GPU enumeration failed");
                return Vec::new();
            }
        };

        let candidates: Vec<Candidate> = gpus
            .into_iter()
            .enumerate()
            .filter_map(|(index, gpu)| {
                let ids = nvapi
                    .pci_ids(&gpu)
                    .inspect_err(|e| tracing::debug!(index, error = %e, "NvAPI_GPU_GetPCIIdentifiers failed"))
                    .ok()?;
                let Some(entry) = lookup(&ids) else {
                    tracing::debug!(
                        index,
                        "GPU {:04x}:{:04x} subsys {:04x}:{:04x} is not an Illumination V1 card",
                        ids.vendor(),
                        ids.device(),
                        ids.subsystem_vendor(),
                        ids.subsystem_device()
                    );
                    return None;
                };
                Some(Candidate {
                    index,
                    gpu,
                    ids,
                    entry,
                })
            })
            .collect();
        if candidates.is_empty() {
            return Vec::new();
        }

        let device_ids = assign_ids(&candidates.iter().map(|c| c.ids).collect::<Vec<_>>());
        let shared = Arc::new(Mutex::new(nvapi));
        let mut out = Vec::new();
        for (c, id) in candidates.into_iter().zip(device_ids) {
            if ctx.is_skipped(&id) {
                tracing::debug!(%id, "already controlled, not touching it");
                continue;
            }
            let location = location(c.index);
            let mut api = NvApiGpu::new(Arc::clone(&shared), c.gpu);
            let opened = api
                .get_control()
                .map_err(controller::device_error)
                .and_then(|current| {
                    IllumController::new_with_stored(
                        api,
                        id.clone(),
                        c.entry.name,
                        location.clone(),
                        c.entry.treats_rgbw_as_rgb,
                        current,
                        ctx.stored_own_lighting(&id),
                        ctx.is_unreleased(&id),
                    )
                });
            match opened {
                Ok(ctrl) => {
                    tracing::info!(%id, name = c.entry.name, %location, "NVIDIA Illumination GPU found");
                    out.push(Detected::Device(Box::new(ctrl)));
                }
                Err(error) => {
                    tracing::warn!(%id, %location, %error, "NVIDIA Illumination GPU could not be opened");
                    out.push(Detected::Failed {
                        family: family::NVIDIA_ILLUMINATION.into(),
                        location,
                        error,
                    });
                }
            }
        }
        out
    }

    fn subscribe_hotplug(&self, _tx: HotplugSender) -> Option<Box<dyn HotplugHandle>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(device_id: u32, subsystem_id: u32) -> PciIds {
        PciIds {
            device_id,
            subsystem_id,
            revision_id: 0,
            ext_device_id: 0,
        }
    }

    #[test]
    fn single_gpu_has_no_ordinal() {
        let got = assign_ids(&[ids(0x2C02_10DE, 0x2095_10DE)]);
        assert_eq!(got[0].as_str(), "nvapi:10de:2c02:10de:2095");
    }

    #[test]
    fn identical_gpus_get_ordinals_in_enumeration_order() {
        let got = assign_ids(&[
            ids(0x2C02_10DE, 0x2095_10DE),
            ids(0x2B85_10DE, 0x2057_10DE),
            ids(0x2C02_10DE, 0x2095_10DE),
        ]);
        let got: Vec<_> = got.iter().map(DeviceId::as_str).collect();
        assert_eq!(
            got,
            [
                "nvapi:10de:2c02:10de:2095:#0",
                "nvapi:10de:2b85:10de:2057",
                "nvapi:10de:2c02:10de:2095:#1",
            ]
        );
    }

    #[test]
    fn location_format() {
        assert_eq!(location(0), "NvAPI GPU 0");
    }

    #[test]
    fn backend_identity_and_disabled_family() {
        let b = backend();
        assert_eq!(b.name(), "nvidia_illumination");
        assert_eq!(b.bus(), Bus::NvApi);
        // Disabled family: returns before touching NvAPI (no DLL load in tests).
        let ctx = DetectContext {
            disabled_families: [family::NVIDIA_ILLUMINATION.to_string()].into(),
            ..Default::default()
        };
        assert!(b.detect(&ctx).is_empty());
        let (tx, _rx) = std::sync::mpsc::channel();
        assert!(b.subscribe_hotplug(tx).is_none());
    }
}
