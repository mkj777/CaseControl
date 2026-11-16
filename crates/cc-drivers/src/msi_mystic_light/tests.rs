//! Detection and controller call sequences against `FakeTransport` scripts. No test touches hardware.

use std::cell::Cell;

use cc_core::{Controller, DetectContext, Detected, DeviceError, DeviceId, ModeState, Rgb, ZoneKind};
use cc_hid::{FakeTransport, HidDeviceInfo, HidError, HidTransport, ScriptBuilder};

use super::protocol::tests::expected_enable_sync;
use super::protocol::{self, CONFIG_LEN, MsiZone, PER_LED_LEN, firmware_request};
use super::*;

const PATH: &str =
    r"\\?\HID#VID_1462&PID_7D75&Col01#7&1a2b3c4d&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";

fn hid(vid: u16, pid: u16, usage_page: u16, usage: u16, serial: Option<&str>, path: &str) -> HidDeviceInfo {
    HidDeviceInfo {
        path: path.to_owned(),
        vid,
        pid,
        interface: -1,
        usage_page,
        usage,
        serial: serial.map(str::to_owned),
        product: Some("MYSTIC LIGHT ".to_owned()),
        manufacturer: Some("MSI".to_owned()),
        release: 1,
    }
}

fn candidate(pid: u16, serial: Option<&str>) -> Candidate {
    Candidate {
        path: PATH.to_owned(),
        vid: 0x1462,
        pid,
        serial: serial.map(str::to_owned),
        id: DeviceId::hid(0x1462, pid, serial),
    }
}

/// A configuration as read from the board: a byte pattern, report id 0x52, JRAINBOW1/2 counts 0, JRAINBOW3's count byte (offset 63) at
/// 120 so the synchronized protocol applies, and a save byte of 1 so tests can see it forced to 0.
fn original() -> [u8; CONFIG_LEN] {
    let mut b = [0u8; CONFIG_LEN];
    for (i, v) in b.iter_mut().enumerate() {
        *v = u8::try_from((i * 7 + 3) % 256).expect("fits");
    }
    b[0] = 0x52;
    b[63] = 120;
    // JRAINBOW1/2 count bytes: headers start with 0 LEDs (tests resize them explicitly).
    b[41] = 0;
    b[52] = 0;
    b[184] = 1;
    b
}

/// What the board reports as its current configuration before an update (another pattern).
fn current() -> [u8; CONFIG_LEN] {
    let mut b = [0u8; CONFIG_LEN];
    for (i, v) in b.iter_mut().enumerate() {
        *v = u8::try_from((i * 13 + 1) % 256).expect("fits");
    }
    b[0] = 0x52;
    b
}

fn with_save(mut b: [u8; CONFIG_LEN], save: u8) -> Vec<u8> {
    b[184] = save;
    b.to_vec()
}

fn fw_response(version: u8) -> Vec<u8> {
    let mut r = vec![0u8; 64];
    r[0] = 0x01;
    r[2] = version;
    r
}

/// ReadFwVersion (APROM 1.2, LDROM 3.4).
fn firmware_steps(s: ScriptBuilder, aprom: u8) -> ScriptBuilder {
    s.write(firmware_request(0xB0))
        .read(fw_response(aprom))
        .write(firmware_request(0xB6))
        .read(fw_response(0x34))
}

/// Detection read (200-byte buffer), firmware queries, configuration read.
fn open_steps(s: ScriptBuilder) -> ScriptBuilder {
    open_steps_with(s, original())
}

fn open_steps_with(s: ScriptBuilder, cfg: [u8; CONFIG_LEN]) -> ScriptBuilder {
    firmware_steps(s.feature_reply(0x52, cfg[1..].to_vec()), 0x12).feature_reply(0x52, cfg[1..].to_vec())
}

fn open_7d75(rest: impl FnOnce(ScriptBuilder) -> ScriptBuilder) -> (MysticLight185, FakeTransport) {
    let fake = rest(open_steps(FakeTransport::builder())).build();
    let c = MysticLight185::open(Box::new(fake.clone()), &candidate(0x7D75, None)).expect("opens");
    (c, fake)
}

