//! The Display's GPU: a screen of up to 1024 × 1024 pixels behind 16-bit
//! registers written over a digital bus a byte at a time (`display.rs` reads
//! the bus; this is what the writes do).
//!
//! A write is `REG ← DATA`, a byte. A register's high byte is what was
//! written to [`reg::HIGH`] just before; without one the byte stands for a
//! small number — sign-extended for the signed registers (coordinates,
//! spins), so an 8-bit machine writes -100 in one go.
//!
//! A program fills a **vertex buffer** (x, y, z — signed 16-bit) and an
//! **index buffer** (16-bit, each a vertex), sets the transform, and draws
//! a range of indices as lines, wire triangles or flat-shaded faces. Draws
//! go into the back frame; SWAP shows it. Spin registers keep the shown
//! frame turning on their own — every tick, as long as nothing replaces it.
//!
//! Colors are RGB332 bytes (`rrrgggbb`). Turns are in 256ths of a turn.

use serde::{Deserialize, Serialize};

/// The widest and tallest a screen can be, in pixels.
pub const MAX_SIZE: u16 = 1024;
/// A new screen's width and height.
pub const DEFAULT_SIZE: u16 = 256;
/// Vertices and indices the buffers hold.
pub const BUFFER: usize = 4096;
/// Draw calls a frame keeps (a program that never clears stops adding).
const MAX_OPS: usize = 1024;

/// The registers.
pub mod reg {
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
}

/// What a write to CMD does.
pub mod cmd {
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
}

/// How DRAW reads its indices.
pub mod mode {
    /// Two to a line.
    pub const LINES: u16 = 0;
    /// Three to a triangle, its edges.
    pub const WIRE: u16 = 1;
    /// Three to a triangle, filled and shaded; the side facing away isn't
    /// drawn (counter-clockwise faces the viewer).
    pub const FACES: u16 = 2;
}

/// A mesh drawn with a transform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draw {
    pub mode: u16,
    pub color: u8,
    /// Yaw, pitch, roll when drawn; then the spin they gain each tick.
    pub turn: [u16; 3],
    pub spin: [i16; 3],
    pub scale: u16,
    pub vertices: Vec<[i16; 3]>,
    pub indices: Vec<u16>,
}

/// One thing drawn into a frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    Clear { color: u8, width: u16, height: u16 },
    Pixel { x: u16, y: u16, color: u8 },
    Line { from: (u16, u16), to: (u16, u16), color: u8 },
    Draw(Draw),
}

/// The GPU's state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Gpu {
    pub regs: Vec<u16>,
    /// A high byte waiting for the next write.
    high: Option<u8>,
    pub vertices: Vec<[i16; 3]>,
    pub indices: Vec<u16>,
    vptr: u16,
    /// The coordinate VDATA writes next (0 x, 1 y, 2 z).
    component: u8,
    iptr: u16,
    back: Vec<Op>,
    /// What's shown, and a number that changes when it does.
    pub front: Vec<Op>,
    pub version: u64,
}

fn blank() -> Vec<Op> {
    vec![Op::Clear { color: 0, width: DEFAULT_SIZE, height: DEFAULT_SIZE }]
}

impl Default for Gpu {
    fn default() -> Self {
        let mut regs = vec![0; reg::COUNT_OF];
        regs[reg::COLOR as usize] = 0xFF;
        regs[reg::SCALE as usize] = 32;
        regs[reg::WIDTH as usize] = DEFAULT_SIZE;
        regs[reg::HEIGHT as usize] = DEFAULT_SIZE;
        Gpu {
            regs,
            high: None,
            vertices: vec![[0; 3]; BUFFER],
            indices: vec![0; BUFFER],
            vptr: 0,
            component: 0,
            iptr: 0,
            back: blank(),
            front: blank(),
            version: 0,
        }
    }
}

impl Gpu {
    fn r(&self, r: u8) -> u16 {
        self.regs[r as usize]
    }

    /// `REG ← DATA`: a byte off the bus.
    pub fn write(&mut self, r: u8, data: u8) {
        if r == reg::HIGH {
            self.high = Some(data);
            return;
        }
        let value = match self.high.take() {
            Some(h) => (h as u16) << 8 | data as u16,
            None if reg::signed(r) => data as i8 as i16 as u16,
            None => data as u16,
        };
        self.set(r, value);
    }

