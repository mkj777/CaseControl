// Modified port of OpenRGB code (https://gitlab.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later; see NOTICE.md.
//! The [`Controller`] for a Wooting V3 keyboard.

use std::time::Duration;

use cc_core::{Controller, DeviceDescription, DeviceError, Rgb};
use cc_hid::HidTransport;

use super::protocol;

/// Pause after the colour-init command, as in `WootingKeyboardController::SendInitialize` (50 ms).
const INIT_DELAY: Duration = Duration::from_millis(50);

/// Drives one Wooting V3 keyboard over its 0xFF55 configuration interface.
///
/// Software control starts lazily: the first [`set_direct`](Controller::set_direct) or
/// [`set_mode`](Controller::set_mode) after creation or after [`release`](Controller::release) sends the
/// colour-init command, then frames follow. Release sends "reset all", which makes the keyboard show its active
/// profile's lighting again. Nothing here touches key mappings, actuation or stored profiles.
pub struct WootingController {
    transport: Box<dyn HidTransport>,
    desc: DeviceDescription,
    /// Hardware index of each LED in `desc.leds` order.
    hw_index: Vec<u8>,
    /// Reused colour frame; LED slots are overwritten every frame, the rest stays zero.
    frame: Vec<u8>,
    /// Whether the colour-init command has been sent since creation or the last release.
    controlling: bool,
    init_delay: Duration,
}

impl std::fmt::Debug for WootingController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WootingController")
            .field("id", &self.desc.info.id)
            .field("controlling", &self.controlling)
            .finish_non_exhaustive()
    }
}

impl WootingController {
    /// Wraps an opened transport. Sends nothing.
    pub fn new(transport: Box<dyn HidTransport>, desc: DeviceDescription, hw_index: Vec<u8>) -> Self {
        debug_assert_eq!(desc.leds.len(), hw_index.len());
        Self {
            transport,
            desc,
            hw_index,
            frame: protocol::empty_frame(),
            controlling: false,
            init_delay: INIT_DELAY,
        }
    }

    #[cfg(test)]
    pub(super) fn without_delay(mut self) -> Self {
        self.init_delay = Duration::ZERO;
        self
    }

    /// Sends the colour-init command once per take-over.
    ///
    /// Like the reference, a failed init is logged and not fatal: OpenRGB ignores its result and frames work
    /// without it.
    fn take_control(&mut self) {
        if self.controlling {
            return;
        }
        match self.transport.send_feature_report(&protocol::color_init_packet()) {
            Ok(()) => {
                tracing::debug!(target: "cc_drivers::wooting", id = %self.desc.info.id, "colour init sent")
            }
            Err(e) => tracing::warn!(
                target: "cc_drivers::wooting",
                id = %self.desc.info.id,
                error = %e,
                "colour init failed; continuing like the reference"
            ),
        }
        if !self.init_delay.is_zero() {
            std::thread::sleep(self.init_delay);
        }
        self.controlling = true;
    }

    fn send_frame(&mut self, colors: &[Rgb]) -> Result<(), DeviceError> {
        if colors.len() != self.hw_index.len() {
            return Err(DeviceError::InvalidArgument(format!(
                "expected {} colors, got {}",
                self.hw_index.len(),
                colors.len()
            )));
        }
        self.take_control();
        protocol::fill_frame(&mut self.frame, &self.hw_index, colors);
        self.transport.write(&self.frame)?;
        Ok(())
    }
}

impl Controller for WootingController {
    fn description(&self) -> &DeviceDescription {
        &self.desc
    }

    fn set_direct(&mut self, colors: &[Rgb]) -> Result<(), DeviceError> {
        self.send_frame(colors)
    }

    fn set_zone_direct(&mut self, zone: usize, colors: &[Rgb]) -> Result<(), DeviceError> {
        // One zone covering every LED; the reference's DeviceUpdateZoneLEDs sends the full frame too.
        if zone != 0 {
            return Err(DeviceError::InvalidArgument(format!("no zone {zone}")));
        }
        self.send_frame(colors)
    }

    fn set_mode(&mut self, state: &cc_core::ModeState) -> Result<(), DeviceError> {
        self.desc
            .validate_state(state)
            .map_err(|e| DeviceError::InvalidArgument(e.to_string()))?;
        // Direct is the only mode; its state carries the full LED buffer.
        self.send_frame(&state.colors)
    }

    fn release(&mut self) -> Result<(), DeviceError> {
        // Always sent, even if this session never took control: it is idempotent and also cleans up after a
        // previous run that crashed while in control.
        self.controlling = false;
        self.transport
            .send_feature_report(&protocol::reset_all_packet())?;
        tracing::debug!(target: "cc_drivers::wooting", id = %self.desc.info.id, "reset to profile lighting");
        Ok(())
    }
}