/// A per-LED report 0x53 with the given LED positions set.
fn per_led(hdr1: u8, hdr2: u8, leds: &[(usize, Rgb)]) -> Vec<u8> {
    let mut v = vec![0u8; PER_LED_LEN];
    v[..5].copy_from_slice(&[0x53, 0x25, hdr1, hdr2, 0x00]);
    for (p, c) in leds {
        v[5 + 3 * p..8 + 3 * p].copy_from_slice(&[c.r, c.g, c.b]);
    }
    v
}

const C1: Rgb = Rgb::new(1, 2, 3);
const C2: Rgb = Rgb::new(4, 5, 6);
const A: Rgb = Rgb::new(0x10, 0x11, 0x12);
const B: Rgb = Rgb::new(0x20, 0x21, 0x22);
const C: Rgb = Rgb::new(0x30, 0x31, 0x32);
const D: Rgb = Rgb::new(0x40, 0x41, 0x42);
const E: Rgb = Rgb::new(0x50, 0x51, 0x52);

/// The direct frame for MS-7D75 with JRGB1 = C1, JRGB2 = C2, JRAINBOW1 = [A, B, C], JRAINBOW2 = [D, E].
///
/// Sync layout from the constructor: onboard 0 LEDs, pipes 0, so per_led_jrgb = LED 0 (JRGB1 at 0, JRGB2 at 1),
/// per_led_jrainbow1_sync = jrgb + numof_JRGBs (2) = LED 2, per_led_jrainbow2_sync = 2 + 40 = LED 42.
fn expected_frame_7d75() -> Vec<u8> {
    per_led(
        0x06,
        0x00,
        &[(0, C1), (1, C2), (2, A), (3, B), (4, C), (42, D), (43, E)],
    )
}

fn frame_colors() -> Vec<Rgb> {
    vec![C1, C2, A, B, C, D, E]
}

#[test]
fn description_of_7d75() {
    let (c, fake) = open_7d75(|s| s);
    let d = c.description();
    assert_eq!(d.validate(), Ok(()));
    assert_eq!(d.info.id.as_str(), "hid:1462:7d75");
    assert_eq!(d.info.name, "MSI MAG B650 TOMAHAWK WIFI");
    assert_eq!(d.info.vendor, "MSI");
    assert_eq!(d.info.location, PATH);
    assert_eq!(d.info.firmware.as_deref(), Some("AP/LD 1.2 / 3.4"));
    let zones: Vec<_> = d
        .zones
        .iter()
        .map(|z| (z.name.as_str(), z.len, z.resizable.clone()))
        .collect();
    assert_eq!(
        zones,
        vec![
            ("JRGB1", 1, None),
            ("JRGB2", 1, None),
            ("JRAINBOW1", 0, Some(0..=200)),
            ("JRAINBOW2", 0, Some(0..=240)),
        ]
    );
    assert_eq!(d.zones[0].kind, ZoneKind::Single);
    assert_eq!(d.zones[2].kind, ZoneKind::Linear);
    assert_eq!(d.leds[0].name, "JRGB1, LED 1");
    assert_eq!(d.direct_mode, Some(100));
    assert_eq!(d.modes.len(), 23);
    assert_eq!(d.modes[0].name, "Direct");
    assert_eq!(d.keepalive, None);
    assert_eq!(c.original_config().0, original());
    assert!(c.mixed_mode());
    assert!(c.sync_direct());
    // The working copy carries the header sizes (0) in the JRAINBOW count bytes.
    assert_eq!(c.working_config().0[41], 0);
    assert_eq!(c.working_config().0[52], 0);
    fake.finish().expect("script consumed");
}

#[test]
fn direct_sequence_and_release() {
    let frame = expected_frame_7d75();
    let (mut c, fake) = open_7d75(|s| {
        s.feature(expected_enable_sync()) // SetDirectMode(true) -> SelectPerLedProtocol
            .feature(frame.clone()) //         Update: sync report
            .feature(frame.clone()) //         second frame: report only
            .feature_reply(0x52, current()[1..].to_vec()) // release: ReadSettings
            .feature(with_save(current(), 0)) //            current state back
            .feature(with_save(original(), 0)) //           the configuration read at open, save byte 0
            .feature(expected_enable_sync()) //             release-then-set_direct: enable again
            .feature(frame.clone())
    });
    c.resize_zone(2, 3).expect("resize JRAINBOW1");
    c.resize_zone(3, 2).expect("resize JRAINBOW2");
    assert_eq!(c.description().led_count(), 7);
    assert_eq!(c.description().leds[2].name, "JRAINBOW1, LED 1");
    c.set_direct(&frame_colors()).expect("frame 1");
    c.set_direct(&frame_colors()).expect("frame 2");
    c.release().expect("release");
    c.release().expect("second release is a no-op");
    c.set_direct(&frame_colors()).expect("frame after release");
    fake.finish().expect("script consumed");
}

