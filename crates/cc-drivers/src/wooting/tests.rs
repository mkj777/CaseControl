use std::collections::HashSet;

use cc_core::{Controller, DeviceError, ModeState, Rgb, ZoneKind};
use cc_hid::{FakeTransport, HidDeviceInfo};

use super::*;

fn hid(pid: u16, usage_page: u16, serial: Option<&str>, path: &str) -> HidDeviceInfo {
    HidDeviceInfo {
        path: path.to_owned(),
        vid: protocol::VID,
        pid,
        interface: 3,
        usage_page,
        usage: 1,
        serial: serial.map(str::to_owned),
        product: Some("Wooting 60HE (ARM)".to_owned()),
        manufacturer: Some("Wooting".to_owned()),
        release: 0x0100,
    }
}

fn info() -> HidDeviceInfo {
    hid(
        0x1310,
        0xFF55,
        Some("A02B2341W031H00001"),
        r"\\?\HID#VID_31E3&PID_1310&MI_03#test",
    )
}

fn controller(fake: &FakeTransport) -> WootingController {
    let info = info();
    let id = device_id(&info);
    let (desc, hw) = description(id, &info);
    WootingController::new(Box::new(fake.clone()), desc, hw).without_delay()
}

/// 67 colours: first LED (Escape) 0x12,0x34,0x56, last LED (Right Fn) 0xFF,0x00,0xFF, everything else black.
fn test_colors() -> Vec<Rgb> {
    let mut c = vec![Rgb::BLACK; 67];
    c[0] = Rgb::new(0x12, 0x34, 0x56);
    c[66] = Rgb::new(0xFF, 0x00, 0xFF);
    c
}

/// The full frame for [`test_colors`], derived from the reference by hand:
/// header `[4, 0xD1, 0xDA, 11]` (`WootingV3KeyboardController::SendDirect`), then the 6x21 framebuffer of
/// little-endian R5G6B5 words. Escape sits at hardware index 21 (`matrix_map_60[0][0]`), so bytes 4 + 42 = 46/47;
/// RGB32ToRGBcolor16(0x12,0x34,0x56) = 0x1000 | 0x01A0 | 0x000A = 0x11AA -> `AA 11`. Right Fn sits at hardware
/// index 118 (`matrix_map_60[4][13]`), so bytes 4 + 236 = 240/241; (0xFF,0,0xFF) -> 0xF800 | 0x001F = 0xF81F ->
/// `1F F8`. Every other byte up to 2046 is zero.
fn expected_frame() -> Vec<u8> {
    let mut f = vec![0u8; 2046];
    f[..4].copy_from_slice(&[0x04, 0xD1, 0xDA, 0x0B]);
    f[46] = 0xAA;
    f[47] = 0x11;
    f[240] = 0x1F;
    f[241] = 0xF8;
    f
}

const INIT: [u8; 8] = [0x01, 0xD1, 0xDA, 0x21, 0, 0, 0, 0];
const RESET: [u8; 8] = [0x01, 0xD1, 0xDA, 0x20, 0, 0, 0, 0];

#[test]
fn key_map_shape() {
    let (desc, hw) = description(DeviceId::hid(0x31E3, 0x1310, None), &info());
    desc.validate().expect("description is valid");
    // matrix_map_60: 14 + 14 + 14 + 13 + 12 keys.
    assert_eq!(desc.led_count(), 67);
    assert_eq!(hw.len(), 67);
    assert_eq!(desc.zones.len(), 1);
    let ZoneKind::Matrix { width, height, map } = &desc.zones[0].kind else {
        panic!("expected a matrix zone");
    };
    assert_eq!((*width, *height), (14, 5));
    let mapped: Vec<u32> = map.iter().flatten().copied().collect();
    assert_eq!(mapped.len(), desc.led_count());
    assert_eq!(
        mapped.iter().collect::<HashSet<_>>().len(),
        mapped.len(),
        "LED index twice"
    );
    assert_eq!(
        hw.iter().collect::<HashSet<_>>().len(),
        hw.len(),
        "hardware index twice"
    );
    assert!(hw.iter().all(|&h| usize::from(h) < protocol::HW_SLOTS));
    // Empty cells exactly where matrix_map_60 has NA: [3][12], [4][3], [4][9].
    let empty: Vec<usize> = map
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_none())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(empty, vec![3 * 14 + 12, 4 * 14 + 3, 4 * 14 + 9]);
    assert_eq!(desc.leds[0].name, "Key: Escape");
    assert_eq!(desc.leds[66].name, "Key: Right Fn");
    assert_eq!(desc.direct_mode, Some(DIRECT_MODE_ID));
    assert_eq!(desc.zones[0].name, "Wooting 60HE (ARM) zone");
}

