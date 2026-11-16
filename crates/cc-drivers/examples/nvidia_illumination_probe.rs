//! Read-only probe of NVIDIA Illumination GPUs. Never calls `SetControl`.
//!
//! Runs the driver's detection (which only reads `GetControl`), prints each device description, then reads the
//! current zone control once more through `cc-nvapi` and prints it decoded per zone.
//!
//! Run by hand only (not in CI): `cargo run -p cc-drivers --example nvidia_illumination_probe`.

use cc_core::{DetectContext, Detected};
use cc_drivers::nvidia_illumination::{self, mapping};
use cc_nvapi::NvApi;

fn main() -> std::process::ExitCode {
    let found = nvidia_illumination::backend().detect(&DetectContext::default());
    println!("detect: {} result(s)", found.len());
    for d in &found {
        match d {
            Detected::Device(c) => {
                let desc = c.description();
                println!("\n{} ({})", desc.info.name, desc.info.id);
                println!("  location: {}", desc.info.location);
                for z in &desc.zones {
                    println!("  zone {:?} leds {}..{}", z.name, z.start, z.start + z.len);
                }
                for m in &desc.modes {
                    println!(
                        "  mode {} {:?} caps {:?} brightness {:?}",
                        m.id, m.name, m.caps, m.brightness
                    );
                }
                println!("  direct mode: {:?}", desc.direct_mode);
            }
            Detected::Failed { location, error, .. } => println!("\nfailed at {location}: {error}"),
        }
    }
    // The controllers made no writes; dropping them needs no release.
    drop(found);

    let nvapi = match NvApi::load() {
        Ok(n) => n,
        Err(e) => {
            println!("NvApi::load failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let gpus = match nvapi.physical_gpus() {
        Ok(g) => g,
        Err(e) => {
            println!("physical_gpus failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    for (i, gpu) in gpus.iter().enumerate() {
        let Ok(ids) = nvapi.pci_ids(gpu) else { continue };
        let Some(entry) = nvidia_illumination::lookup(&ids) else {
            continue;
        };
        println!("\nGPU {i}: {} current control:", entry.name);
        match nvapi.illum_zones_get_control(gpu, false) {
            Ok(p) => {
                for (z, zone) in p.zones.iter().enumerate() {
                    println!(
                        "  zone {z}: kind={:?} mode={:?} manual={:?} shown as {:?}",
                        zone.kind,
                        zone.mode,
                        zone.manual,
                        mapping::current_color(zone)
                    );
                }
            }
            Err(e) => println!("  GetControl failed: {e}"),
        }
    }
    std::process::ExitCode::SUCCESS
}
