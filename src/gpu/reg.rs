//! The registers.

/// Writing runs a command: [`super::cmd`].
pub const CMD: u8 = 0;
pub const COLOR: u8 = 1;
/// The color CLEAR fills with.
pub const BG: u8 = 2;
pub const X: u8 = 3;
pub const Y: u8 = 4;
pub const X2: u8 = 5;
pub const Y2: u8 = 6;
/// Points the vertex buffer's writes at a vertex (and its x).
pub const VPTR: u8 = 7;
/// The next coordinate: x, y, z, then the next vertex.
pub const VDATA: u8 = 8;
/// Points the index buffer's writes at an index.
pub const IPTR: u8 = 9;
/// The next index.
pub const IDATA: u8 = 10;
/// What DRAW draws: [`super::mode`].
pub const MODE: u8 = 11;
/// The first index DRAW reads, and how many.
pub const FIRST: u8 = 12;
pub const COUNT: u8 = 13;
/// Turns about y, x and z.
pub const YAW: u8 = 14;
pub const PITCH: u8 = 15;
pub const ROLL: u8 = 16;
/// Pixels per 16 units.
pub const SCALE: u8 = 17;
/// What the turns gain each tick.
pub const SPIN_YAW: u8 = 18;
pub const SPIN_PITCH: u8 = 19;
pub const SPIN_ROLL: u8 = 20;
/// The screen's size in pixels (1–1024), from the next CLEAR on.
pub const WIDTH: u8 = 21;
pub const HEIGHT: u8 = 22;
pub const COUNT_OF: usize = 23;
/// The high byte of the next write.
pub const HIGH: u8 = 255;

/// Whether a byte written without a high byte is sign-extended.
pub fn signed(r: u8) -> bool {
    matches!(r, VDATA | SPIN_YAW | SPIN_PITCH | SPIN_ROLL)
}
