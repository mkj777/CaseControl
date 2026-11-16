//! Wooting V3 protocol: constants and pure packet builders.
//!
//! Ported from `reference/openrgb/WootingKeyboardController/` (`WootingKeyboardController.cpp`,
//! `WootingV3KeyboardController.cpp`). Feature-command framing follows the V3 ("multi report") branch of Wooting's
//! own RGB SDK (`wooting-usb.c`, `wooting_usb_send_feature_buff`), see [`FEATURE_REPORT_ID`].

use cc_core::Rgb;

/// Wooting vendor id (`WOOTING_NEW_VID`).
pub const VID: u16 = 0x31E3;
/// Wooting 60HE (ARM) base product id (`WOOTING_60HE_ARM_PID`).
pub const PID_60HE_ARM: u16 = 0x1310;
/// The three product ids the 60HE (ARM) enumerates with, one per gamepad-emulation setting
/// (`XINP_PID`, `DINP_PID`, `NONE_PID` in `WootingKeyboardControllerDetect.cpp`).
pub const PIDS_60HE_ARM: [u16; 3] = [PID_60HE_ARM, PID_60HE_ARM | 0x0001, PID_60HE_ARM | 0x0002];
/// Usage page of the V3 configuration interface (`WOOTING_CONFIG_USAGE_PAGE_V3`).
pub const USAGE_PAGE_V3: u16 = 0xFF55;

/// Length of a feature command, report id included (`WOOTING_COMMAND_SIZE`).
pub const COMMAND_SIZE: usize = 8;
/// Report id of V3 feature commands.
///
/// Deviation from the OpenRGB reference: `WootingV3KeyboardController` inherits the V1/V2 framing
/// (`{0x00, 0xD0, 0xDA, ...}`), but the V3 interface uses numbered reports (its colour frame is report 4), and
/// Windows rejects a feature report with id 0 on such an interface, so OpenRGB's init silently fails there.
/// Wooting's RGB SDK sends `{0x01, 0xD1, 0xDA, ...}` when the interface is on usage page 0xFF55.
pub const FEATURE_REPORT_ID: u8 = 0x01;
/// Magic word of V3 packets (`0xD1 0xDA`; V1/V2 used `0xD0 0xDA`).
pub const MAGIC: [u8; 2] = [0xD1, 0xDA];

/// `WOOTING_RAW_COLORS_REPORT`: the command byte of a full colour frame.
pub const RAW_COLORS_REPORT: u8 = 11;
/// `WOOTING_RESET_ALL_COMMAND`: drop software colours and show the active profile's lighting again.
/// Wooting's SDK sends it from `wooting_rgb_reset_rgb()` / `wooting_rgb_close()`. Lighting only, no flash write.
pub const RESET_ALL_COMMAND: u8 = 32;
/// `WOOTING_COLOR_INIT_COMMAND`: sent once after opening, before colour frames (`SendInitialize`).
pub const COLOR_INIT_COMMAND: u8 = 33;

/// Output report id of the V3 colour frame (`rgb_buffer[0] = 4` in `WootingV3KeyboardController::SendDirect`).
pub const FRAME_REPORT_ID: u8 = 4;
/// Length of the V3 colour frame, report id included (`WOOTING_V3_REPORT_SIZE`).
pub const FRAME_SIZE: usize = 2046;
/// Offset of the first colour in the frame (after report id, magic word and command).
pub const FRAME_HEADER_LEN: usize = 4;
/// Rows of the full hardware LED grid all Wooting boards address (`WOOTING_RGB_ROWS`).
pub const HW_ROWS: usize = 6;
/// Columns of the full hardware LED grid (`WOOTING_RGB_COLUMNS`).
pub const HW_COLUMNS: usize = 21;
/// Number of addressable hardware LED slots; a hardware index is `row * HW_COLUMNS + column`.
pub const HW_SLOTS: usize = HW_ROWS * HW_COLUMNS;

/// Encodes a colour as the keyboard's 16-bit R5G6B5 value (`RGB32ToRGBcolor16`).
pub fn rgb565(c: Rgb) -> u16 {
    (u16::from(c.r & 0xF8) << 8) | (u16::from(c.g & 0xFC) << 3) | (u16::from(c.b) >> 3)
}

/// Builds a feature command. `params` are `parameter0..parameter3`; the wire order is reversed
/// (`buf[4] = parameter3 .. buf[7] = parameter0`), as in `wooting_usb_send_feature`.
pub fn feature_command(command: u8, params: [u8; 4]) -> [u8; COMMAND_SIZE] {
    let [p0, p1, p2, p3] = params;
    [FEATURE_REPORT_ID, MAGIC[0], MAGIC[1], command, p3, p2, p1, p0]
}

/// The "colour init" feature command sent before taking control.
pub fn color_init_packet() -> [u8; COMMAND_SIZE] {
    feature_command(COLOR_INIT_COMMAND, [0; 4])
}

