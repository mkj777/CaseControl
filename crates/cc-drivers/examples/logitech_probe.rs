//! Probe of the Logitech devices (G560, PRO Wireless via Lightspeed receiver). Takes no lighting control.
//!
//! Runs the driver's detection and prints each description, the 0x8070 zones and effects, and the raw 0x8070
//! GetEffect readback of each zone (what the firmware showed before anything was claimed). Detection itself sends
//! exactly what the reference sends: HID++ 2.0 feature/name/zone queries to the G560 and the paired device; on the
//! receiver a read of registers 0x00 and 0x02, enabling wireless notifications only if they are off, and the "fake
//! reconnect" write to register 0x02 that makes the receiver list its paired devices. No SetSWControl, no effect
//! writes, nothing to flash.
//!
//! Run by hand only (not in CI): `cargo run -p cc-drivers --example logitech_probe`.

use cc_core::{Controller, DetectContext};
use cc_drivers::logitech::hidpp::{ZoneEffectState, ZoneInfo};
use cc_drivers::logitech::{Found, LogitechBackend};

fn print_zones(zones: &[ZoneInfo], initial: &[ZoneEffectState]) {
    for z in zones {
        println!("  0x8070 zone {} location 0x{:04X}", z.index, z.location);
        for e in &z.effects {
            println!(
                "    effect {:2}: id 0x{:04X} caps 0x{:04X} default period {} ms",
                e.index, e.id, e.caps, e.period
            );
        }
        match initial.iter().find(|s| s.zone == z.index).and_then(|s| s.raw) {
            Some(raw) => println!("    live effect (GetEffect): {}", cc_hid::hex(&raw)),
            None => println!("    live effect (GetEffect): rejected by the device"),
        }
    }
}

fn print_description(c: &dyn Controller) {
    let d = c.description();
    println!("\n{} ({})", d.info.name, d.info.id);
    println!("  kind: {:?}", d.info.kind);
    println!("  location: {}", d.info.location);
    println!("  serial: {:?}", d.info.serial);
    for z in &d.zones {
        println!("  zone {:?} leds {}..{}", z.name, z.start, z.start + z.len);
    }
    for m in &d.modes {
        println!(
            "  mode {:#x} {:?} caps {:?} color {:?} speed {:?} brightness {:?}",
            m.id, m.name, m.caps, m.color_mode, m.speed, m.brightness
        );
    }
    println!("  direct mode: {:?}", d.direct_mode.map(|m| format!("{m:#x}")));
}

fn main() {
    let found = LogitechBackend::new().detect_typed(&DetectContext::default());
    println!("detect: {} result(s)", found.len());
    for f in &found {
        match f {
            Found::G560(c) => {
                print_description(c);
                print_zones(c.zones(), c.initial_state());
            }
            Found::Lightspeed(c) => {
                print_description(c);
                println!("  receiver device index: {}", c.device_index());
                print_zones(c.zones(), c.initial_state());
            }
            Found::Failed { location, error } => println!("\nfailed at {location}: {error}"),
        }
    }
    // Nothing was claimed; dropping the controllers needs no release.
    drop(found);
}
