//! The [`Controller`] trait every device driver implements.

use crate::color::Rgb;
use crate::device::DeviceDescription;
use crate::error::DeviceError;
use crate::mode::ModeState;
use crate::persist::{PersistCaps, PersistLook, PersistOutcome};

/// One physical controller.
///
/// Threading contract: the engine runs one thread per controller and calls a controller only from that thread, so
/// `Send` suffices and `Sync` is not required. Implementations need no internal locking.
///
/// `release` is mandatory: every driver hands the device back to its firmware or hardware mode so a crash-free quit
/// leaves the device as the firmware would. Coexistence with vendor tools depends on this.
pub trait Controller: Send {
    /// The device's identity, layout and modes. Stable for the controller's lifetime except after
    /// [`resize_zone`](Controller::resize_zone).
    fn description(&self) -> &DeviceDescription;

    /// Shows `colors` in direct mode. `colors.len()` equals the device's LED count.
    fn set_direct(&mut self, colors: &[Rgb]) -> Result<(), DeviceError>;

    /// Shows `colors` on zone `zone` in direct mode. `colors.len()` equals the zone length.
    fn set_zone_direct(&mut self, zone: usize, colors: &[Rgb]) -> Result<(), DeviceError>;

    /// Switches to the mode named by `state` with its parameters.
    fn set_mode(&mut self, state: &ModeState) -> Result<(), DeviceError>;