#[test]
fn set_mode_direct_resends_enable_then_frame() {
    let frame = expected_frame_7d75();
    let (mut c, fake) = open_7d75(|s| {
        s.feature(expected_enable_sync())
            .feature(frame.clone())
            .feature(expected_enable_sync())
            .feature(frame.clone())
    });
    c.resize_zone(2, 3).expect("resize");
    c.resize_zone(3, 2).expect("resize");
    let state = ModeState {
        mode: 100,
        speed: None,
        brightness: None,
        direction: None,
        colors: frame_colors(),
    };
    c.set_mode(&state).expect("direct mode");
    // SetDirectMode(true) always re-sends the enable message.
    c.set_mode(&state).expect("direct mode again");
    fake.finish().expect("script consumed");
}

#[test]
fn zone_direct_sends_whole_frame() {
    // DeviceUpdateZoneLEDs updates one zone's LEDs, then Update sends the full sync report.
    let expected = per_led(0x06, 0x00, &[(42, D), (43, E)]);
    let (mut c, fake) = open_7d75(|s| s.feature(expected_enable_sync()).feature(expected));
    c.resize_zone(3, 2).expect("resize");
    c.set_zone_direct(3, &[D, E]).expect("zone");
    assert!(matches!(
        c.set_zone_direct(3, &[D]),
        Err(DeviceError::InvalidArgument(_))
    ));
    assert!(matches!(
        c.set_zone_direct(9, &[]),
        Err(DeviceError::InvalidArgument(_))
    ));
    fake.finish().expect("script consumed");
}

#[test]
fn non_sync_direct_uses_one_report_per_header() {
    // JRAINBOW1 with 41 LEDs exceeds SYNC_PER_LED_MODE_JRAINBOW_LED_COUNT (40): SelectPerLedProtocol picks the
    // unsynchronized protocol. Enable message: see protocol::tests::enable_direct_non_sync_is_byte_exact.
    let mut enable = expected_enable_sync();
    enable[31] = 0x25;
    enable[41] = 0xC8;
    enable[42] = 0x25;
    enable[52] = 0xF0;
    enable[82] = 0xB1;
    let mut jr1_colors = vec![Rgb::BLACK; 41];
    jr1_colors[0] = A;
    jr1_colors[40] = B;
    // Update: JRAINBOW1 report (hdr 04 00), 13 ms, JRAINBOW2 report (hdr 04 01), 13 ms, no JCORSAIR on this board,
    // then the on-board/sync report with JRGB1/2 at LEDs 0 and 1.
    let jr1 = per_led(0x04, 0x00, &[(0, A), (40, B)]);
    let jr2 = per_led(0x04, 0x01, &[(0, D)]);
    let onboard = per_led(0x06, 0x00, &[(0, C1), (1, C2)]);
    let (mut c, fake) = open_7d75(|s| s.feature(enable).feature(jr1).feature(jr2).feature(onboard));
    c.resize_zone(2, 41).expect("resize");
    c.resize_zone(3, 1).expect("resize");
    assert!(!c.sync_direct());
    let mut colors = vec![C1, C2];
    colors.extend(jr1_colors);
    colors.push(D);
    c.set_direct(&colors).expect("frame");
    fake.finish().expect("script consumed");
}

#[test]
fn resize_in_direct_mode_resends_enable() {
    let (mut c, fake) = open_7d75(|s| {
        s.feature(expected_enable_sync())
            .feature(per_led(0x06, 0x00, &[(0, C1), (1, C2)]))
            .feature(expected_enable_sync())
    });
    c.set_direct(&[C1, C2]).expect("frame");
    c.resize_zone(2, 10).expect("resize");
    assert_eq!(c.description().led_count(), 12);
    assert!(matches!(
        c.resize_zone(2, 201),
        Err(DeviceError::InvalidArgument(_))
    ));
    assert_eq!(c.resize_zone(0, 1), Err(DeviceError::Unsupported));
    assert!(matches!(
        c.resize_zone(7, 1),
        Err(DeviceError::InvalidArgument(_))
    ));
    fake.finish().expect("script consumed");
}