#[test]
fn description_identity() {
    let info = info();
    let id = device_id(&info);
    let (desc, _) = description(id, &info);
    assert_eq!(desc.info.id.as_str(), "hid:31e3:1310:A02B2341W031H00001");
    assert_eq!(desc.info.location, info.path);
    assert_eq!(desc.info.serial.as_deref(), Some("A02B2341W031H00001"));
    assert_eq!(desc.info.name, "Wooting 60HE (ARM)");
}

#[test]
fn candidates_match_v3_interface_of_60he_arm_only() {
    let devices = vec![
        hid(0x1310, 0x0001, Some("S1"), "kbd"),    // keyboard collection, not V3
        hid(0x1310, 0xFF54, Some("S1"), "analog"), // analog interface
        hid(0x1310, 0xFF55, Some("S1"), "v3"),     // the one
        hid(0x1310, 0xFF55, Some("S1"), "v3-second"), // same serial again: ignored
        hid(0x1300, 0xFF55, Some("S2"), "60he-avr"), // other model
        hid(0x1312, 0xFF55, Some("S3"), "none-mode"), // NONE_PID variant
        hid(0x1311, 0xFF55, None, "dinp-a"),       // DINP_PID, no serial
        hid(0x1311, 0xFF55, Some("0000"), "dinp-b"), // all-zero serial counts as absent
    ];
    let mut other_vendor = hid(0x1310, 0xFF55, Some("X"), "other");
    other_vendor.vid = 0x03EB;
    let mut all = devices;
    all.push(other_vendor);

    let got: Vec<(String, &str)> = candidates(&all)
        .into_iter()
        .map(|(id, d)| (id.as_str().to_owned(), d.path.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("hid:31e3:1310:S1".to_owned(), "v3"),
            ("hid:31e3:1310:S3".to_owned(), "none-mode"),
            ("hid:31e3:1310:#0".to_owned(), "dinp-a"),
            ("hid:31e3:1310:#1".to_owned(), "dinp-b"),
        ]
    );
}

#[test]
fn id_uses_base_pid_for_every_emulation_variant() {
    for pid in [0x1310, 0x1311, 0x1312] {
        let d = hid(
            pid,
            0xFF55,
            Some("SER"),
            &format!(r"\\?\HID#VID_31E3&PID_{pid:04X}&MI_03#x"),
        );
        let id = device_id(&d);
        assert_eq!(id.as_str(), "hid:31e3:1310:SER");
        let (desc, _) = description(id, &d);
        // The enumerated pid stays visible in the location for diagnostics.
        assert!(desc.info.location.contains(&format!("PID_{pid:04X}")));
    }
}

#[test]
fn single_serialless_device_has_no_ordinal() {
    let devices = [hid(0x1310, 0xFF55, None, "p")];
    let got = candidates(&devices);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0.as_str(), "hid:31e3:1310");
}

#[test]
fn detect_respects_disabled_family() {
    let mut ctx = DetectContext::default();
    ctx.disabled_families.insert(family::WOOTING.to_owned());
    assert!(backend().detect(&ctx).is_empty());
}

#[test]
fn init_then_frame() {
    let fake = FakeTransport::builder()
        .feature(INIT)
        .write(expected_frame())
        .build();
    let mut c = controller(&fake);
    c.set_direct(&test_colors()).expect("set_direct");
    fake.finish().expect("script consumed");
}

