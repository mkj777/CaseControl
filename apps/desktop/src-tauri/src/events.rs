//! Tauri event names and payloads, and the thread that forwards engine events to the frontend.

use cc_core::EngineEvent;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Runtime};

/// A device was detected; payload: `DeviceInfo`.
pub const DEVICE_ADDED: &str = "device-added";
/// A device disappeared or its family was disabled; payload: `{ id }`.
pub const DEVICE_REMOVED: &str = "device-removed";
/// A device was taken out of service; payload: `{ id, error }`.
pub const DEVICE_FAULTED: &str = "device-faulted";
/// A device was taken under control, handed back to its own lighting, or switched to a hardware mode; payload:
/// `{ id, active, state: ModeState | null }`.
pub const DEVICE_STATE_CHANGED: &str = "device-state-changed";
/// Device colours or status changed through a path the frontend did not start (tray actions); no payload.
pub const DEVICES_CHANGED: &str = "devices-changed";
/// Profiles were created, duplicated, renamed, deleted or switched; payload: `ProfileList`.
pub const PROFILES_CHANGED: &str = "profiles-changed";
/// A conflict scan finished or conflict settings / autostart changed; payload: `ConflictScan`.
pub const CONFLICTS_UPDATED: &str = "conflicts-updated";
/// The start-up check found programs to ask about (sent once per start, after the window was shown); payload:
/// `StartupConflicts` (non-empty). The frontend can also call `startup_conflicts`.
pub const STARTUP_CONFLICTS: &str = "startup-conflicts";

/// The Tauri event name and payload for an engine event.
pub fn engine_event(ev: &EngineEvent) -> (&'static str, Value) {
    match ev {
        EngineEvent::DeviceAdded(info) => (DEVICE_ADDED, json!(info)),
        EngineEvent::DeviceRemoved(id) => (DEVICE_REMOVED, json!({ "id": id })),
        EngineEvent::DeviceFaulted { id, error } => (DEVICE_FAULTED, json!({ "id": id, "error": error })),
        EngineEvent::DeviceStateChanged { id, active, state } => (
            DEVICE_STATE_CHANGED,
            json!({ "id": id, "active": active, "state": state }),
        ),
    }
}

/// Emits `payload` as `event` to the frontend, logging (not failing) when that is impossible.
pub fn emit<R: Runtime, S: serde::Serialize + Clone>(app: &AppHandle<R>, event: &str, payload: S) {
    if let Err(err) = app.emit(event, payload) {
        tracing::warn!(event, %err, "cannot emit event");
    }
}

/// Forwards every event from `rx` until the engine closes the stream (on shutdown).
pub fn spawn_forwarder<R: Runtime>(app: AppHandle<R>, rx: crossbeam_channel::Receiver<EngineEvent>) {
    let spawned = std::thread::Builder::new()
        .name("cc-events".into())
        .spawn(move || {
            for ev in rx {
                let (name, payload) = engine_event(&ev);
                tracing::debug!(event = name, "engine event");
                emit(&app, name, payload);
            }
            tracing::debug!("engine event stream closed");
        });
    if let Err(err) = spawned {
        tracing::error!(%err, "cannot start the event forwarder; the UI will not see hot-plug events");
    }
}

#[cfg(test)]
mod tests {
    use cc_core::{Bus, DeviceId, DeviceInfo, DeviceKind, ModeState};

    use super::*;

    #[test]
    fn engine_events_map_to_tauri_events() {
        let id = DeviceId::hid(0x046D, 0x0A78, None);
        let info = DeviceInfo {
            id: id.clone(),
            name: "G560".into(),
            vendor: "Logitech".into(),
            kind: DeviceKind::Speaker,
            bus: Bus::Hid,
            location: "p".into(),
            serial: None,
            firmware: None,
        };
        let (name, payload) = engine_event(&EngineEvent::DeviceAdded(info));
        assert_eq!(name, DEVICE_ADDED);
        assert_eq!(payload["id"], json!("hid:046d:0a78"));
        assert_eq!(payload["kind"], json!("Speaker"));

        let (name, payload) = engine_event(&EngineEvent::DeviceRemoved(id.clone()));
        assert_eq!(
            (name, payload),
            (DEVICE_REMOVED, json!({ "id": "hid:046d:0a78" }))
        );

        let (name, payload) = engine_event(&EngineEvent::DeviceFaulted {
            id: id.clone(),
            error: "gone".into(),
        });
        assert_eq!(
            (name, payload),
            (DEVICE_FAULTED, json!({ "id": "hid:046d:0a78", "error": "gone" }))
        );

        let state = ModeState {
            mode: 2,
            speed: None,
            brightness: Some(50),
            direction: None,
            colors: vec![],
        };
        let (name, payload) = engine_event(&EngineEvent::DeviceStateChanged {
            id: id.clone(),
            active: true,
            state: Some(state),
        });
        assert_eq!(name, DEVICE_STATE_CHANGED);
        assert_eq!(payload["active"], json!(true));
        assert_eq!(payload["state"]["brightness"], json!(50));
        assert_eq!(payload["state"]["direction"], Value::Null);

        let (name, payload) = engine_event(&EngineEvent::DeviceStateChanged {
            id,
            active: false,
            state: None,
        });
        assert_eq!(
            (name, payload),
            (
                DEVICE_STATE_CHANGED,
                json!({ "id": "hid:046d:0a78", "active": false, "state": null })
            )
        );
    }
}
