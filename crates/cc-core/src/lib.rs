//! CaseControl device model: ids, zones, modes, the [`Controller`] and [`Backend`] traits, and engine events.
//!
//! This crate mirrors OpenRGB's `RGBController` semantics in idiomatic Rust: one flat color buffer per device,
//! zones that index into it, and modes described by capability flags. A "direct" mode means per-LED static colors
//! with no hardware effect running. Every data type is serde-serializable because the same values cross the Tauri
//! IPC boundary as JSON and are stored in TOML profiles.
//!
//! Saving a look to a device's own memory (ADR 0001 #42) is part of the [`Controller`] trait, with its types in
//! [`persist`]; the colour helpers for it ([`average_linear`], sRGB <-> linear) live in [`color`]. The "last good own
//! lighting" seam the drivers share (ADR 0001 #38) is [`OwnLighting`].
//!
//! What this crate deliberately does not do: no I/O, no device access, no threads, no platform-specific code and no
//! `cfg(windows)`. Drivers (`cc-drivers`) implement the traits, transports live in `cc-hid` / `cc-nvapi`, and the
//! engine (`cc-engine`) owns threading and scheduling. Byte-order packing of colors is the drivers' business.

pub mod backend;
pub mod color;
pub mod controller;
pub mod device;
pub mod error;
pub mod event;
pub mod family;
pub mod id;
pub mod mode;
pub mod own;
pub mod persist;

pub use backend::{
    Backend, DetectContext, Detected, HotplugAction, HotplugEvent, HotplugHandle, HotplugSender,
};
pub use color::{ParseRgbError, Rgb, average_linear, linear_to_srgb, srgb_to_linear};
pub use controller::Controller;
pub use device::{Bus, DeviceDescription, DeviceInfo, DeviceKind, Led, Zone, ZoneKind};
pub use error::{DeviceError, ModelError};
pub use event::EngineEvent;
pub use id::DeviceId;
pub use mode::{ColorMode, Direction, Mode, ModeCaps, ModeState};
pub use own::{OwnLighting, OwnSource};
pub use persist::{PersistCaps, PersistLook, PersistOutcome};