    /// A register gets a 16-bit value.
    pub fn set(&mut self, r: u8, value: u16) {
        if (r as usize) < reg::COUNT_OF {
            self.regs[r as usize] = value;
        }
        match r {
            reg::CMD => self.command(value),
            reg::VPTR => {
                self.vptr = value % BUFFER as u16;
                self.component = 0;
            }
            reg::VDATA => {
                self.vertices[self.vptr as usize][self.component as usize] = value as i16;
                self.component += 1;
                if self.component == 3 {
                    self.component = 0;
                    self.vptr = (self.vptr + 1) % BUFFER as u16;
                }
            }
            reg::IPTR => self.iptr = value % BUFFER as u16,
            reg::IDATA => {
                self.indices[self.iptr as usize] = value % BUFFER as u16;
                self.iptr = (self.iptr + 1) % BUFFER as u16;
            }
            _ => {}
        }
    }

    fn command(&mut self, c: u16) {
        let op = match c {
            cmd::CLEAR => {
                self.back.clear();
                let size = |r| self.r(r).clamp(1, MAX_SIZE);
                Op::Clear { color: self.r(reg::BG) as u8, width: size(reg::WIDTH), height: size(reg::HEIGHT) }
            }
            cmd::DRAW => {
                let first = self.r(reg::FIRST) as usize;
                let count = (self.r(reg::COUNT) as usize).min(BUFFER);
                let indices: Vec<u16> = (first..first + count).map(|i| self.indices[i % BUFFER]).collect();
                // Only the vertices it uses, renumbered.
                let mut used = indices.clone();
                used.sort_unstable();
                used.dedup();
                Op::Draw(Draw {
                    mode: self.r(reg::MODE),
                    color: self.r(reg::COLOR) as u8,
                    turn: [self.r(reg::YAW), self.r(reg::PITCH), self.r(reg::ROLL)],
                    spin: [self.r(reg::SPIN_YAW) as i16, self.r(reg::SPIN_PITCH) as i16, self.r(reg::SPIN_ROLL) as i16],
                    scale: self.r(reg::SCALE),
                    vertices: used.iter().map(|&v| self.vertices[v as usize]).collect(),
                    indices: indices.iter().map(|i| used.binary_search(i).unwrap() as u16).collect(),
                })
            }
            cmd::SWAP => {
                self.front = self.back.clone();
                self.version += 1;
                return;
            }
            cmd::PIXEL => Op::Pixel { x: self.r(reg::X), y: self.r(reg::Y), color: self.r(reg::COLOR) as u8 },
            cmd::LINE => Op::Line {
                from: (self.r(reg::X), self.r(reg::Y)),
                to: (self.r(reg::X2), self.r(reg::Y2)),
                color: self.r(reg::COLOR) as u8,
            },
            _ => return,
        };
        if self.back.len() < MAX_OPS {
            self.back.push(op);
        }
    }

    /// A tick: the turns gain their spin, so the next draw goes on from
    /// where the shown one has turned to.
    pub fn tick(&mut self) {
        for (turn, spin) in [(reg::YAW, reg::SPIN_YAW), (reg::PITCH, reg::SPIN_PITCH), (reg::ROLL, reg::SPIN_ROLL)] {
            self.regs[turn as usize] = self.r(turn).wrapping_add(self.r(spin));
        }
    }
}

/// An RGB332 color as RGBA bytes, `light` (0–1) times as bright.
fn rgba(c: u8, light: f32) -> [u8; 4] {
    let r = (c >> 5) as f32 * 255.0 / 7.0;
    let g = ((c >> 2) & 7) as f32 * 255.0 / 7.0;
    let b = (c & 3) as f32 * 255.0 / 3.0;
    [(r * light) as u8, (g * light) as u8, (b * light) as u8, 255]
}

/// A screen being drawn: colors and depths.
struct Raster {
    w: i32,
    h: i32,
    px: Vec<[u8; 4]>,
    depth: Vec<f32>,
}

impl Raster {
    fn new(w: u16, h: u16, c: u8) -> Self {
        let n = w as usize * h as usize;
        Raster { w: w as i32, h: h as i32, px: vec![rgba(c, 1.0); n], depth: vec![f32::MIN; n] }
    }

    fn put(&mut self, x: i32, y: i32, c: [u8; 4]) {
        if (0..self.w).contains(&x) && (0..self.h).contains(&y) {
            self.px[(y * self.w + x) as usize] = c;
        }
    }

