//! Tested board list and per-board configuration of the 185-byte class.
//!
//! `DETECTORS` is the reference detector table (`MSIMotherboardControllerDetect.cpp`) without the
//! `ENABLE_UNTESTED_MYSTIC_LIGHT` block. `BOARD_CONFIGS` is `board_configs[]` from `MSIMotherboard185Controller.cpp`,
//! with each entry's comment turned into its display name.

use cc_hid::HidMatch;

use super::protocol::{MsiZone, PID_COMMON, VID_MSI, VID_MSI_COMMON};

/// How a board supports direct (per-LED) control (`MSIMotherboard185Controller::DIRECT_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirectSupport {
    /// No direct mode.
    Disabled,
    /// Per-LED feature reports 0x53 (ported).
    PerLed,
    /// Zone-based direct mode through 0x52 packets (not ported; such boards fail detection).
    ZoneBased,
}

/// One entry of `board_configs[]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardConfig {
    /// Board id (USB PID, or the virtual PID of a common-PID board).
    pub pid: u16,
    /// Board name, from the reference's comment.
    pub name: &'static str,
    /// `numof_onboard_leds`.
    pub onboard_leds: u8,
    /// `numof_pipe1_leds`.
    pub pipe1_leds: u8,
    /// `numof_pipe2_leds`.
    pub pipe2_leds: u8,
    /// `numof_JRGBs`.
    pub jrgbs: u8,
    /// `supported_zones`, in the order the zones are exposed.
    pub zones: &'static [MsiZone],
    /// `per_led_mode`.
    pub direct: DirectSupport,
    /// `minimum_firmware_version` (APROM byte); 0 accepts every version.
    pub minimum_firmware: u8,
}

/// The configuration the reference uses for a 185-byte board that is not in `board_configs[]`.
pub const DEFAULT_BOARD: BoardConfig = BoardConfig {
    pid: 0,
    name: "",
    onboard_leds: 6,
    pipe1_leds: 1,
    pipe2_leds: 1,
    jrgbs: 2,
    zones: ALL_ZONES,
    direct: DirectSupport::Disabled,
    minimum_firmware: 0,
};

use MsiZone::{JCorsair, JPipe1, JPipe2, JRainbow1, JRainbow2, JRainbow3, JRgb1, JRgb2};
const ONBOARD: MsiZone = MsiZone::OnBoard(0);

const ALL_ZONES: &[MsiZone] = &[
    JRgb1, JRgb2, JRainbow1, JRainbow2, JCorsair, JPipe1, JPipe2, ONBOARD,
];
const SET0: &[MsiZone] = &[JRgb1, JRgb2, JRainbow1, JRainbow2, ONBOARD];
const SET1: &[MsiZone] = &[JRgb1, JRainbow1, JRainbow2, JCorsair];
const SET2: &[MsiZone] = &[JRgb1, JRainbow1, JRainbow2, ONBOARD];
const SET3: &[MsiZone] = &[JRgb1, JRainbow1, JRainbow2, JCorsair, JPipe1, ONBOARD];
const SET5: &[MsiZone] = &[JRgb1, JRainbow1];
const SET6: &[MsiZone] = &[JRainbow1];
const SET7: &[MsiZone] = &[JRgb1, JRgb2, JRainbow1, JRainbow2, JPipe1, ONBOARD];
const SET8: &[MsiZone] = &[JRgb1, JRainbow1, JRainbow2, JCorsair, JPipe1, JPipe2];
const SET9: &[MsiZone] = &[JRgb1, JRainbow1, JRainbow2, JCorsair, JPipe1];
const SET10: &[MsiZone] = &[JRgb1, JRainbow1, JRainbow2, JRainbow3];
const SET11: &[MsiZone] = &[JRgb1, JRainbow1, JRainbow2];
const SET12: &[MsiZone] = &[JRgb1, JRainbow1, JRainbow2, JRainbow3, JPipe1, ONBOARD];
const SET13: &[MsiZone] = &[JRgb1, JRgb2, JRainbow1, JRainbow2];
const SET14: &[MsiZone] = &[JRainbow1, JRainbow2];
const SET15: &[MsiZone] = &[JRgb1, JRainbow1, JRainbow2, JRainbow3, JPipe1, JPipe2, ONBOARD];
const SET16: &[MsiZone] = &[JRgb1, JRainbow1, JRainbow2, JRainbow3, JPipe1, JPipe2];
const SET17: &[MsiZone] = &[JRgb1, JRgb2, JRainbow1, JRainbow2, JPipe1];
const SET18: &[MsiZone] = &[JRgb1, JRainbow1, JRainbow2, JRainbow3, JPipe1];
const SET19: &[MsiZone] = &[JRainbow1, JRainbow2, JRainbow3];
const SET20: &[MsiZone] = &[JRgb1];
const SET21: &[MsiZone] = &[
    JRgb1, JRgb2, JRainbow1, JRainbow2, JCorsair, JPipe1, JPipe2, ONBOARD,
];