#[test]
fn static_mode_packet_is_byte_exact() {
    // SetMode + SetZoneColor for JRGB1 (offset 1) and JRGB2 (mixed mode: on_board_led_6 slot, offset 134), each with
    // MSI_MODE_STATIC, speed 0 (Static has no speed), brightness LEVEL_100: flags (10 << 2) | 0 = 0x28,
    // colorFlags |= 0x80, padding 0. MS-7D75 has no on-board zone, so both mirror into on_board_led (74):
    // effect 1, colorFlags |= SYNC_ONBOARD (0x01), padding 0; mode <= LIGHTNING clears 0x80 of @78 and 0x30 of @82.
    // JRAINBOW zones have 0 LEDs and are skipped; their count bytes (41, 52) hold the size 0.
    let o = original();
    let mut e = o;
    e[41] = 0;
    e[52] = 0;
    e[1] = 0x01;
    e[2..5].copy_from_slice(&[0xFF, 0x00, 0x00]);
    e[5] = 0x28;
    e[6..9].copy_from_slice(&[0xFF, 0x00, 0x00]);
    e[9] = o[9] | 0x80;
    e[10] = 0;
    e[134] = 0x01;
    e[135..138].copy_from_slice(&[0x00, 0xFF, 0x00]);
    e[138] = 0x28;
    e[139..142].copy_from_slice(&[0x00, 0xFF, 0x00]);
    e[142] = o[142] | 0x80;
    e[143] = 0;
    e[74] = 0x01;
    e[75..78].copy_from_slice(&[0x00, 0xFF, 0x00]); // JRGB2 is mirrored last
    e[78] = o[78] & !0x80;
    e[79..82].copy_from_slice(&[0x00, 0xFF, 0x00]);
    e[82] = (o[82] | 0x01) & !0x30;
    e[83] = 0;
    e[184] = 0;

    let (mut c, fake) = open_7d75(|s| {
        s.feature_reply(0x52, current()[1..].to_vec()) // Update: ReadSettings
            .feature(with_save(current(), 0)) //            old state first
            .feature(e.to_vec()) //                         new state
    });
    let state = ModeState {
        mode: 1,
        speed: None,
        brightness: None,
        direction: None,
        colors: vec![Rgb::new(0xFF, 0, 0), Rgb::new(0, 0xFF, 0)],
    };
    c.set_mode(&state).expect("static");
    fake.finish().expect("script consumed");
}

#[test]
fn random_mode_packet_and_save() {
    // Rainbow wave (26, RANDOM_ONLY) at speed HIGH (2), brightness 5: flags (5 << 2) | 2 = 0x16, colorFlags &= !0x80
    // (rainbow_color = true). Zones with LEDs: JRGB1 (1), JRGB2 (134), JRAINBOW1 (31, resized to 10 -> count byte
    // 41 = 10). Mode > LIGHTNING sets SYNC_JRGB (0x80 of @78) and SYNC_JPIPE1/2 (0x30 of @82); JRGB zones mirror the
    // effect into @74 and set SYNC_ONBOARD. Colors come from the color buffer (black).
    let o = original();
    let mut e = o;
    for (off, cf) in [(1, 9), (134, 142), (31, 39)] {
        e[off] = 26;
        e[off + 1..off + 4].copy_from_slice(&[0, 0, 0]);
        e[off + 4] = 0x16;
        e[off + 5..off + 8].copy_from_slice(&[0, 0, 0]);
        e[cf] = o[cf] & 0x7F;
        e[off + 9] = 0;
    }
    e[41] = 10;
    e[52] = 0;
    e[74] = 26;
    e[75..78].copy_from_slice(&[0, 0, 0]);
    e[78] = o[78] | 0x80;
    e[79..82].copy_from_slice(&[0, 0, 0]);
    e[82] = o[82] | 0x01 | 0x30;
    e[83] = 0;
    e[184] = 0;
    let mut saved = e;
    saved[184] = 1;

    let (mut c, fake) = open_7d75(|s| {
        s.feature_reply(0x52, current()[1..].to_vec())
            .feature(with_save(current(), 0))
            .feature(e.to_vec())
            // save_to_hardware: Update(true) sets save_data on both reports.
            .feature_reply(0x52, current()[1..].to_vec())
            .feature(with_save(current(), 1))
            .feature(saved.to_vec())
            // release: current state back, then the original with save 0.
            .feature_reply(0x52, current()[1..].to_vec())
            .feature(with_save(current(), 0))
            .feature(with_save(o, 0))
    });
    c.resize_zone(2, 10).expect("resize");
    let state = ModeState {
        mode: 26,
        speed: Some(2),
        brightness: Some(5),
        direction: None,
        colors: vec![],
    };
    c.set_mode(&state).expect("rainbow wave");
    c.save_to_hardware().expect("save");
    c.release().expect("release");
    fake.finish().expect("script consumed");
}