    /// Changes the LED count of a resizable zone. Unsupported unless the driver overrides it.
    fn resize_zone(&mut self, _zone: usize, _len: usize) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported)
    }

    /// Persists the current hardware mode in the device's flash, through the same gates as
    /// [`persist_look`](Self::persist_look); refused while the device is not driven by a hardware mode. The engine
    /// routes a manual save through `persist_look` itself (backup first, daily cap, fingerprint) and does not call
    /// this. Unsupported unless the driver overrides it.
    fn save_to_hardware(&mut self) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported)
    }

    /// Refreshes software control on devices that fall back to hardware mode when idle (see
    /// [`DeviceDescription::keepalive`]). A no-op unless the driver overrides it.
    fn keepalive(&mut self) -> Result<(), DeviceError> {
        Ok(())
    }

    /// Hands the device back to its firmware/hardware mode. Called on shutdown and on explicit "release".
    fn release(&mut self) -> Result<(), DeviceError>;

    /// The lighting the device shows on its own (its hardware mode), as read from the device before software control
    /// was taken, expressed as a state of one of the description's modes; `None` when the driver cannot read it or
    /// cannot express it as a mode (the default). Must not do I/O: drivers return what they read when the device was
    /// opened (or re-read when control was taken again). The engine shows it for a released device and adopts it as
    /// the device's setting when a brightness change takes over a device that has none, so nothing jumps.
    fn hardware_mode(&self) -> Option<ModeState> {
        None
    }

    /// Driver-opaque bytes describing the lighting the device showed on its own when it was opened, worth
    /// remembering as its "last good own lighting" (ADR 0001 #38); `None` (the default) when the driver has nothing
    /// to remember, e.g. because the device was found in a state left by software control (see
    /// [`DetectContext::unreleased`](crate::DetectContext::unreleased)). Must not do I/O. The engine stores the bytes
    /// per device id and hands them back in [`DetectContext::own_lighting`](crate::DetectContext::own_lighting) on
    /// later opens.
    fn own_lighting(&self) -> Option<Vec<u8>> {
        None
    }

    /// Whether the device shows (after open) or will show (after a successful `release()`) lighting of its own,
    /// rather than a state CaseControl left on it (ADR 0001 #38). The engine asks right after open and after every
    /// successful release, and clears the device's "unreleased" marker when this is `true`. A driver returns `false`
    /// when it was opened on an unreleased device and could not restore its own lighting, so `release()` hands back
    /// CaseControl's state. Must not do I/O. Default `true`.
    fn shows_own_lighting(&self) -> bool {
        true
    }

    /// A hardware mode that shows the device dark at the lowest value of its brightness range, which the engine uses
    /// for brightness 0 (ADR 0001 #35, #38) instead of black in the direct mode, e.g. because entering the direct mode
    /// changes the device's state in a way that outlives the app. `None` (the default) keeps black in the direct mode.
    fn off_mode(&self) -> Option<u32> {
        None
    }

    /// What this device can save to its own memory (ADR 0001 #42); `None` (the default) when it cannot. Must not do
    /// I/O.
    fn persist_caps(&self) -> Option<PersistCaps> {
        None
    }

    /// Writes `look` to the device's memory so it shows it from power-on (ADR 0001 #34, #42). Returns
    /// [`PersistOutcome::Unchanged`] without any I/O when the look's fingerprint equals `last` (the fingerprint of the
    /// previous write). The driver enforces every safety gate of its device and refuses
    /// ([`DeviceError::Refused`], nothing written to the memory) when one fails; the device keeps showing what the
    /// engine drives it with. Never called on live edits. Unsupported unless the driver overrides it.
    fn persist_look(
        &mut self,
        _look: &PersistLook,
        _last: Option<&[u8]>,
    ) -> Result<PersistOutcome, DeviceError> {
        Err(DeviceError::Unsupported)
    }

    /// The device's original saved state (what it showed from power-on before CaseControl first saved to it), as
    /// driver-opaque bytes for [`restore_saved_state`](Self::restore_saved_state); `None` (the default) when the
    /// driver cannot tell it completely, in which case the engine never saves to the device. Captured before
    /// CaseControl's first write of the session. Must not do I/O.
    fn saved_state_backup(&self) -> Option<Vec<u8>> {
        None
    }

    /// Writes `backup` (from [`saved_state_backup`](Self::saved_state_backup), possibly of an earlier run) back to
    /// the device's memory. Validates the bytes before sending anything. Returns [`PersistOutcome::Unchanged`] when
    /// the driver can tell that the memory already holds the backup (nothing written). A refusal before anything
    /// reached the memory is [`DeviceError::Refused`] or [`DeviceError::InvalidArgument`]. Unsupported unless the
    /// driver overrides it.
    fn restore_saved_state(&mut self, _backup: &[u8]) -> Result<PersistOutcome, DeviceError> {
        Err(DeviceError::Unsupported)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::device::sample_description;

    /// A controller that records calls and touches no hardware.
    #[derive(Debug)]
    pub(crate) struct FakeController {
        pub desc: DeviceDescription,
        pub calls: Vec<String>,
    }

    impl FakeController {
        pub(crate) fn new() -> Self {
            Self {
                desc: sample_description(),
                calls: Vec::new(),
            }
        }
    }

    impl Controller for FakeController {
        fn description(&self) -> &DeviceDescription {
            &self.desc
        }

        fn set_direct(&mut self, colors: &[Rgb]) -> Result<(), DeviceError> {
            if colors.len() != self.desc.led_count() {
                return Err(DeviceError::InvalidArgument("wrong length".into()));
            }
            self.calls.push(format!("direct {}", colors.len()));
            Ok(())
        }

        fn set_zone_direct(&mut self, zone: usize, colors: &[Rgb]) -> Result<(), DeviceError> {
            self.calls.push(format!("zone {zone} {}", colors.len()));
            Ok(())
        }

        fn set_mode(&mut self, state: &ModeState) -> Result<(), DeviceError> {
            self.desc
                .validate_state(state)
                .map_err(|e| DeviceError::InvalidArgument(e.to_string()))?;
            self.calls.push(format!("mode {}", state.mode));
            Ok(())
        }

        fn release(&mut self) -> Result<(), DeviceError> {
            self.calls.push("release".into());
            Ok(())
        }
    }

    #[test]
    fn usable_as_trait_object() {
        let mut c: Box<dyn Controller> = Box::new(FakeController::new());
        assert_eq!(c.description().led_count(), 7);
        assert_eq!(c.set_direct(&[Rgb::WHITE; 7]), Ok(()));
        assert!(c.set_direct(&[Rgb::WHITE; 3]).is_err());
        assert_eq!(c.set_zone_direct(1, &[Rgb::BLACK; 3]), Ok(()));
        let state = ModeState {
            mode: 3,
            speed: None,
            brightness: None,
            direction: None,
            colors: vec![],
        };
        assert_eq!(c.set_mode(&state), Ok(()));
        assert_eq!(c.resize_zone(1, 10), Err(DeviceError::Unsupported));
        assert_eq!(c.save_to_hardware(), Err(DeviceError::Unsupported));
        assert_eq!(c.keepalive(), Ok(()));
        assert_eq!(c.hardware_mode(), None);
        assert_eq!(c.own_lighting(), None);
        assert!(c.shows_own_lighting());
        assert_eq!(c.off_mode(), None);
        assert_eq!(c.persist_caps(), None);
        assert_eq!(
            c.persist_look(&PersistLook::Off, None),
            Err(DeviceError::Unsupported)
        );
        assert_eq!(c.saved_state_backup(), None);
        assert_eq!(c.restore_saved_state(&[1, 2]), Err(DeviceError::Unsupported));
        assert_eq!(c.release(), Ok(()));
    }

    #[test]
    fn controller_is_send() {
        fn assert_send<T: Send + ?Sized>() {}
        assert_send::<Box<dyn Controller>>();
    }
}
