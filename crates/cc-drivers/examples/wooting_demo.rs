//! Takes control of every detected Wooting V3 keyboard, shows red, green and blue for 3 s each, then releases
//! (Wooting "reset all": the keyboard shows its active profile's lighting again). Release also runs when a step
//! fails. Lighting commands only; no key mapping, actuation or profile is touched.
//!
//! Run by hand: `cargo run -p cc-drivers --example wooting_demo`.

#![allow(clippy::print_stdout)]

use std::thread::sleep;
use std::time::Duration;

use cc_core::{Controller, DetectContext, Detected, DeviceError, Rgb};
use cc_drivers::wooting;

fn show(c: &mut dyn Controller) -> Result<(), DeviceError> {
    let n = c.description().led_count();
    for (name, color) in [
        ("red", Rgb::new(255, 0, 0)),
        ("green", Rgb::new(0, 255, 0)),
        ("blue", Rgb::new(0, 0, 255)),
    ] {
        println!("  {name}");
        c.set_direct(&vec![color; n])?;
        sleep(Duration::from_secs(3));
    }
    Ok(())
}

fn main() {
    let detected = wooting::backend().detect(&DetectContext::default());
    if detected.is_empty() {
        println!("no Wooting V3 keyboard detected");
    }
    for d in detected {
        match d {
            Detected::Device(mut c) => {
                println!("{} ({})", c.description().info.name, c.description().info.id);
                if let Err(e) = show(c.as_mut()) {
                    println!("  error: {e}");
                }
                match c.release() {
                    Ok(()) => println!("  released: sent reset-all, keyboard shows its profile lighting"),
                    Err(e) => println!("  release failed: {e}"),
                }
            }
            Detected::Failed { location, error, .. } => println!("failed at {location}: {error}"),
        }
    }
}
