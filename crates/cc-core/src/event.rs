//! Events the engine publishes to the UI and other observers.

use serde::{Deserialize, Serialize};

use crate::device::DeviceInfo;
use crate::id::DeviceId;
use crate::mode::ModeState;

/// Something that happened to a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineEvent {
    /// A device was detected and is under control.
    DeviceAdded(DeviceInfo),
    /// A device was disconnected or released.
    DeviceRemoved(DeviceId),
    /// A device hit a fatal error and was taken out of service.
    DeviceFaulted {
        /// The device.
        id: DeviceId,
        /// What went wrong.
        error: String,
    },
    /// A device was taken under software control or handed back to its own lighting, or switched to a hardware
    /// mode.
    DeviceStateChanged {
        /// The device.
        id: DeviceId,
        /// Whether CaseControl drives the device now (`false`: it shows its own lighting).
        active: bool,
        /// The hardware mode the device was switched to, when that was the change.
        state: Option<ModeState>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;
    use crate::device::sample_description;
    use crate::mode::Direction;

    #[test]
    fn json_round_trip() {
        let info = sample_description().info;
        let id = info.id.clone();
        let state = ModeState {
            mode: 1,
            speed: Some(2),
            brightness: Some(80),
            direction: Some(Direction::Left),
            colors: vec![Rgb::new(0xFF, 0, 0x10)],
        };
        let events = [
            EngineEvent::DeviceAdded(info),
            EngineEvent::DeviceRemoved(id.clone()),
            EngineEvent::DeviceFaulted {
                id: id.clone(),
                error: "device disconnected".into(),
            },
            EngineEvent::DeviceStateChanged {
                id: id.clone(),
                active: true,
                state: Some(state.clone()),
            },
            EngineEvent::DeviceStateChanged {
                id,
                active: false,
                state: None,
            },
        ];
        for ev in events {
            let json = serde_json::to_string(&ev).expect("serialize");
            let back: EngineEvent = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, ev);
        }

        let json = serde_json::to_value(&state).expect("serialize state");
        assert_eq!(
            json,
            serde_json::json!({
                "mode": 1, "speed": 2, "brightness": 80, "direction": "Left", "colors": ["#FF0010"]
            })
        );
        let back: ModeState = serde_json::from_value(json).expect("deserialize state");
        assert_eq!(back, state);
    }
}