#[test]
fn invalid_mode_states_send_nothing() {
    let (mut c, fake) = open_7d75(|s| s);
    let bad_speed = ModeState {
        mode: 1,
        speed: Some(1),
        brightness: None,
        direction: None,
        colors: vec![Rgb::BLACK; 2],
    };
    assert!(matches!(
        c.set_mode(&bad_speed),
        Err(DeviceError::InvalidArgument(_))
    ));
    let unknown = ModeState {
        mode: 9,
        speed: None,
        brightness: None,
        direction: None,
        colors: vec![],
    };
    assert!(matches!(
        c.set_mode(&unknown),
        Err(DeviceError::InvalidArgument(_))
    ));
    assert!(matches!(
        c.set_direct(&[Rgb::BLACK]),
        Err(DeviceError::InvalidArgument(_))
    ));
    // Nothing was sent, so release has nothing to restore.
    c.release().expect("release");
    fake.finish().expect("script consumed");
}

#[test]
fn save_in_direct_mode_is_unsupported() {
    let (mut c, fake) = open_7d75(|s| {
        s.feature(expected_enable_sync())
            .feature(per_led(0x06, 0x00, &[(0, C1), (1, C2)]))
    });
    c.set_direct(&[C1, C2]).expect("frame");
    assert_eq!(c.save_to_hardware(), Err(DeviceError::Unsupported));
    fake.finish().expect("script consumed");
}

fn detect_one(fake: &FakeTransport, devices: &[HidDeviceInfo]) -> Vec<Detected> {
    let f = fake.clone();
    detect_with(&DetectContext::default(), devices, move |_| {
        Ok(Box::new(f.clone()) as Box<dyn HidTransport>)
    })
}

#[test]
fn detect_finds_7d75_on_the_right_collection() {
    let fake = open_steps(FakeTransport::builder()).build();
    let devices = [
        hid(0x1462, 0x7D75, 0x0001, 0x00, None, PATH),
        hid(0x1462, 0x7D75, 0xFF00, 0x01, None, "other collection"),
        hid(0x046D, 0x0A78, 0x0001, 0x00, None, "logitech"),
    ];
    let found = detect_one(&fake, &devices);
    assert_eq!(found.len(), 1);
    match &found[0] {
        Detected::Device(c) => {
            assert_eq!(c.description().info.id.as_str(), "hid:1462:7d75");
            assert_eq!(c.description().info.location, PATH);
        }
        other => panic!("expected a device, got {other:?}"),
    }
    fake.finish().expect("script consumed");
}

#[test]
fn detect_accepts_186_byte_answer() {
    // `packet_length <= sizeof(FeaturePacket_185) + 1`.
    let cfg = original();
    let mut long = cfg[1..].to_vec();
    long.push(0);
    let fake = firmware_steps(FakeTransport::builder().feature_reply(0x52, long), 0x12)
        .feature_reply(0x52, cfg[1..].to_vec())
        .build();
    let found = detect_one(&fake, &[hid(0x1462, 0x7D75, 1, 0, None, PATH)]);
    assert!(matches!(found[..], [Detected::Device(_)]));
    fake.finish().expect("script consumed");
}