    fn line(&mut self, (mut x0, mut y0): (i32, i32), (x1, y1): (i32, i32), c: [u8; 4]) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        // Points far off the screen aren't walked to the end.
        let limit = 2 * (self.w + self.h) + dx - dy;
        for _ in 0..limit.min(1 << 17) {
            self.put(x0, y0, c);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// A filled triangle of screen points with depths (bigger is nearer).
    fn triangle(&mut self, p: [[f32; 3]; 3], c: [u8; 4]) {
        let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
        let area = edge(p[0], p[1], p[2][0], p[2][1]);
        if area.abs() < 1e-6 {
            return;
        }
        let lo = |i: usize| p.iter().map(|q| q[i]).fold(f32::MAX, f32::min).floor().max(0.0) as i32;
        let hi = |i: usize, max: i32| p.iter().map(|q| q[i]).fold(f32::MIN, f32::max).ceil().min(max as f32 - 1.0) as i32;
        for y in lo(1)..=hi(1, self.h) {
            for x in lo(0)..=hi(0, self.w) {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let w = [edge(p[1], p[2], fx, fy) / area, edge(p[2], p[0], fx, fy) / area, edge(p[0], p[1], fx, fy) / area];
                if w.iter().all(|&w| w >= 0.0) {
                    let z = w[0] * p[0][2] + w[1] * p[1][2] + w[2] * p[2][2];
                    let i = (y * self.w + x) as usize;
                    if z >= self.depth[i] {
                        self.depth[i] = z;
                        self.px[i] = c;
                    }
                }
            }
        }
    }
}

/// A draw's vertices turned (`turn` in 256ths of a turn) and scaled: x
/// right, y up, z toward the viewer.
fn transform(d: &Draw, turn: [f32; 3]) -> Vec<[f32; 3]> {
    let a = |t: f32| t * std::f32::consts::TAU / 256.0;
    let (sy, cy) = a(turn[0]).sin_cos();
    let (sp, cp) = a(turn[1]).sin_cos();
    let (sr, cr) = a(turn[2]).sin_cos();
    let s = d.scale as f32 / 16.0;
    d.vertices
        .iter()
        .map(|v| {
            let (x, y, z) = (v[0] as f32 * s, v[1] as f32 * s, v[2] as f32 * s);
            // Yaw about y, then pitch about x, then roll about z.
            let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
            let (y, z) = (y * cp - z * sp, y * sp + z * cp);
            let (x, y) = (x * cr - y * sr, x * sr + y * cr);
            [x, y, z]
        })
        .collect()
}

/// A frame as an image `ticks` after it was shown: its width, height and
/// RGBA bytes, rows from the top.
pub fn render(ops: &[Op], ticks: f32) -> (u16, u16, Vec<u8>) {
    let (w, h) = ops
        .iter()
        .find_map(|op| match op {
            Op::Clear { width, height, .. } => Some((*width, *height)),
            _ => None,
        })
        .unwrap_or((DEFAULT_SIZE, DEFAULT_SIZE));
    let (w, h) = (w.clamp(1, MAX_SIZE), h.clamp(1, MAX_SIZE));
    let mut r = Raster::new(w, h, 0);
    // The eye stands back as far for any size: a picture looks the same.
    let eye = 600.0 * w.max(h) as f32 / DEFAULT_SIZE as f32;
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let project = |v: [f32; 3]| {
        let f = eye / (eye - v[2]).max(1.0);
        [cx + v[0] * f, cy - v[1] * f, v[2]]
    };
    for op in ops {
        match op {
            Op::Clear { color, .. } => {
                r.px.fill(rgba(*color, 1.0));
                r.depth.fill(f32::MIN);
            }
            Op::Pixel { x, y, color } => r.put(*x as i32, *y as i32, rgba(*color, 1.0)),
            Op::Line { from, to, color } => r.line((from.0 as i32, from.1 as i32), (to.0 as i32, to.1 as i32), rgba(*color, 1.0)),
            Op::Draw(d) => {
                let turn = [0, 1, 2].map(|i| d.turn[i] as f32 + d.spin[i] as f32 * ticks);
                let view = transform(d, turn);
                let screen: Vec<[f32; 3]> = view.iter().map(|&v| project(v)).collect();
                let at = |i: u16| screen.get(i as usize).copied();
                let xy = |p: [f32; 3]| (p[0].round() as i32, p[1].round() as i32);
                let color = rgba(d.color, 1.0);
                match d.mode {
                    mode::LINES => {
                        for pair in d.indices.chunks_exact(2) {
                            if let (Some(a), Some(b)) = (at(pair[0]), at(pair[1])) {
                                r.line(xy(a), xy(b), color);
                            }
                        }
                    }
                    mode::WIRE => {
                        for t in d.indices.chunks_exact(3) {
                            if let (Some(a), Some(b), Some(c)) = (at(t[0]), at(t[1]), at(t[2])) {
                                r.line(xy(a), xy(b), color);
                                r.line(xy(b), xy(c), color);
                                r.line(xy(c), xy(a), color);
                            }
                        }
                    }
                    mode::FACES => {
                        let light = {
                            let l = [-0.4f32, 0.6, 0.7];
                            let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
                            l.map(|v| v / n)
                        };
                        for t in d.indices.chunks_exact(3) {
                            let (Some(a), Some(b), Some(c)) = (at(t[0]), at(t[1]), at(t[2])) else { continue };
                            // On screen y grows down: counter-clockwise faces us when this is negative.
                            let facing = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                            if facing >= 0.0 {
                                continue;
                            }
                            let (va, vb, vc) = (view[t[0] as usize], view[t[1] as usize], view[t[2] as usize]);
                            let (u, v) = ([0, 1, 2].map(|i| vb[i] - va[i]), [0, 1, 2].map(|i| vc[i] - va[i]));
                            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
                            let lit = (n[0] * light[0] + n[1] * light[1] + n[2] * light[2]) / len;
                            r.triangle([a, b, c], rgba(d.color, 0.3 + 0.7 * lit.max(0.0)));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    (w, h, r.px.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &(u16, u16, Vec<u8>), x: usize, y: usize) -> [u8; 4] {
        let i = (y * img.0 as usize + x) * 4;
        [img.2[i], img.2[i + 1], img.2[i + 2], img.2[i + 3]]
    }

    fn cube(g: &mut Gpu, half: i8) {
        g.write(reg::VPTR, 0);
        for v in [[-1, -1, -1], [1, -1, -1], [1, 1, -1], [-1, 1, -1], [-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1]] {
            for c in v {
                g.write(reg::VDATA, (c * half) as u8);
            }
        }
        g.write(reg::IPTR, 0);
        // Counter-clockwise seen from outside.
        let faces = [[4, 5, 6], [4, 6, 7], [1, 0, 3], [1, 3, 2], [5, 1, 2], [5, 2, 6], [0, 4, 7], [0, 7, 3], [7, 6, 2], [7, 2, 3], [0, 1, 5], [0, 5, 4]];
        for f in faces {
            for i in f {
                g.write(reg::IDATA, i);
            }
        }
        g.write(reg::COUNT, 36);
    }

    fn frame(g: &mut Gpu) {
        g.write(reg::CMD, cmd::CLEAR as u8);
        g.write(reg::CMD, cmd::DRAW as u8);
        g.write(reg::CMD, cmd::SWAP as u8);
    }

    #[test]
    fn a_pixel_and_a_line_land_where_asked() {
        let mut g = Gpu::default();
        g.write(reg::BG, 0b000_000_01);
        g.write(reg::CMD, cmd::CLEAR as u8);
        g.write(reg::COLOR, 0b111_000_00);
        g.write(reg::X, 10);
        g.write(reg::Y, 20);
        g.write(reg::CMD, cmd::PIXEL as u8);
        g.write(reg::X2, 50);
        g.write(reg::Y2, 20);
        g.write(reg::CMD, cmd::LINE as u8);
        // Not shown till SWAP.
        assert_eq!(g.front, blank());
        g.write(reg::CMD, cmd::SWAP as u8);
        let img = render(&g.front, 0.0);
        assert_eq!(px(&img, 10, 20), [255, 0, 0, 255]);
        assert_eq!(px(&img, 30, 20), [255, 0, 0, 255]);
        assert_eq!(px(&img, 30, 21), [0, 0, 85, 255]);
    }

    #[test]
    fn sixteen_bits_reach_a_big_screen() {
        let mut g = Gpu::default();
        for (r, v) in [(reg::WIDTH, 1024u16), (reg::HEIGHT, 600), (reg::X, 1000), (reg::Y, 590)] {
            g.write(reg::HIGH, (v >> 8) as u8);
            g.write(r, v as u8);
        }
        g.write(reg::CMD, cmd::CLEAR as u8);
        g.write(reg::CMD, cmd::PIXEL as u8);
        g.write(reg::CMD, cmd::SWAP as u8);
        let img = render(&g.front, 0.0);
        assert_eq!((img.0, img.1, img.2.len()), (1024, 600, 1024 * 600 * 4));
        assert_eq!(px(&img, 1000, 590), [255, 255, 255, 255]);
        // A byte alone: small and, for a coordinate, signed.
        g.write(reg::VPTR, 0);
        g.write(reg::VDATA, -100i8 as u8);
        g.write(reg::HIGH, 0x12);
        g.write(reg::VDATA, 0x34);
        assert_eq!(&g.vertices[0][..2], &[-100, 0x1234]);
        assert_eq!(g.regs[reg::X as usize], 1000);
    }

    #[test]
    fn a_cube_of_faces_covers_the_middle_and_hides_its_back() {
        let mut g = Gpu::default();
        cube(&mut g, 40);
        g.write(reg::MODE, mode::FACES as u8);
        g.write(reg::COLOR, 0b111_111_11);
        frame(&mut g);
        let Op::Draw(d) = &g.front[1] else { panic!() };
        assert_eq!((d.vertices.len(), d.indices.len()), (8, 36));
        let img = render(&g.front, 0.0);
        // Face on: the front face, lit, fills the middle; the corners stay clear.
        let mid = px(&img, 128, 128);
        assert!(mid[0] > 100 && mid == px(&img, 140, 110), "{mid:?}");
        assert_eq!(px(&img, 5, 5), [0, 0, 0, 255]);
        // Turned an eighth about y, two faces show, lit differently.
        g.write(reg::YAW, 32);
        frame(&mut g);
        let img = render(&g.front, 0.0);
        assert_ne!(px(&img, 108, 128), px(&img, 148, 128));
    }

    #[test]
    fn spin_turns_the_shown_frame_and_the_registers() {
        let mut g = Gpu::default();
        cube(&mut g, 40);
        g.write(reg::MODE, mode::WIRE as u8);
        g.write(reg::SPIN_YAW, 4);
        frame(&mut g);
        assert_ne!(render(&g.front, 0.0), render(&g.front, 3.0));
        // 64 ticks of 4: a full turn, the cube as it was.
        assert_eq!(render(&g.front, 0.0), render(&g.front, 64.0));
        g.tick();
        assert_eq!(g.regs[reg::YAW as usize], 4);
        // A negative spin turns back.
        g.write(reg::SPIN_YAW, -4i8 as u8);
        g.tick();
        assert_eq!(g.regs[reg::YAW as usize], 0);
    }

    #[test]
    fn the_buffers_fill_in_order_and_wrap() {
        let mut g = Gpu::default();
        g.write(reg::HIGH, 0x0F);
        g.write(reg::VPTR, 0xFF);
        for c in [1, 2, 3, 4] {
            g.write(reg::VDATA, c);
        }
        assert_eq!(g.vertices[BUFFER - 1], [1, 2, 3]);
        assert_eq!(g.vertices[0], [4, 0, 0]);
        g.write(reg::HIGH, 0x0F);
        g.write(reg::IPTR, 0xFE);
        for i in [7, 8, 9] {
            g.write(reg::IDATA, i);
        }
        assert_eq!((g.indices[BUFFER - 2], g.indices[BUFFER - 1], g.indices[0]), (7, 8, 9));
    }

    /// `WRITES=file OUT=dir`: (register, byte) writes replayed, every shown
    /// frame a PPM picture.
    #[test]
    fn replay_writes() {
        let (Ok(file), Ok(dir)) = (std::env::var("WRITES"), std::env::var("OUT")) else { return };
        let writes: Vec<(u8, u8)> = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        let mut g = Gpu::default();
        for (r, d) in writes {
            let shown = g.version;
            g.write(r, d);
            if g.version != shown {
                let (w, h, img) = render(&g.front, 10.0);
                let rgb: Vec<u8> = img.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
                let mut ppm = format!("P6 {w} {h} 255\n").into_bytes();
                ppm.extend(rgb);
                std::fs::write(format!("{dir}/frame-{}.ppm", g.version), ppm).unwrap();
            }
        }
    }

    /// `OUT=dir`: the cube in each mode, as PPM pictures (for the docs).
    #[test]
    fn render_cubes() {
        let Ok(dir) = std::env::var("OUT") else { return };
        for (name, m, color) in [("lines", mode::WIRE, 0b000_111_00), ("faces", mode::FACES, 0b111_101_00)] {
            let mut g = Gpu::default();
            cube(&mut g, 30);
            g.write(reg::MODE, m as u8);
            g.write(reg::COLOR, color);
            g.write(reg::YAW, 20);
            g.write(reg::PITCH, 18);
            frame(&mut g);
            let (w, h, img) = render(&g.front, 0.0);
            let rgb: Vec<u8> = img.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            let mut ppm = format!("P6 {w} {h} 255\n").into_bytes();
            ppm.extend(rgb);
            std::fs::write(format!("{dir}/cube-{name}.ppm"), ppm).unwrap();
        }
    }
}
