//! Hardware demo for the Logitech devices: all LEDs red, 3 s, green, 3 s, blue, 3 s, then `release()`.
//!
//! The direct colours are volatile 0x8070 Static writes under `SetSWControl(1, 1)`. `release()` sends
//! `SetSWControl(0, 0)` (`11 <index> <feature> 87 00 00 ...`), which hands the zones back to the firmware effect;
//! the firmware effect each zone showed before the demo is printed so it can be compared by eye. Every device is
//! released on every path, errors included.
//!
//! Run by hand only (not in CI): `cargo run -p cc-drivers --example logitech_demo`.

use std::thread;
use std::time::Duration;

use cc_core::{Controller, DetectContext, DeviceError, Rgb};
use cc_drivers::logitech::hidpp::ZoneEffectState;
use cc_drivers::logitech::{Found, LogitechBackend};

fn show(c: &mut dyn Controller) -> Result<(), DeviceError> {
    let n = c.description().led_count();
    for (name, color) in [
        ("red", Rgb::new(0xFF, 0, 0)),
        ("green", Rgb::new(0, 0xFF, 0)),
        ("blue", Rgb::new(0, 0, 0xFF)),
    ] {
        println!("  {name}");
        c.set_direct(&vec![color; n])?;
        thread::sleep(Duration::from_secs(3));
    }
    Ok(())
}

fn run(c: &mut dyn Controller, initial: &[ZoneEffectState]) -> bool {
    let name = c.description().info.name.clone();
    println!("\n{name} ({})", c.description().info.id);
    for s in initial {
        match s.raw {
            Some(raw) => println!("  zone {} before: {}", s.zone, cc_hid::hex(&raw)),
            None => println!("  zone {} before: unreadable", s.zone),
        }
    }
    let shown = show(c);
    if let Err(e) = &shown {
        println!("  error while showing colours: {e}");
    }
    // Release on every path.
    match c.release() {
        Ok(()) => println!("  released: SetSWControl(0, 0) sent, firmware effect restored"),
        Err(e) => println!("  release FAILED: {e}"),
    }
    shown.is_ok()
}

fn main() -> std::process::ExitCode {
    let mut found = LogitechBackend::new().detect_typed(&DetectContext::default());
    println!("detect: {} result(s)", found.len());
    let mut ok = true;
    for f in &mut found {
        ok &= match f {
            Found::G560(c) => {
                let initial = c.initial_state().to_vec();
                run(c, &initial)
            }
            Found::Lightspeed(c) => {
                let initial = c.initial_state().to_vec();
                run(c, &initial)
            }
            Found::Failed { location, error } => {
                println!("\nfailed at {location}: {error}");
                false
            }
        };
    }
    if ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
