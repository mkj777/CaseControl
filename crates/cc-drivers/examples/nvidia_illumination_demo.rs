//! Hardware demo for NVIDIA Illumination GPUs. Writes to the GPU; run by hand only, never in CI.
//!
//! 1. Safety round trip per supported GPU: `GetControl`, `SetControl` with exactly those params, `GetControl`
//!    again, compare. Any difference (or error) aborts before any colour change.
//! 2. Detect through the driver, set every zone red, wait 3 s, green, 3 s, blue, 3 s, then `release()`. Release
//!    also runs on every error path. When every zone is brightness-only (SINGLE_COLOR / FIXED COLOR, as on the
//!    RTX 5080 FE), red, green and blue would all map to brightness 100, so the demo steps through white (100 %),
//!    50 % grey (brightness 50) and 10 % grey (brightness 10) instead.
//! 3. Read `GetControl` again and print what was restored, compared with the state from step 1.
//!
//! Run: `cargo run -p cc-drivers --example nvidia_illumination_demo`.

use std::process::ExitCode;
use std::time::Duration;

use cc_core::{Controller, DetectContext, Detected, DeviceError, Rgb};
use cc_drivers::nvidia_illumination::{self, mapping};
use cc_nvapi::{NvApi, ZoneControlParams};

fn main() -> ExitCode {
    let before = match round_trip() {
        Ok(b) if !b.is_empty() => b,
        Ok(_) => {
            println!("no supported NVIDIA Illumination GPU found");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            println!("ABORT: {e}");
            return ExitCode::FAILURE;
        }
    };

    let found = nvidia_illumination::backend().detect(&DetectContext::default());
    let mut ok = true;
    for d in found {
        match d {
            Detected::Device(mut c) => {
                println!("\n{} ({})", c.description().info.name, c.description().info.id);
                let shown = show_colors(c.as_mut());
                let released = c.release();
                if let Err(e) = &shown {
                    println!("  colour sequence failed: {e}");
                    ok = false;
                }
                match released {
                    Ok(()) => println!("  released"),
                    Err(e) => {
                        println!("  RELEASE FAILED: {e}");
                        ok = false;
                    }
                }
            }
            Detected::Failed { location, error, .. } => {
                println!("detect failed at {location}: {error}");
                ok = false;
            }
        }
    }

    match read_supported() {
        Ok(after) => {
            for ((i, name, b), (_, _, a)) in before.iter().zip(&after) {
                println!("\nGPU {i}: {name} restored state:");
                print_params(a);
                if a == b {
                    println!("  identical to the state before the demo");
                } else {
                    println!("  DIFFERS from the state before the demo:");
                    print_params(b);
                    ok = false;
                }
            }
        }
        Err(e) => {
            println!("reading the restored state failed: {e}");
            ok = false;
        }
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn show_colors(c: &mut dyn Controller) -> Result<(), DeviceError> {
    let desc = c.description();
    let n = desc.leds.len();
    let brightness_only = desc
        .zones
        .iter()
        .all(|z| z.name.ends_with("SINGLE COLOR") || z.name.ends_with("FIXED COLOR"));
    let steps = if brightness_only {
        [
            ("white (brightness 100)", Rgb::WHITE),
            ("50 % grey (brightness 50)", Rgb::new(128, 128, 128)),
            ("10 % grey (brightness 10)", Rgb::new(26, 26, 26)),
        ]
    } else {
        [
            ("red", Rgb::new(255, 0, 0)),
            ("green", Rgb::new(0, 255, 0)),
            ("blue", Rgb::new(0, 0, 255)),
        ]
    };
    for (name, color) in steps {
        println!("  {name}");
        c.set_direct(&vec![color; n])?;
        std::thread::sleep(Duration::from_secs(3));
    }
    Ok(())
}

/// GetControl -> SetControl(same) -> GetControl on every supported GPU; returns the state read first.
fn round_trip() -> Result<Vec<(usize, &'static str, ZoneControlParams)>, String> {
    let nvapi = NvApi::load().map_err(|e| format!("NvApi::load: {e}"))?;
    let mut out = Vec::new();
    for (i, gpu) in nvapi
        .physical_gpus()
        .map_err(|e| e.to_string())?
        .iter()
        .enumerate()
    {
        let Ok(ids) = nvapi.pci_ids(gpu) else { continue };
        let Some(entry) = nvidia_illumination::lookup(&ids) else {
            continue;
        };
        println!("GPU {i}: {} round trip", entry.name);
        let first = nvapi
            .illum_zones_get_control(gpu, false)
            .map_err(|e| format!("first GetControl: {e}"))?;
        print_params(&first);
        nvapi
            .illum_zones_set_control(gpu, &first)
            .map_err(|e| format!("SetControl of unchanged params: {e}"))?;
        let second = nvapi
            .illum_zones_get_control(gpu, false)
            .map_err(|e| format!("second GetControl: {e}"))?;
        let same_bytes = match (first.to_raw(), second.to_raw()) {
            (Ok(a), Ok(b)) => a.to_bytes() == b.to_bytes(),
            _ => false,
        };
        if first != second || !same_bytes {
            println!("  after round trip:");
            print_params(&second);
            return Err(format!(
                "GPU {i}: round trip changed the params, not touching colours"
            ));
        }
        println!("  round trip identical");
        out.push((i, entry.name, first));
    }
    Ok(out)
}

fn read_supported() -> Result<Vec<(usize, &'static str, ZoneControlParams)>, String> {
    let nvapi = NvApi::load().map_err(|e| format!("NvApi::load: {e}"))?;
    let mut out = Vec::new();
    for (i, gpu) in nvapi
        .physical_gpus()
        .map_err(|e| e.to_string())?
        .iter()
        .enumerate()
    {
        let Ok(ids) = nvapi.pci_ids(gpu) else { continue };
        let Some(entry) = nvidia_illumination::lookup(&ids) else {
            continue;
        };
        let p = nvapi
            .illum_zones_get_control(gpu, false)
            .map_err(|e| e.to_string())?;
        out.push((i, entry.name, p));
    }
    Ok(out)
}

fn print_params(p: &ZoneControlParams) {
    for (z, zone) in p.zones.iter().enumerate() {
        println!(
            "    zone {z}: kind={:?} mode={:?} manual={:?} shown as {:?}",
            zone.kind,
            zone.mode,
            zone.manual,
            mapping::current_color(zone)
        );
    }
}