const fn cfg(
    pid: u16,
    name: &'static str,
    (onboard_leds, pipe1_leds, pipe2_leds, jrgbs): (u8, u8, u8, u8),
    zones: &'static [MsiZone],
    direct: DirectSupport,
    minimum_firmware: u8,
) -> BoardConfig {
    BoardConfig {
        pid,
        name,
        onboard_leds,
        pipe1_leds,
        pipe2_leds,
        jrgbs,
        zones,
        direct,
        minimum_firmware,
    }
}

use DirectSupport::{Disabled, PerLed, ZoneBased};

/// `board_configs[]` of the 185-byte controller, in the reference's order.
pub static BOARD_CONFIGS: &[BoardConfig] = &[
    cfg(
        0x7B93,
        "MPG X570 GAMING PRO CARBON WIFI",
        (6, 1, 0, 1),
        SET3,
        ZoneBased,
        0,
    ),
    cfg(0x7C34, "MEG X570 GODLIKE", (0, 1, 1, 1), SET8, ZoneBased, 0),
    cfg(0x7C35, "MEG X570 ACE", (0, 1, 0, 1), SET9, ZoneBased, 0),
    cfg(0x7C36, "PRESTIGE X570 CREATION", (6, 1, 0, 1), SET3, ZoneBased, 0),
    cfg(0x7C37, "MPG X570 GAMING PLUS", (6, 0, 0, 2), SET0, ZoneBased, 0),
    cfg(0x7C56, "MPG B550 GAMING PLUS", (6, 0, 0, 1), SET2, PerLed, 6),
    cfg(0x7C59, "CREATOR TRX40", (0, 8, 0, 1), SET9, Disabled, 0),
    cfg(0x7C60, "TRX40-A PRO", (6, 0, 0, 1), SET1, PerLed, 0),
    cfg(0x7C67, "MAG B365M MORTAR", (6, 0, 0, 1), SET5, PerLed, 0),
    cfg(0x7C71, "MEG Z490 ACE", (6, 6, 0, 1), SET3, PerLed, 0),
    cfg(
        0x7C73,
        "MPG Z490 GAMING CARBON WIFI",
        (6, 4, 0, 1),
        SET3,
        PerLed,
        0,
    ),
    cfg(0x7C75, "MPG Z490 GAMING PLUS", (6, 0, 0, 1), SET2, PerLed, 0),
    cfg(0x7C76, "MPG Z490M GAMING EDGE", (6, 0, 0, 2), SET0, Disabled, 0),
    cfg(0x7C77, "MEG Z490I UNIFY", (0, 0, 0, 0), SET14, PerLed, 0),
    cfg(0x7C79, "MPG Z490 GAMING EDGE WIFI", (6, 0, 0, 2), SET0, PerLed, 0),
    cfg(0x7C80, "MAG Z490 TOMAHAWK", (6, 0, 0, 2), SET0, PerLed, 0),
    cfg(0x7C02, "MAG B450 TOMAHAWK MAX", (6, 0, 0, 2), SET0, PerLed, 0),
    cfg(0x7C81, "MAG B460 TOMAHAWK", (6, 0, 0, 2), SET0, PerLed, 0),
    cfg(0x7C82, "MAG B460M MORTAR WIFI", (6, 0, 0, 1), SET5, PerLed, 0),
    cfg(0x7C83, "B460M PRO-VDH WIFI", (6, 0, 0, 1), SET5, Disabled, 0),
    cfg(0x7C84, "MAG X570 TOMAHAWK WIFI", (6, 0, 0, 2), SET0, ZoneBased, 0),
    cfg(0x7C86, "MPG B460I GAMING EDGE", (6, 0, 0, 1), SET5, Disabled, 0),
    cfg(0x7C87, "B450M BAZOOKA MAX WIFI", (6, 0, 0, 2), SET13, PerLed, 0),
    cfg(
        0x7C90,
        "MPG B550 GAMING CARBON WIFI",
        (6, 4, 0, 1),
        SET3,
        PerLed,
        0,
    ),
    cfg(0x7C91, "MAG B550 TOMAHAWK", (6, 0, 0, 2), SET0, PerLed, 0),
    cfg(
        0x7C92,
        "MPG B550I GAMING EDGE WIFI",
        (6, 0, 0, 0),
        SET6,
        PerLed,
        0,
    ),
    cfg(0x7C94, "MAG B550M MORTAR", (6, 0, 0, 1), SET11, PerLed, 0),
    cfg(0x7C95, "B550M PRO-VDH WIFI", (6, 0, 0, 2), SET0, PerLed, 0),
    cfg(0x7C98, "Z490 PLUS", (6, 0, 0, 1), SET5, PerLed, 0),
    cfg(0x7D03, "MPG Z590 GODLIKE", (0, 15, 18, 1), SET8, PerLed, 0),
    cfg(0x7D06, "MPG Z590 GAMING FORCE", (4, 4, 0, 1), SET3, PerLed, 0),
    cfg(0x7D07, "MPG Z590 GAMING EDGE WIFI", (4, 5, 0, 2), SET7, PerLed, 0),
    cfg(0x7D08, "MAG Z590 TOMAHAWK", (6, 0, 0, 2), SET0, PerLed, 0),
    cfg(0x7D09, "Z590-A PRO WIFI", (6, 0, 0, 2), SET11, PerLed, 0),
    cfg(0x7D13, "MEG B550 UNIFY", (6, 0, 0, 1), SET1, PerLed, 0),
    cfg(0x7D14, "A520M PRO", (6, 0, 0, 1), SET5, PerLed, 0),
    cfg(0x7D15, "MAG B560 TOMAHAWK WIFI", (6, 0, 0, 2), SET0, PerLed, 0),
    cfg(0x7D17, "MAG B560M MORTAR", (6, 0, 0, 2), SET11, PerLed, 0),
    cfg(0x7D18, "MAG B560M PRO-VDH", (6, 0, 0, 2), SET5, PerLed, 0),
    cfg(
        0x7D19,
        "MPG B560I GAMING EDGE WIFI",
        (6, 0, 0, 2),
        SET5,
        PerLed,
        0,
    ),
    cfg(0x7D20, "MAG B560M PRO", (6, 0, 0, 2), SET5, PerLed, 0),
    cfg(0x7D25, "PRO Z690-A WIFI DDR4", (6, 0, 0, 2), SET11, PerLed, 0),
    cfg(0x7D27, "MEG Z690 ACE", (6, 0, 0, 2), SET1, PerLed, 0),
    cfg(0x7D28, "MEG Z690 UNIFY-X", (6, 0, 0, 1), SET1, PerLed, 0),
    cfg(0x7D29, "MEG Z690I UNIFY", (6, 0, 0, 0), SET6, PerLed, 0),
    cfg(0x7D30, "MPG Z690 CARBON WIFI", (6, 6, 0, 2), SET3, PerLed, 0),
    cfg(0x7D31, "MPG EDGE WIFI DDR4", (4, 8, 0, 2), SET12, PerLed, 0),
    cfg(
        0x7D32,
        "MAG Z690 TOMAHAWK WIFI DDR4",
        (1, 0, 0, 1),
        SET10,
        PerLed,
        0,
    ),
    cfg(0x7D33, "PRO Z790-VC WIFI", (6, 0, 0, 2), SET5, PerLed, 0),
    cfg(0x7D36, "PRO Z690-P DDR4", (6, 0, 0, 2), SET5, PerLed, 0),
    cfg(0x7D37, "PRO B760M-VC WIFI", (6, 0, 0, 2), SET11, PerLed, 0),
    cfg(0x7D38, "MEG Z590 UNIFY-X", (0, 0, 0, 1), SET1, PerLed, 0),
    cfg(0x7D40, "MPG B760I EDGE WIFI DDR4", (0, 0, 0, 1), SET5, PerLed, 0),
    cfg(
        0x7D41,
        "MAG B660M TOMAHAWK WIFI DDR4",
        (6, 0, 0, 2),
        SET13,
        PerLed,
        0,
    ),
    cfg(
        0x7D42,
        "MAG B660 MORTAR WIFI DDR4",
        (6, 0, 0, 2),
        SET11,
        PerLed,
        0,
    ),
    cfg(0x7D43, "PRO B660M-A WIFI DDR4", (0, 0, 0, 2), SET11, PerLed, 0),
    cfg(0x7D46, "PRO H610M-G DDR4", (0, 1, 1, 0), SET11, PerLed, 0),
    cfg(0x7D50, "MEG X570S ACE MAX", (6, 12, 0, 1), SET3, PerLed, 0),
    cfg(0x7D51, "MEG X570S UNIFY-X MAX", (6, 0, 0, 2), SET1, PerLed, 0),
    cfg(0x7D52, "MPG X570S CARBON EK X", (6, 14, 0, 1), SET3, PerLed, 0),
    cfg(0x7D53, "MPG X570S EDGE MAX WIFI", (6, 0, 0, 2), SET0, PerLed, 0),
    cfg(
        0x7D54,
        "MAG X570S TOMAHAWK MAX WIFI",
        (6, 0, 0, 2),
        SET0,
        PerLed,
        0,
    ),
    cfg(0x7D59, "PRO B660-A DDR4", (0, 0, 0, 2), SET11, PerLed, 0),
    cfg(0x7D67, "PRO X670-P WIFI", (0, 0, 0, 1), SET11, PerLed, 0),
    cfg(0x7D69, "MEG X670E ACE", (9, 2, 4, 1), SET15, PerLed, 0),
    cfg(0x7D70, "MPG X670E CARBON WIFI", (0, 6, 0, 1), SET9, PerLed, 0),
    cfg(0x7D73, "MPG B650I EDGE WIFI", (1, 0, 0, 0), SET6, PerLed, 0),
    cfg(0x7D74, "MPG B650 CARBON WIFI", (0, 6, 0, 1), SET18, PerLed, 0),
    cfg(0x7D75, "MAG B650 TOMAHAWK WIFI", (0, 0, 0, 2), SET13, PerLed, 0),
    cfg(0x7D76, "MAG B650M MORTAR WIFI", (0, 0, 0, 2), SET13, PerLed, 0),
    cfg(0x7D77, "PRO B650M-A WIFI", (0, 0, 0, 2), SET13, PerLed, 0),
    cfg(0x7D78, "PRO B650-P WIFI", (0, 0, 0, 2), SET13, PerLed, 0),
    cfg(0x7D86, "MEG Z790 ACE", (0, 18, 4, 1), SET16, PerLed, 0),
    cfg(0x7D88, "MPG Z790-S WIFI", (0, 0, 0, 1), SET20, PerLed, 0),
    cfg(0x7D89, "MPG Z790 CARBON WIFI", (0, 6, 0, 1), SET18, PerLed, 0),
    cfg(0x7D90, "B760M BOMBER DDR4", (0, 0, 0, 1), SET11, PerLed, 0),
    cfg(0x7D91, "MAG Z790 TOMAHAWK WIFI", (1, 0, 0, 1), SET10, PerLed, 0),
    cfg(0x7D93, "Z790 GAMING PRO WIFI", (6, 0, 0, 1), SET2, PerLed, 0),
    cfg(
        0x7D96,
        "MAG B760 TOMAHAWK WIFI DDR5",
        (0, 0, 0, 2),
        SET13,
        PerLed,
        0,
    ),
    cfg(
        0x7D97,
        "MAG B660 MORTAR MAX WIFI DDR4",
        (6, 0, 0, 2),
        SET11,
        PerLed,
        0,
    ),
    cfg(0x7D98, "PRO B760-P WIFI DDR4", (0, 0, 0, 1), SET11, PerLed, 0),
    cfg(0x7D99, "PRO B760M-A WIFI DDR4", (6, 0, 0, 1), SET11, PerLed, 0),
    cfg(0x7E01, "MAG B760M MORTAR MAX", (0, 0, 0, 1), SET11, PerLed, 0),
    cfg(0x7E03, "MPG Z790I EDGE WIFI", (6, 0, 0, 0), SET6, PerLed, 0),
    cfg(0x7E06, "PRO Z790-P WIFI DDR4", (0, 0, 0, 2), SET11, PerLed, 0),
    cfg(0x7E07, "PRO Z790-A WIFI DDR4", (0, 0, 0, 2), SET10, PerLed, 0),
    cfg(0x7E09, "B650M PROJECT ZERO", (0, 0, 0, 0), SET19, PerLed, 0),
    cfg(0x7E10, "MPG B650 EDGE WIFI", (0, 6, 0, 2), SET17, PerLed, 0),
    cfg(0x7E12, "MAG X670E TOMAHAWK WIFI", (0, 0, 0, 2), SET13, PerLed, 0),
    cfg(
        PID_COMMON,
        "MAG X670E TOMAHAWK WIFI",
        (0, 0, 0, 2),
        SET13,
        PerLed,
        0,
    ),
    cfg(0x7E16, "X670E GAMING PLUS WIFI", (0, 0, 0, 2), SET13, PerLed, 0),
    cfg(0x7E24, "B650M GAMING PLUS WIFI", (0, 0, 0, 2), SET13, PerLed, 0),
    cfg(0x7E26, "B650 GAMING PLUS WIFI", (0, 0, 0, 2), SET13, PerLed, 0),
    cfg(0x7E27, "PRO B650M-P", (0, 0, 0, 2), SET13, PerLed, 0),
    cfg(0x7E28, "PRO A620M-B", (6, 0, 0, 2), SET21, ZoneBased, 0),
    cfg(0x7E30, "B650M GAMING WIFI", (0, 0, 0, 2), SET13, PerLed, 0),
];

