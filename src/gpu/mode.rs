//! How DRAW reads its indices.

/// Two to a line.
pub const LINES: u16 = 0;
/// Three to a triangle, its edges.
pub const WIRE: u16 = 1;
/// Three to a triangle, filled and shaded; the side facing away isn't
/// drawn (counter-clockwise faces the viewer).
pub const FACES: u16 = 2;