#[test]
fn init_is_sent_once() {
    let fake = FakeTransport::builder()
        .feature(INIT)
        .write(expected_frame())
        .write(expected_frame())
        .build();
    let mut c = controller(&fake);
    c.set_direct(&test_colors()).expect("first frame");
    c.set_zone_direct(0, &test_colors()).expect("second frame");
    fake.finish().expect("script consumed");
}

#[test]
fn black_frame_after_colored_one_clears_slots() {
    let mut black = vec![0u8; 2046];
    black[..4].copy_from_slice(&[0x04, 0xD1, 0xDA, 0x0B]);
    let fake = FakeTransport::builder()
        .feature(INIT)
        .write(expected_frame())
        .write(black)
        .build();
    let mut c = controller(&fake);
    c.set_direct(&test_colors()).expect("colored");
    c.set_direct(&[Rgb::BLACK; 67]).expect("black");
    fake.finish().expect("script consumed");
}

#[test]
fn set_mode_direct_sends_frame() {
    let fake = FakeTransport::builder()
        .feature(INIT)
        .write(expected_frame())
        .build();
    let mut c = controller(&fake);
    let state = ModeState {
        mode: DIRECT_MODE_ID,
        speed: None,
        brightness: None,
        direction: None,
        colors: test_colors(),
    };
    c.set_mode(&state).expect("set_mode");
    fake.finish().expect("script consumed");
}

#[test]
fn set_mode_rejects_unknown_mode_without_io() {
    let fake = FakeTransport::builder().build();
    let mut c = controller(&fake);
    let state = ModeState {
        mode: 1,
        speed: None,
        brightness: None,
        direction: None,
        colors: Vec::new(),
    };
    assert!(matches!(c.set_mode(&state), Err(DeviceError::InvalidArgument(_))));
    fake.finish().expect("nothing sent");
}

#[test]
fn wrong_color_count_is_rejected_without_io() {
    let fake = FakeTransport::builder().build();
    let mut c = controller(&fake);
    assert!(matches!(
        c.set_direct(&[Rgb::WHITE; 3]),
        Err(DeviceError::InvalidArgument(_))
    ));
    assert!(matches!(
        c.set_zone_direct(1, &test_colors()),
        Err(DeviceError::InvalidArgument(_))
    ));
    fake.finish().expect("nothing sent");
}

#[test]
fn release_sends_reset_all() {
    let fake = FakeTransport::builder()
        .feature(INIT)
        .write(expected_frame())
        .feature(RESET)
        .build();
    let mut c = controller(&fake);
    c.set_direct(&test_colors()).expect("frame");
    c.release().expect("release");
    fake.finish().expect("script consumed");
}

#[test]
fn release_then_set_direct_reinitialises() {
    let fake = FakeTransport::builder()
        .feature(INIT)
        .write(expected_frame())
        .feature(RESET)
        .feature(INIT)
        .write(expected_frame())
        .build();
    let mut c = controller(&fake);
    c.set_direct(&test_colors()).expect("frame");
    c.release().expect("release");
    c.set_direct(&test_colors()).expect("frame after release");
    fake.finish().expect("script consumed");
}

#[test]
fn release_twice_is_harmless() {
    let fake = FakeTransport::builder().feature(RESET).feature(RESET).build();
    let mut c = controller(&fake);
    c.release().expect("first release");
    c.release().expect("second release");
    fake.finish().expect("script consumed");
}

#[test]
fn failed_init_does_not_block_frames() {
    // The script expects the frame first, so the init fails with a script error. Like the reference, the
    // controller logs that and still sends the frame.
    let fake = FakeTransport::builder().write(expected_frame()).build();
    let mut c = controller(&fake);
    // The failed fake also rejects the frame; what matters is that the frame was attempted after the init.
    let _ = c.set_direct(&test_colors());
    assert_eq!(fake.recorded(), vec![INIT.to_vec(), expected_frame()]);
}
