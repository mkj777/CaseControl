//! CaseControl runtime: detection, per-device worker threads, frame coalescing, profile apply, hot-plug and the
//! event bus.
//!
//! [`start`] runs detection on the backends it is given (the app passes `cc_drivers::all_backends()`), then gives every
//! detected [`Controller`](cc_core::Controller) its own thread (`cc-dev-<id>`). The [`EngineHandle`] is the only way
//! in:
//!
//! - Colour edits ([`EngineHandle::set_direct`], [`EngineHandle::set_zone`]) update the engine's per-device colour
//!   buffer and are coalesced: each device thread sends at most one frame per frame interval, always the newest.
//! - Mode changes, zone resizes, saving and releasing are acknowledged: the call waits for the device thread (3 s).
//! - Error policy: write errors back off exponentially (50 ms doubling to 5 s); ten consecutive failures or one fatal
//!   error mark the device `Faulted`, after which its thread no longer talks to it.
//! - Hot-plug: one HID watcher; arrivals trigger detection of HID backends with every open device skipped, removals
//!   drop the matching device without releasing it.
//! - Brightness (ADR 0001 #33): a device brightness per device and a master brightness scale what goes to the device
//!   (colours, or a hardware mode's brightness); All off fades every driven device to black and holds it there. The
//!   stored settings stay unscaled, so `current_settings()` and profiles never see dimmed colours.
//! - Resume (sleep, hibernation): on Windows the engine subscribes to suspend/resume broadcasts and then calls
//!   [`EngineHandle::reassert`]: debounced, it resets and re-sends every driven device's current output, retries
//!   failed writes with back-off for a few seconds and re-opens devices that faulted or vanished during sleep.
//! - [`EngineHandle::shutdown`] (or dropping the last handle) releases every active device and joins the threads.
//!
//! What this crate deliberately does not do: no device protocols (that is `cc-drivers`), no file I/O or profile
//! storage (that is `cc-config`; the engine only takes and returns settings), no Tauri or UI code, no effects engine
//! (colour frames come from the caller), and it never writes device flash unless [`EngineHandle::save_to_hardware`] is
//! called.

mod engine;
mod error;
mod hotplug;
mod lighting;
mod resume;
mod view;
mod worker;

#[cfg(test)]
mod tests;

pub use cc_config::DeviceSetting;
pub use engine::{EngineConfig, EngineHandle, start};
pub use error::EngineError;
pub use view::{ApplyFailure, ApplyReport, DetectFailure, DeviceStatus, DeviceView, LightingView};
