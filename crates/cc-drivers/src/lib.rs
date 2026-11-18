//! CaseControl device drivers: MSI Mystic Light, Wooting, Logitech, NVIDIA Illumination.
//!
//! One module per device family, each behind its own cargo feature and each exposing `pub fn backend() -> Box<dyn
//! Backend>`. Packet builders are pure functions tested byte-exact against transcripts derived from the OpenRGB
//! reference in `reference/openrgb/`. This crate does not schedule frames, retry, or own threads; that is
//! `cc-engine`'s job.

use cc_core::Backend;

#[cfg(feature = "logitech")]
pub mod logitech;
#[cfg(feature = "msi_mystic_light")]
pub mod msi_mystic_light;
#[cfg(feature = "nvidia_illumination")]
pub mod nvidia_illumination;
#[cfg(feature = "wooting")]
pub mod wooting;

/// Every backend compiled into this build, in detection order.
#[allow(clippy::vec_init_then_push)]
pub fn all_backends() -> Vec<Box<dyn Backend>> {
    #[allow(unused_mut)]
    let mut out: Vec<Box<dyn Backend>> = Vec::new();
    #[cfg(feature = "msi_mystic_light")]
    out.push(msi_mystic_light::backend());
    #[cfg(feature = "wooting")]
    out.push(wooting::backend());
    #[cfg(feature = "logitech")]
    out.push(logitech::backend());
    #[cfg(feature = "nvidia_illumination")]
    out.push(nvidia_illumination::backend());
    out
}
