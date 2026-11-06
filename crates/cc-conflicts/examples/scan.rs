//! Runs a real conflict scan on this machine and prints the findings as a table.
//!
//! `cargo run -p cc-conflicts --example scan` (add `-- --openrgb-port` to probe 127.0.0.1:6742).
//! Not run in CI. Reads processes, services and the registry only; changes nothing.

#[cfg(windows)]
fn main() {
    use cc_conflicts::{Evidence, ScanOptions, Scanner};

    let opts = ScanOptions {
        check_openrgb_port: std::env::args().any(|a| a == "--openrgb-port"),
    };
    let report = Scanner::windows().scan(&opts);

    let mut out = String::new();
    out.push_str(&format!(
        "{:<26} {:<8} {:<7} {:<9} {:<8} {}\n",
        "entry", "severity", "running", "installed", "verified", "evidence"
    ));
    for f in &report.findings {
        let evidence: Vec<String> = f
            .evidence
            .iter()
            .map(|e| match e {
                Evidence::Process { pid, name, .. } => format!("process {name} ({pid})"),
                Evidence::Service { name, state, .. } => format!("service {name} {state:?}"),
                Evidence::Driver { name, state, .. } => format!("driver {name} {state:?}"),
                Evidence::Installed {
                    display_name,
                    version,
                    hive,
                } => {
                    format!(
                        "installed {display_name} {} {hive:?}",
                        version.as_deref().unwrap_or("-")
                    )
                }
                Evidence::Path(p) => format!("path {p}"),
                Evidence::DynamicLighting(dl) => format!(
                    "dynamic lighting ambient={} foreground={} devices={} providers={:?}",
                    dl.ambient_enabled, dl.foreground_controls, dl.device_count, dl.providers
                ),
                Evidence::OpenRgbPort => "openrgb sdk port 6742".to_owned(),
            })
            .collect();
        out.push_str(&format!(
            "{:<26} {:<8} {:<7} {:<9} {:<8} {}\n",
            f.entry_id,
            f.severity,
            f.running,
            f.installed,
            f.verified,
            evidence.join("; ")
        ));
    }
    for e in &report.errors {
        out.push_str(&format!("error: {e}\n"));
    }
    out.push_str(&format!("{} findings\n", report.findings.len()));

    use std::io::Write;
    std::io::stdout()
        .write_all(out.as_bytes())
        .expect("write to stdout");
}

#[cfg(not(windows))]
fn main() {}
