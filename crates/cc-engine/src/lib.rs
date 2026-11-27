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
//! - Brightness (ADR 0001 #33, #35): a device brightness per device and a master brightness scale what goes to the
//!   device (colours, or a hardware mode's brightness); brightness 0 is off (black, or the device's "Off" mode). The
//!   stored settings stay unscaled, so `current_settings()` and profiles never see dimmed colours.
//! - Control status (phase 3 brief 05): a device is `Active` only while the engine outputs something to it and
//!   `Released` while it shows its own lighting, which includes "never taken since it was detected";
//!   `DeviceStateChanged` is emitted on every change. Every lighting control takes over an undriven device that is
//!   not excluded: colours and modes directly; brightness and Connect all with the device's last setting, else the
//!   mode its driver read back from the hardware (`DeviceView::hardware`), else the default colour (ADR 0001 #36,
//!   [`EngineHandle::set_default_color`]) on every LED in direct mode, or in the first mode that takes a colour for a
//!   device without a direct mode. Only a device with none of these keeps its own lighting. `DeviceView::takeover`
//!   says what a takeover would use, so the UI does not repeat that choice.
//! - Profiles (ADR 0001 #40): [`EngineHandle::apply_replacing`] applies a whole profile; connected devices it has
//!   no entry for forget their setting and, if driven, fall back to their own mode, else the default colour.
//!   [`EngineHandle::current_settings`] returns what the engine drives each device with, for saving.
//! - Dark zones (the app's hidden zones, ADR 0001 #26): every direct output has them black, whatever path drives the
//!   device ([`EngineHandle::set_dark_zones`], [`mask_hidden_zones`]).
//! - Last good own lighting (ADR 0001 #38): every detection hands the drivers what is remembered per device
//!   (`DetectContext::own_lighting`, from [`EngineConfig::hardware_state`]); what a newly opened controller reports
//!   through `Controller::own_lighting` is recorded there, which writes `hardware-state.toml` (through `cc-config`)
//!   only when the value changed. A persisted "unreleased" marker per device (`DetectContext::unreleased`) says
//!   CaseControl wrote to it and did not hand it back: a worker sets it before its first write after a takeover and
//!   clears it after a successful release, and a new controller that found (or restored) the device's own lighting
//!   (`Controller::shows_own_lighting`) clears it at open. Brightness 0 uses the driver's off mode
//!   (`Controller::off_mode`, Mystic Light: Static at brightness 0) instead of black in the direct mode where the
//!   driver names one. [`EngineHandle::hardware_state`] returns a read-only snapshot.
//! - Release hardening (ADR 0001 #38): stopping releases faulted devices too (best effort), and detection releases
//!   the faulted devices of the backends it runs (all stops first, then one deadline) before it opens them again; a
//!   device whose old worker has not ended by then is skipped in that pass, so two controllers never drive it.
//! - Resume (sleep, hibernation): on Windows the engine subscribes to suspend/resume broadcasts and then calls
//!   [`EngineHandle::reassert`]: debounced, it resets and re-sends every driven device's current output, retries
//!   failed writes with back-off for a few seconds and re-opens devices that faulted or vanished during sleep.
//! - [`EngineHandle::shutdown`] (or dropping the last handle) releases every active device and joins the threads.
//!   [`EngineHandle::shutdown_with`] with [`ShutdownMode::Keep`] (ADR 0001 #42: Windows shutdown, restart, sign-out)
//!   sends each device its pending or paced output once and stops without a release; the "unreleased" markers stay.
//! - Early apply (ADR 0001 #42): [`EngineConfig::initial_settings`] are pending before the first detection, and each
//!   backend's devices are added (and get their setting) as soon as that backend is done, so the first devices are
//!   driven while later backends are still probed. Drivers learn which devices are about to be driven
//!   (`DetectContext::setting_pending`) and skip restoring an old look at open for them.
//! - Save to devices (ADR 0001 #34, #42): [`EngineHandle::persist_all`] writes the current look of every visible,
//!   capable, driven device to its own memory (one colour per zone averaged in linear light, a mode exactly,
//!   brightness baked in, off at brightness 0) within a time budget; the driver's backup of the original saved state
//!   is stored first and a device without one is never written; unchanged looks are skipped and at most 20 writes a
//!   day go out per device. [`EngineHandle::restore_saved`] writes the backups back (absent devices when they next
//!   connect; at most 3 attempts a day each). A manual [`EngineHandle::save_to_hardware`] goes through the same
//!   machinery. Every such request carries a deadline (a worker that takes it up late skips it), and a request whose
//!   answer does not come in time is reported as [`PersistResult::Unconfirmed`]. [`EngineConfig::device_memory_writes`]
//!   switches all of it off. The records live in `device-memory.toml` (`cc_config::HardwareState`), written durably
//!   before a device is touched.
//!
//! What this crate deliberately does not do: no device protocols (that is `cc-drivers`), no file I/O or profile
//! storage of its own (that is `cc-config`; the engine only takes and returns settings, and records the drivers'
//! own lighting, the "unreleased" markers and the device-memory records through `cc_config::HardwareState`, the only
//! files it causes to be written), no Tauri or UI code, no effects engine (colour frames come from the caller), and it
//! never writes device flash unless [`EngineHandle::save_to_hardware`],
//! [`EngineHandle::persist_all`] or [`EngineHandle::restore_saved`] is called (or a restore is pending for a device
//! that connects), never on live edits.

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
pub use lighting::mask_hidden_zones;
pub use view::{
    ApplyFailure, ApplyReport, DetectFailure, DeviceStatus, DeviceView, LightingView, PersistEntry,
    PersistReport, PersistResult, ReleaseReport, ShutdownMode, SkipReason, Takeover,
};
