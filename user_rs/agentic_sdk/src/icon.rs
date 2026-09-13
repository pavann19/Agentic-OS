//! Minimal, built-in 1-bit monochrome icon assets (Phase 13 / UI enhancements).
//! Each icon is a 16x16 1-bit bitmap (32 bytes total, 2 bytes per row, MSB first).
//! No runtime decompression, zero allocations, embedded directly in .rodata.

use crate::surface;

pub struct Icon16 {
    pub data: [u8; 32],
}

impl Icon16 {
    pub const fn new(data: [u8; 32]) -> Self {
        Self { data }
    }

    /// Draws this icon at `(x, y)` on the given surface.
    /// If `bg == 0`, 0-bits are transparent (skipped).
    pub unsafe fn draw(&self, surface_cap: u32, x: u32, y: u32, fg: u32, bg: u32) -> u64 {
        surface::draw_bitmap(surface_cap, x, y, 16, 16, &self.data, fg, bg)
    }
}

/// Terminal prompt icon (`>_` inside window frame)
pub static TERMINAL_ICON: Icon16 = Icon16::new([
    0b00000000, 0b00000000, // row 0
    0b01111111, 0b11111110, // row 1: top border
    0b01000000, 0b00000010, // row 2
    0b01010000, 0b00000010, // row 3: '>' start
    0b01001000, 0b00000010, // row 4: '>'
    0b01000100, 0b00000010, // row 5: '>' center
    0b01001000, 0b00000010, // row 6: '>'
    0b01010000, 0b00000010, // row 7: '>' end
    0b01000000, 0b01110010, // row 8: '_' cursor
    0b01000000, 0b00000010, // row 9
    0b01000000, 0b00000010, // row 10
    0b01000000, 0b00000010, // row 11
    0b01000000, 0b00000010, // row 12
    0b01000000, 0b00000010, // row 13
    0b01111111, 0b11111110, // row 14: bottom border
    0b00000000, 0b00000000, // row 15
]);

/// Document / Text Editor icon (paper sheet with lines of text and folded ear)
pub static EDITOR_ICON: Icon16 = Icon16::new([
    0b00000000, 0b00000000, // row 0
    0b00111111, 0b11000000, // row 1: top edge
    0b00100000, 0b01100000, // row 2: fold
    0b00100000, 0b01010000, // row 3: fold
    0b00100000, 0b01111000, // row 4: fold bottom
    0b00101111, 0b10001000, // row 5: text line 1
    0b00100000, 0b00001000, // row 6
    0b00101111, 0b11001000, // row 7: text line 2
    0b00100000, 0b00001000, // row 8
    0b00101111, 0b10001000, // row 9: text line 3
    0b00100000, 0b00001000, // row 10
    0b00101110, 0b00001000, // row 11: text line 4
    0b00100000, 0b00001000, // row 12
    0b00100000, 0b00001000, // row 13
    0b00111111, 0b11111000, // row 14: bottom edge
    0b00000000, 0b00000000, // row 15
]);

/// File Manager Folder icon
pub static FOLDER_ICON: Icon16 = Icon16::new([
    0b00000000, 0b00000000, // row 0
    0b00000000, 0b00000000, // row 1
    0b00111100, 0b00000000, // row 2: tab top
    0b01000011, 0b11111000, // row 3: tab & back top
    0b01000000, 0b00000100, // row 4
    0b01111111, 0b11111110, // row 5: front folder top
    0b01000000, 0b00000010, // row 6
    0b01000000, 0b00000010, // row 7
    0b01000000, 0b00000010, // row 8
    0b01000000, 0b00000010, // row 9
    0b01000000, 0b00000010, // row 10
    0b01000000, 0b00000010, // row 11
    0b01000000, 0b00000010, // row 12
    0b01111111, 0b11111110, // row 13: front folder bottom
    0b00000000, 0b00000000, // row 14
    0b00000000, 0b00000000, // row 15
]);

/// Application / System Launcher icon (4-quadrant diamond grid)
pub static APP_ICON: Icon16 = Icon16::new([
    0b00000000, 0b00000000, // row 0
    0b00000001, 0b10000000, // row 1
    0b00000011, 0b11000000, // row 2
    0b00000111, 0b11100000, // row 3
    0b00001111, 0b11110000, // row 4
    0b00011100, 0b00111000, // row 5
    0b00111000, 0b00011100, // row 6
    0b01110000, 0b00001110, // row 7
    0b01110000, 0b00001110, // row 8
    0b00111000, 0b00011100, // row 9
    0b00011100, 0b00111000, // row 10
    0b00001111, 0b11110000, // row 11
    0b00000111, 0b11100000, // row 12
    0b00000011, 0b11000000, // row 13
    0b00000001, 0b10000000, // row 14
    0b00000000, 0b00000000, // row 15
]);
