//! Probe for MSI Mystic Light boards. Run by hand only, never in CI:
//!
//! ```text
//! cargo run -p cc-drivers --example msi_mystic_light_probe
//! ```
//!
//! Enumerates the boards the tested detector table matches, reads the configuration report length (the class
//! selector), brings each 185-byte board up exactly as detection does and prints the description, firmware and the
//! configuration read from the board. It changes no LED state: the only writes are the two firmware version queries
//! (output reports `01 B0 ..` / `01 B6 ..`) that the reference detector also sends. `release()` is not called because
//! nothing was changed.

use std::process::ExitCode;

use cc_core::Controller;
use cc_drivers::msi_mystic_light::{self as msi, MysticLight185, protocol};

fn main() -> ExitCode {
    let devices = match cc_hid::enumerate() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("HID enumeration failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let candidates = msi::candidates(&devices);
    if candidates.is_empty() {
        println!("no Mystic Light board found (tested detector table, usage page 0x0001, usage 0x00)");
        return ExitCode::SUCCESS;
    }

    let mut ok = true;
    for cand in &candidates {
        println!("== {} ({:04x}:{:04x})", cand.id, cand.vid, cand.pid);
        println!("   path:   {}", cand.path);
        println!("   serial: {:?}", cand.serial);
        if let Err(e) = probe(cand) {
            println!("   FAILED: {e}");
            ok = false;
        }
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn probe(cand: &msi::Candidate) -> Result<(), cc_core::DeviceError> {
    let mut dev = cc_hid::open(&cand.path)?;
    let len = msi::read_config_len(dev.as_mut())?;
    println!("   0x52 report length: {len} -> {:?}", protocol::classify(len));

    let c = MysticLight185::open(dev, cand)?;
    let d = c.description();
    println!("   name:     {}", d.info.name);
    println!(
        "   board id: {:04x} (mixed mode: {})",
        c.board_id(),
        c.mixed_mode()
    );
    println!("   firmware: {}", c.firmware().display());
    println!("   sync per-LED protocol: {}", c.sync_direct());
    println!("   direct mode id: {:?}", d.direct_mode);
    for z in &d.zones {
        println!(
            "   zone {:<10} {:?} start {} len {} resizable {:?}",
            z.name, z.kind, z.start, z.len, z.resizable
        );
    }
    println!(
        "   modes: {}",
        d.modes
            .iter()
            .map(|m| format!("{}={}", m.id, m.name))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let original = c.original_config();
    println!(
        "   configuration read from the board (left in per-LED mode: {}):",
        original.is_direct()
    );
    for chunk in original.0.chunks(16) {
        println!("     {}", cc_hid::hex(chunk));
    }
    for (zone, msi_zone) in d.zones.iter().zip(c.msi_zones()) {
        if let Some(s) = original.zone_state(*msi_zone, c.mixed_mode()) {
            println!(
                "   {:<10} effect {:>3} speed {} brightness {:>2} custom color {} color {} color2 {} led count {:?}",
                zone.name,
                s.effect,
                s.speed,
                s.brightness,
                s.custom_color,
                s.color,
                s.color2,
                original.led_count(*msi_zone)
            );
        }
    }
    Ok(())
}
