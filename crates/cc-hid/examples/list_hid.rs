//! Prints every enumerated HID interface as
//! `vid:pid iface usage_page/usage product serial path`. Never opens a device.
//!
//! Run by hand: `cargo run -p cc-hid --example list_hid`.

#![allow(clippy::print_stdout)]

fn main() -> Result<(), cc_hid::HidError> {
    let mut devices = cc_hid::enumerate()?;
    devices.sort_by(|a, b| (a.vid, a.pid, a.interface, &a.path).cmp(&(b.vid, b.pid, b.interface, &b.path)));
    for d in &devices {
        println!(
            "{:04x}:{:04x} iface {:>2} {:04x}/{:04x} {:?} {:?} {}",
            d.vid,
            d.pid,
            d.interface,
            d.usage_page,
            d.usage,
            d.product.as_deref().unwrap_or("-"),
            d.serial.as_deref().unwrap_or("-"),
            d.path
        );
    }
    println!("{} HID interfaces", devices.len());
    Ok(())
}
