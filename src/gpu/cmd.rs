//! What a write to CMD does.

/// Starts a new back frame, filled with BG, WIDTH × HEIGHT.
pub const CLEAR: u16 = 0;
/// Draws COUNT indices from FIRST.
pub const DRAW: u16 = 1;
/// Shows the back frame.
pub const SWAP: u16 = 2;
/// A pixel at (X, Y).
pub const PIXEL: u16 = 3;
/// A line from (X, Y) to (X2, Y2).
pub const LINE: u16 = 4;
