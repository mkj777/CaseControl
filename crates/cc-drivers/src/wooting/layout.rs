//! The 60% key layout (Wooting 60HE family), from `matrix_map_60` and `led_names_60HE` in
//! `reference/openrgb/WootingKeyboardController/RGBController_WootingKeyboard.cpp`.
//!
//! Each cell holds the LED's hardware index into the 6x21 grid (`row * 21 + column`) and its name. Names are the
//! string values of OpenRGB's `KEY_EN_*` constants (`RGBControllerKeyNames.cpp`, not copied into `reference/`).

/// Rows of the 60% matrix.
pub const ROWS: usize = 5;
/// Columns of the 60% matrix.
pub const COLUMNS: usize = 14;

type Cell = Option<(u8, &'static str)>;

const fn k(hw: u8, name: &'static str) -> Cell {
    Some((hw, name))
}

/// Row-major 5x14 grid; `None` where `matrix_map_60` has `NA`.
pub const KEYS_60: [[Cell; COLUMNS]; ROWS] = [
    [
        k(21, "Key: Escape"),
        k(22, "Key: 1"),
        k(23, "Key: 2"),
        k(24, "Key: 3"),
        k(25, "Key: 4"),
        k(26, "Key: 5"),
        k(27, "Key: 6"),
        k(28, "Key: 7"),
        k(29, "Key: 8"),
        k(30, "Key: 9"),
        k(31, "Key: 0"),
        k(32, "Key: -"),
        k(33, "Key: ="),
        k(34, "Key: Backspace"),
    ],
    [
        k(42, "Key: Tab"),
        k(43, "Key: Q"),
        k(44, "Key: W"),
        k(45, "Key: E"),
        k(46, "Key: R"),
        k(47, "Key: T"),
        k(48, "Key: Y"),
        k(49, "Key: U"),
        k(50, "Key: I"),
        k(51, "Key: O"),
        k(52, "Key: P"),
        k(53, "Key: ["),
        k(54, "Key: ]"),
        k(55, "Key: \\ (ANSI)"),
    ],
    [
        k(63, "Key: Caps Lock"),
        k(64, "Key: A"),
        k(65, "Key: S"),
        k(66, "Key: D"),
        k(67, "Key: F"),
        k(68, "Key: G"),
        k(69, "Key: H"),
        k(70, "Key: J"),
        k(71, "Key: K"),
        k(72, "Key: L"),
        k(73, "Key: ;"),
        k(74, "Key: '"),
        k(75, "Key: #"),
        k(76, "Key: Enter"),
    ],
    [
        k(84, "Key: Left Shift"),
        k(85, "Key: \\ (ISO)"),
        k(86, "Key: Z"),
        k(87, "Key: X"),
        k(88, "Key: C"),
        k(89, "Key: V"),
        k(90, "Key: B"),
        k(91, "Key: N"),
        k(92, "Key: M"),
        k(93, "Key: ,"),
        k(94, "Key: ."),
        k(95, "Key: /"),
        None,
        k(97, "Key: Right Shift"),
    ],
    [
        k(105, "Key: Left Control"),
        k(106, "Key: Left Windows"),
        k(107, "Key: Left Alt"),
        None,
        k(109, "Spacebar LED 1"),
        k(110, "Spacebar LED 2"),
        k(111, "Key: Space"),
        k(112, "Spacebar LED 3"),
        k(113, "Spacebar LED 4"),
        None,
        k(115, "Key: Right Alt"),
        k(116, "Key: Menu"),
        k(117, "Key: Right Control"),
        k(118, "Key: Right Fn"),
    ],
];