#[test]
fn detect_fails_other_classes_without_further_io() {
    // 162 and 112 bytes select classes that are not ported; 184/200/761 select the reference's 761-byte path, which
    // is not ported either. Nothing but the 0x52 read happens.
    for len in [162usize, 163, 112, 113, 184, 187, 200] {
        let fake = FakeTransport::builder()
            .feature_reply(0x52, vec![0u8; len - 1])
            .build();
        let found = detect_one(&fake, &[hid(0x1462, 0x7D75, 1, 0, None, PATH)]);
        match &found[..] {
            [
                Detected::Failed {
                    family,
                    location,
                    error,
                },
            ] => {
                assert_eq!(family, FAMILY);
                assert_eq!(location, PATH);
                assert!(
                    matches!(error, DeviceError::Protocol(m) if m.contains(&len.to_string())),
                    "{error}"
                );
            }
            other => panic!("len {len}: {other:?}"),
        }
        fake.finish().expect("script consumed");
    }
}

#[test]
fn detect_applies_firmware_gate() {
    // MPG B550 GAMING PLUS (7C56) requires APROM >= 6; version 5 fails before the configuration is read.
    let cfg = original();
    let fake = firmware_steps(
        FakeTransport::builder().feature_reply(0x52, cfg[1..].to_vec()),
        0x05,
    )
    .build();
    let found = detect_one(&fake, &[hid(0x1462, 0x7C56, 1, 0, None, PATH)]);
    assert!(matches!(
        &found[..],
        [Detected::Failed {
            error: DeviceError::Protocol(_),
            ..
        }]
    ));
    fake.finish().expect("script consumed");

    let fake = firmware_steps(
        FakeTransport::builder().feature_reply(0x52, cfg[1..].to_vec()),
        0x06,
    )
    .feature_reply(0x52, cfg[1..].to_vec())
    .build();
    let found = detect_one(&fake, &[hid(0x1462, 0x7C56, 1, 0, None, PATH)]);
    assert!(matches!(&found[..], [Detected::Device(_)]));
    fake.finish().expect("script consumed");
}

#[test]
fn detect_fails_zone_based_boards_before_firmware_query() {
    let fake = FakeTransport::builder()
        .feature_reply(0x52, original()[1..].to_vec())
        .build();
    let found = detect_one(&fake, &[hid(0x1462, 0x7C84, 1, 0, None, PATH)]);
    assert!(matches!(&found[..], [Detected::Failed { .. }]));
    fake.finish().expect("script consumed");
}

#[test]
fn detect_reports_open_errors() {
    let found = detect_with(
        &DetectContext::default(),
        &[hid(0x1462, 0x7D75, 1, 0, None, PATH)],
        |_| Err(HidError::NotFound("gone".into())),
    );
    assert!(matches!(
        &found[..],
        [Detected::Failed {
            error: DeviceError::Transport(_),
            ..
        }]
    ));
}

#[test]
fn detect_respects_skip_and_disabled_family() {
    let opened = Cell::new(0);
    let devices = [hid(0x1462, 0x7D75, 1, 0, None, PATH)];
    let open = |_: &str| -> Result<Box<dyn HidTransport>, HidError> {
        opened.set(opened.get() + 1);
        Err(HidError::NotFound("unused".into()))
    };

    let skip = DetectContext {
        skip: [DeviceId::hid(0x1462, 0x7D75, None)].into(),
        ..Default::default()
    };
    assert!(detect_with(&skip, &devices, open).is_empty());

    let disabled = DetectContext {
        disabled_families: [FAMILY.to_owned()].into(),
        ..Default::default()
    };
    assert!(detect_with(&disabled, &devices, open).is_empty());
    assert!(backend().detect(&disabled).is_empty());
    assert_eq!(opened.get(), 0);
}

#[test]
fn candidate_ids_and_ordinals() {
    let two = [
        hid(0x1462, 0x7D75, 1, 0, None, "a"),
        hid(0x1462, 0x7D75, 1, 0, Some(""), "b"),
    ];
    let ids: Vec<_> = candidates(&two)
        .into_iter()
        .map(|c| c.id.as_str().to_owned())
        .collect();
    assert_eq!(ids, vec!["hid:1462:7d75:#0", "hid:1462:7d75:#1"]);

    let serials = [
        hid(0x1462, 0x7D75, 1, 0, Some("A1"), "a"),
        hid(0x1462, 0x7D75, 1, 0, Some("B2"), "b"),
        hid(0x0DB0, 0x0076, 1, 0, Some("7E12XYZ"), "c"),
    ];
    let ids: Vec<_> = candidates(&serials)
        .into_iter()
        .map(|c| c.id.as_str().to_owned())
        .collect();
    assert_eq!(
        ids,
        vec!["hid:1462:7d75:A1", "hid:1462:7d75:B2", "hid:0db0:0076:7E12XYZ"]
    );
}

