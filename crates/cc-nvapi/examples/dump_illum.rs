//! Read-only NvAPI illumination dump. Never calls `SetControl`.
//!
//! Loads NvAPI, lists every physical GPU with PCI ids and name, then prints the zone list and the
//! current and default zone control params, both decoded and as a hex dump of the bytes the driver
//! returned (header plus the active zone slots). The output is the first fixture for the Phase 3
//! `nvidia_illumination` driver.
//!
//! Run: `cargo run -p cc-nvapi --example dump_illum` (set `RUST_LOG=cc_nvapi=trace` for loader logs).

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use cc_nvapi::{NvApi, raw};

    init_logging();

    let nvapi = match NvApi::load() {
        Ok(nvapi) => nvapi,
        Err(e) => {
            println!("NvApi::load failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    println!(
        "NvAPI loaded. ZONE_CONTROL_PARAMS_VER_1 = {} ({:#010x}), struct size {} bytes",
        raw::ZONE_CONTROL_PARAMS_VER_1,
        raw::ZONE_CONTROL_PARAMS_VER_1,
        raw::ZONE_CONTROL_PARAMS_V1_SIZE
    );

    let gpus = match nvapi.physical_gpus() {
        Ok(gpus) => gpus,
        Err(e) => {
            println!("physical_gpus failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    println!("{} physical GPU(s)", gpus.len());

    for (i, gpu) in gpus.iter().enumerate() {
        println!("\n=== GPU {i} {gpu:?} ===");
        match nvapi.full_name(gpu) {
            Ok(name) => println!("name: {name}"),
            Err(e) => println!("name: error: {e}"),
        }
        match nvapi.pci_ids(gpu) {
            Ok(ids) => println!(
                "pci: {:04X}:{:04X} subsys {:04X}:{:04X} rev {:#x} ext_device_id {:#010x} \
                 (raw device_id {:#010x}, subsystem_id {:#010x})",
                ids.vendor(),
                ids.device(),
                ids.subsystem_vendor(),
                ids.subsystem_device(),
                ids.revision_id,
                ids.ext_device_id,
                ids.device_id,
                ids.subsystem_id
            ),
            Err(e) => println!("pci: error: {e}"),
        }
        match nvapi.illum_zones_info(gpu) {
            Ok(zones) => {
                println!("zones ({}):", zones.len());
                for z in zones {
                    println!("  {z:?}");
                }
            }
            Err(e) => println!("zones: error: {e}"),
        }
        for default_values in [false, true] {
            let label = if default_values { "default" } else { "current" };
            match nvapi.illum_zones_get_control(gpu, default_values) {
                Ok(params) => dump_params(label, &params),
                Err(e) => println!("{label} control: error: {e}"),
            }
        }
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(windows)]
fn dump_params(label: &str, params: &cc_nvapi::ZoneControlParams) {
    use cc_nvapi::raw;

    println!(
        "{label} control: default_values={} zones={}",
        params.default_values,
        params.zones.len()
    );
    for (i, z) in params.zones.iter().enumerate() {
        println!(
            "  zone {i}: kind={:?} mode={:?} manual={:?}",
            z.kind, z.mode, z.manual
        );
    }
    match params.to_raw() {
        Ok(raw_params) => {
            let bytes = raw_params.to_bytes();
            let active =
                raw::ZONE_CONTROL_PARAMS_V1_HEADER_SIZE + params.zones.len() * raw::ZONE_CONTROL_V1_SIZE;
            let trailing_nonzero = bytes[active..].iter().filter(|&&b| b != 0).count();
            println!(
                "  hex (bytes 0..{active} of {}; {trailing_nonzero} non-zero bytes in unused slots):",
                bytes.len()
            );
            hexdump(&bytes[..active]);
        }
        Err(e) => println!("  to_raw failed: {e}"),
    }
}

#[cfg(windows)]
fn hexdump(bytes: &[u8]) {
    use std::fmt::Write as _;
    for (row, chunk) in bytes.chunks(16).enumerate() {
        let mut line = format!("    {:04x}:", row * 16);
        for b in chunk {
            let _ = write!(line, " {b:02x}");
        }
        println!("{line}");
    }
}

#[cfg(windows)]
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("cc_nvapi=debug")),
        )
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(not(windows))]
fn main() {
    println!("dump_illum: NvAPI is only available on Windows");
}