/// The "reset all" feature command that hands lighting back to the keyboard's profile.
pub fn reset_all_packet() -> [u8; COMMAND_SIZE] {
    feature_command(RESET_ALL_COMMAND, [0; 4])
}

/// A zeroed colour frame with the header filled in.
pub fn empty_frame() -> Vec<u8> {
    let mut frame = vec![0u8; FRAME_SIZE];
    frame[0] = FRAME_REPORT_ID;
    frame[1..3].copy_from_slice(&MAGIC);
    frame[3] = RAW_COLORS_REPORT;
    frame
}

/// Writes `colors[i]` into the slot of hardware index `hw_index[i]` of `frame` (a buffer from [`empty_frame`]),
/// little-endian R5G6B5. Slots without an LED keep whatever they hold (zero in a fresh frame), exactly like the
/// zero-initialised framebuffer of `RGBController_WootingKeyboard::DeviceUpdateLEDs`.
///
/// # Panics
///
/// If `frame` is shorter than the header plus [`HW_SLOTS`] colours, or a hardware index is `>= HW_SLOTS`.
pub fn fill_frame(frame: &mut [u8], hw_index: &[u8], colors: &[Rgb]) {
    for (&hw, &color) in hw_index.iter().zip(colors) {
        let hw = usize::from(hw);
        assert!(hw < HW_SLOTS, "hardware index {hw} outside the 6x21 grid");
        let at = FRAME_HEADER_LEN + hw * 2;
        frame[at..at + 2].copy_from_slice(&rgb565(color).to_le_bytes());
    }
}

/// Builds a complete colour frame for LEDs at hardware indices `hw_index` showing `colors`.
pub fn frame(hw_index: &[u8], colors: &[Rgb]) -> Vec<u8> {
    let mut f = empty_frame();
    fill_frame(&mut f, hw_index, colors);
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_matches_reference_macro() {
        // RGB32ToRGBcolor16 on OpenRGB's 0x00BBGGRR: (R & 0xF8) << 8 | (G & 0xFC) << 3 | (B & 0xF8) >> 3.
        assert_eq!(rgb565(Rgb::new(0xFF, 0, 0)), 0xF800);
        assert_eq!(rgb565(Rgb::new(0, 0xFF, 0)), 0x07E0);
        assert_eq!(rgb565(Rgb::new(0, 0, 0xFF)), 0x001F);
        assert_eq!(rgb565(Rgb::WHITE), 0xFFFF);
        // 0x12 & 0xF8 = 0x10 -> 0x1000; 0x34 & 0xFC = 0x34 -> 0x01A0; 0x56 >> 3 = 0x0A. Sum 0x11AA.
        assert_eq!(rgb565(Rgb::new(0x12, 0x34, 0x56)), 0x11AA);
        // Low bits are dropped: 0x07 red, 0x03 green, 0x07 blue all vanish.
        assert_eq!(rgb565(Rgb::new(0x07, 0x03, 0x07)), 0x0000);
    }

    #[test]
    fn init_packet_bytes() {
        // wooting_usb_send_feature_buff (multi report): [1, 0xD1, 0xDA, cmd=33, p3, p2, p1, p0], all params 0.
        assert_eq!(
            color_init_packet(),
            [0x01, 0xD1, 0xDA, 0x21, 0x00, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn reset_packet_bytes() {
        // Same framing, cmd = WOOTING_RESET_ALL_COMMAND = 32 = 0x20.
        assert_eq!(
            reset_all_packet(),
            [0x01, 0xD1, 0xDA, 0x20, 0x00, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn feature_params_are_reversed() {
        // feature_buffer[4] = parameter3 ... feature_buffer[7] = parameter0.
        assert_eq!(
            feature_command(30, [0xA0, 0xA1, 0xA2, 0xA3]),
            [0x01, 0xD1, 0xDA, 0x1E, 0xA3, 0xA2, 0xA1, 0xA0]
        );
    }

    #[test]
    fn empty_frame_header() {
        let f = empty_frame();
        assert_eq!(f.len(), 2046);
        // rgb_buffer[0] = 4; [1] = 0xD1; [2] = 0xDA; [3] = WOOTING_RAW_COLORS_REPORT (11).
        assert_eq!(&f[..4], &[0x04, 0xD1, 0xDA, 0x0B]);
        assert!(f[4..].iter().all(|&b| b == 0));
    }

    #[test]
    fn frame_places_colors_little_endian_by_hw_index() {
        // hw 0 -> bytes 4,5; hw 125 (last slot) -> bytes 4 + 250 = 254, 255.
        let f = frame(&[0, 125], &[Rgb::new(0xFF, 0, 0), Rgb::new(0, 0, 0xFF)]);
        assert_eq!(&f[4..6], &[0x00, 0xF8]);
        assert_eq!(&f[254..256], &[0x1F, 0x00]);
        assert!(f[6..254].iter().all(|&b| b == 0));
        assert!(f[256..].iter().all(|&b| b == 0));
    }
}
