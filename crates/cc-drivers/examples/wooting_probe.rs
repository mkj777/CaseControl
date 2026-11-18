//! Detects Wooting V3 keyboards and prints what the driver sees. Read-only: lists the matching HID interfaces,
//! opens the V3 interface (as detection does) and sends nothing. The V3 protocol has no lighting state to read
//! back, so nothing else is queried.
//!
//! Run by hand: `cargo run -p cc-drivers --example wooting_probe`.

#![allow(clippy::print_stdout)]

use cc_core::{DetectContext, Detected, ZoneKind};
use cc_drivers::wooting;

fn main() {
    match cc_hid::enumerate() {
        Ok(devices) => {
            println!("Wooting HID interfaces (vid 31e3):");
            for d in devices.iter().filter(|d| d.vid == wooting::protocol::VID) {
                println!(
                    "  {:04x}:{:04x} iface {:>2} {:04x}/{:04x} {:?} serial {:?} release {:04x} {}",
                    d.vid,
                    d.pid,
                    d.interface,
                    d.usage_page,
                    d.usage,
                    d.product.as_deref().unwrap_or("-"),
                    d.serial.as_deref().unwrap_or("-"),
                    d.release,
                    d.path
                );
            }
            println!("Candidates:");
            for (id, d) in wooting::candidates(&devices) {
                println!("  {id} -> {}", d.path);
            }
        }
        Err(e) => println!("enumeration failed: {e}"),
    }

    let detected = wooting::backend().detect(&DetectContext::default());
    if detected.is_empty() {
        println!("no Wooting V3 keyboard detected");
    }
    for d in detected {
        match d {
            Detected::Device(c) => {
                let desc = c.description();
                println!("{} ({})", desc.info.name, desc.info.id);
                println!("  vendor {}  serial {:?}", desc.info.vendor, desc.info.serial);
                println!("  location {}", desc.info.location);
                println!("  valid: {:?}", desc.validate());
                for z in &desc.zones {
                    println!("  zone {:?}: {} LEDs from {}", z.name, z.len, z.start);
                    if let ZoneKind::Matrix { width, height, map } = &z.kind {
                        for row in map.chunks(*width as usize).take(*height as usize) {
                            let cells: Vec<String> = row
                                .iter()
                                .map(|c| c.map_or_else(|| " --".to_owned(), |i| format!("{i:3}")))
                                .collect();
                            println!("    {}", cells.join(" "));
                        }
                    }
                }
                for (i, led) in desc.leds.iter().enumerate() {
                    println!("  led {i:2}: {}", led.name);
                }
                for m in &desc.modes {
                    println!("  mode {:#06x} {:?} {:?}", m.id, m.name, m.color_mode);
                }
                // Dropped without release(): nothing was sent, so there is nothing to hand back.
            }
            Detected::Failed { location, error, .. } => println!("failed at {location}: {error}"),
        }
    }
}
