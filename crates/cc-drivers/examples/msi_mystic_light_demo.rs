//! Direct-mode demo for MSI Mystic Light boards. Writes to the hardware; run by hand only, never in CI:
//!
//! ```text
//! cargo run -p cc-drivers --example msi_mystic_light_demo -- [JRAINBOW1 LEDs] [JRAINBOW2 LEDs]
//! ```
//!
//! The ARGB header sizes default to 0 (only JRGB headers light up); pass the LED counts of the connected strips or
//! fans to include them. For each detected board: set all LEDs red, wait 3 s, green, 3 s, blue, 3 s, then `release()`
//! and print the restored configuration. `release()` also runs when any step fails. Nothing is written to flash.

use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use cc_core::{Controller, DeviceError, Rgb};
use cc_drivers::msi_mystic_light::{self as msi, MysticLight185};

const HOLD: Duration = Duration::from_secs(3);

fn main() -> ExitCode {
    let sizes: Vec<usize> = match std::env::args().skip(1).map(|a| a.parse()).collect() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("usage: msi_mystic_light_demo [JRAINBOW1 LEDs] [JRAINBOW2 LEDs] ({e})");
            return ExitCode::FAILURE;
        }
    };
    let devices = match cc_hid::enumerate() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("HID enumeration failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let candidates = msi::candidates(&devices);
    if candidates.is_empty() {
        println!("no Mystic Light board found");
        return ExitCode::SUCCESS;
    }

    let mut ok = true;
    for cand in &candidates {
        println!("== {}", cand.id);
        let opened = cc_hid::open(&cand.path)
            .map_err(DeviceError::from)
            .and_then(|dev| MysticLight185::open(dev, cand));
        let mut c = match opened {
            Ok(c) => c,
            Err(e) => {
                println!("   open failed (nothing was changed): {e}");
                ok = false;
                continue;
            }
        };
        println!(
            "   {} firmware {}",
            c.description().info.name,
            c.firmware().display()
        );

        let result = run(&mut c, &sizes);
        if let Err(e) = &result {
            println!("   demo failed: {e}");
            ok = false;
        }
        match c.release() {
            Ok(()) => {
                println!("   released; restored this configuration (save byte forced to 0):");
                for chunk in c.original_config().with_save(false).chunks(16) {
                    println!("     {}", cc_hid::hex(chunk));
                }
                for (zone, msi_zone) in c.description().zones.iter().zip(c.msi_zones()) {
                    if let Some(s) = c.original_config().zone_state(*msi_zone, c.mixed_mode()) {
                        println!(
                            "   {:<10} effect {:>3} brightness {:>2} color {}",
                            zone.name, s.effect, s.brightness, s.color
                        );
                    }
                }
            }
            Err(e) => {
                println!("   RELEASE FAILED: {e}");
                ok = false;
            }
        }
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn run(c: &mut MysticLight185, sizes: &[usize]) -> Result<(), DeviceError> {
    for (i, name) in ["JRAINBOW1", "JRAINBOW2"].iter().enumerate() {
        let Some(len) = sizes.get(i).copied() else {
            continue;
        };
        if let Some(zone) = c.description().zones.iter().position(|z| z.name == *name) {
            c.resize_zone(zone, len)?;
            println!("   {name} resized to {len} LEDs");
        }
    }
    println!("   sync per-LED protocol: {}", c.sync_direct());
    let n = c.description().led_count();
    for (label, color) in [
        ("red", Rgb::new(0xFF, 0, 0)),
        ("green", Rgb::new(0, 0xFF, 0)),
        ("blue", Rgb::new(0, 0, 0xFF)),
    ] {
        println!("   {label} on {n} LEDs");
        c.set_direct(&vec![color; n])?;
        thread::sleep(HOLD);
    }
    Ok(())
}