#[test]
fn common_pid_board_id_from_serial() {
    assert_eq!(board_id(0x7D75, None), Ok(0x7D75));
    assert_eq!(board_id(0x0076, Some("7E12ABCDEF")), Ok(0x7E12));
    assert!(board_id(0x0076, None).is_err());
    assert!(board_id(0x0076, Some("XYZ1")).is_err());
}

#[test]
fn backend_identity() {
    let b = backend();
    assert_eq!(b.name(), "msi_mystic_light");
    assert_eq!(b.bus(), cc_core::Bus::Hid);
    let (tx, _rx) = std::sync::mpsc::channel();
    assert!(b.subscribe_hotplug(tx).is_none());
}

#[test]
fn header_sizes_come_from_the_config_read_at_open() {
    // cycle_or_led_num of JRAINBOW1 (@41) = 30 and JRAINBOW2 (@52) = 12 become the initial zone lengths.
    let mut cfg = original();
    cfg[41] = 30;
    cfg[52] = 12;
    let fake = open_steps_with(FakeTransport::builder(), cfg).build();
    let c = MysticLight185::open(Box::new(fake.clone()), &candidate(0x7D75, None)).expect("opens");
    let lens: Vec<_> = c.description().zones.iter().map(|z| z.len).collect();
    assert_eq!(lens, vec![1, 1, 30, 12]);
    assert_eq!(c.description().led_count(), 44);
    assert_eq!(c.description().validate(), Ok(()));
    assert!(c.sync_direct());
    assert_eq!((c.working_config().0[41], c.working_config().0[52]), (30, 12));
    fake.finish().expect("script consumed");

    // A count above the zone's range is clamped (JRAINBOW2 max 240) and above 40 selects the non-sync protocol.
    cfg[52] = 250;
    let fake = open_steps_with(FakeTransport::builder(), cfg).build();
    let c = MysticLight185::open(Box::new(fake.clone()), &candidate(0x7D75, None)).expect("opens");
    assert_eq!(c.description().zones[3].len, 240);
    assert_eq!(c.working_config().0[52], 240);
    assert!(!c.sync_direct());
    fake.finish().expect("script consumed");
}

#[test]
fn hardware_mode_keeps_header_counts() {
    // Static on every zone: JRAINBOW1/2 now have LEDs, so SetMode/SetZoneColor touch them, but their count bytes
    // keep the values read at open. Expected packet: the byte-exact Static derivation is covered by
    // static_mode_packet_is_byte_exact; here it is built with the same pure functions and the counts are asserted.
    let mut cfg = original();
    cfg[41] = 30;
    cfg[52] = 12;
    let red = Rgb::new(0xFF, 0, 0);
    let layout = protocol::Layout {
        mixed_mode: true,
        no_onboards: true,
        onboard_leds: 0,
    };
    let mut e = protocol::ConfigPacket(cfg);
    for z in [
        MsiZone::JRgb1,
        MsiZone::JRgb2,
        MsiZone::JRainbow1,
        MsiZone::JRainbow2,
    ] {
        e.apply_mode(&layout, z, 1, 0, 10, false);
        e.apply_zone_color(&layout, z, red, red);
    }
    let e = e.with_save(false);
    assert_eq!((e[41], e[52]), (30, 12));
    assert_eq!((e[31], e[42]), (1, 1));

    let fake = open_steps_with(FakeTransport::builder(), cfg)
        .feature_reply(0x52, current()[1..].to_vec())
        .feature(with_save(current(), 0))
        .feature(e.to_vec())
        .build();
    let mut c = MysticLight185::open(Box::new(fake.clone()), &candidate(0x7D75, None)).expect("opens");
    let state = ModeState {
        mode: 1,
        speed: None,
        brightness: None,
        direction: None,
        colors: vec![red; 44],
    };
    c.set_mode(&state).expect("static");
    fake.finish().expect("script consumed");
}