/// Looks a board id up in `board_configs[]`.
pub fn board_config(pid: u16) -> Option<&'static BoardConfig> {
    BOARD_CONFIGS.iter().find(|c| c.pid == pid)
}

/// The firmware gate of `MSIMotherboard185Controller` (`GetFirmwareVersionInvalid`): a board listed in
/// `board_configs[]` whose APROM version byte is below its `minimum_firmware_version` is invalid. Boards that are
/// not listed are never invalid.
pub fn firmware_version_invalid(aprom: u8, config: Option<&BoardConfig>) -> bool {
    config.is_some_and(|c| aprom < c.minimum_firmware)
}

const fn pu(pid: u16) -> HidMatch {
    HidMatch::new_pu(VID_MSI, pid, 0x0001, 0x00)
}

/// Every interface the tested detector table matches (`REGISTER_HID_DETECTOR_PU`, usage page 0x0001, usage 0x00),
/// without the `ENABLE_UNTESTED_MYSTIC_LIGHT` entries. The common-PID detector on usage page 0xFF00 ("X870") is left
/// out: it only reaches the 761-byte class, which is not ported, and would match a second interface of the same
/// board.
pub static DETECTORS: &[HidMatch] = &[
    pu(0x1720),
    pu(0x7B12),
    pu(0x7B16),
    pu(0x7B17),
    pu(0x7B18),
    pu(0x7B50),
    pu(0x7B85),
    pu(0x7B92),
    pu(0x7B93),
    pu(0x7C02),
    pu(0x7C34),
    pu(0x7C35),
    pu(0x7C36),
    pu(0x7C37),
    pu(0x7C56),
    pu(0x7C59),
    pu(0x7C60),
    pu(0x7C67),
    pu(0x7C71),
    pu(0x7C73),
    pu(0x7C75),
    pu(0x7C76),
    pu(0x7C77),
    pu(0x7C79),
    pu(0x7C80),
    pu(0x7C81),
    pu(0x7C82),
    pu(0x7C83),
    pu(0x7C84),
    pu(0x7C86),
    pu(0x7C87),
    pu(0x7C90),
    pu(0x7C91),
    pu(0x7C92),
    pu(0x7C94),
    pu(0x7C95),
    pu(0x7C98),
    pu(0x7D03),
    pu(0x7D04),
    pu(0x7D06),
    pu(0x7D07),
    pu(0x7D08),
    pu(0x7D09),
    pu(0x7D13),
    pu(0x7D14),
    pu(0x7D15),
    pu(0x7D17),
    pu(0x7D18),
    pu(0x7D19),
    pu(0x7D20),
    pu(0x7D25),
    pu(0x7D27),
    pu(0x7D28),
    pu(0x7D29),
    pu(0x7D30),
    pu(0x7D31),
    pu(0x7D32),
    pu(0x7D33),
    pu(0x7D36),
    pu(0x7D37),
    pu(0x7D38),
    pu(0x7D40),
    pu(0x7D41),
    pu(0x7D42),
    pu(0x7D43),
    pu(0x7D46),
    pu(0x7D50),
    pu(0x7D51),
    pu(0x7D52),
    pu(0x7D53),
    pu(0x7D54),
    pu(0x7D59),
    pu(0x7D67),
    pu(0x7D69),
    pu(0x7D70),
    pu(0x7D73),
    pu(0x7D74),
    pu(0x7D75),
    pu(0x7D76),
    pu(0x7D77),
    pu(0x7D78),
    pu(0x7D86),
    pu(0x7D88),
    pu(0x7D89),
    pu(0x7D90),
    pu(0x7D91),
    pu(0x7D93),
    pu(0x7D96),
    pu(0x7D97),
    pu(0x7D98),
    pu(0x7D99),
    pu(0x7E01),
    pu(0x7E03),
    pu(0x7E06),
    pu(0x7E07),
    pu(0x7E09),
    pu(0x7E10),
    pu(0x7E20),
    pu(0x7E30),
    pu(0x7E32),
    pu(0x7E34),
    pu(0x7E59),
    pu(0x7E66),
    pu(0x7E70),
    pu(0x7E76),
    pu(0x7E80),
    pu(0x7E81),
    pu(0x7E86),
    pu(0xB926),
    HidMatch::new_pu(VID_MSI_COMMON, PID_COMMON, 0x0001, 0x00),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn target_board_is_detected_and_configured() {
        assert!(DETECTORS.contains(&HidMatch::new_pu(0x1462, 0x7D75, 0x0001, 0x00)));
        let c = board_config(0x7D75).expect("7D75 configured");
        assert_eq!(c.name, "MAG B650 TOMAHAWK WIFI");
        assert_eq!(
            (c.onboard_leds, c.pipe1_leds, c.pipe2_leds, c.jrgbs),
            (0, 0, 0, 2)
        );
        assert_eq!(c.zones, &[JRgb1, JRgb2, JRainbow1, JRainbow2]);
        assert_eq!(c.direct, DirectSupport::PerLed);
        assert_eq!(c.minimum_firmware, 0);
    }

    #[test]
    fn untested_pids_are_not_detected() {
        // The ENABLE_UNTESTED_MYSTIC_LIGHT block of the reference.
        for pid in [
            0x3EA4, 0x4459, 0x7B10, 0x7B94, 0x7B96, 0x7C42, 0x7C70, 0x7C85, 0x7C88, 0x7C89, 0x7C96, 0x7C99,
            0x905D,
        ] {
            assert!(DETECTORS.iter().all(|m| m.pid != Some(pid)), "{pid:04x}");
        }
    }

    #[test]
    fn tables_have_no_duplicates() {
        let d: HashSet<_> = DETECTORS.iter().collect();
        assert_eq!(d.len(), DETECTORS.len());
        assert_eq!(DETECTORS.len(), 110);
        let c: HashSet<_> = BOARD_CONFIGS.iter().map(|c| c.pid).collect();
        assert_eq!(c.len(), BOARD_CONFIGS.len());
        assert_eq!(BOARD_CONFIGS.len(), 96);
        assert!(
            DETECTORS
                .iter()
                .all(|m| m.usage_page == Some(1) && m.usage == Some(0))
        );
    }

    #[test]
    fn firmware_gate() {
        // `if(version_APROM_val < board_config->minimum_firmware_version) version_invalid = true;`
        let b550 = board_config(0x7C56);
        assert!(firmware_version_invalid(0x05, b550));
        assert!(!firmware_version_invalid(0x06, b550));
        assert!(!firmware_version_invalid(0x00, board_config(0x7D75)));
        assert!(!firmware_version_invalid(0x00, None));
    }
}
