//! The Display's GPU: a 256 × 256 screen behind registers written over a
//! digital bus (`display.rs` reads the bus; this is what the writes do).
//!
//! A program fills a **vertex buffer** (x, y, z — signed bytes) and an
//! **index buffer** (bytes, each a vertex), sets the transform, and draws
//! a range of indices as lines, wire triangles or flat-shaded faces. Draws
//! go into the back frame; SWAP shows it. Spin registers keep the shown
//! frame turning on their own — every tick, as long as nothing replaces it.
//!
//! Colors are RGB332 bytes (`rrrgggbb`).

use serde::{Deserialize, Serialize};

/// The screen's side, in pixels.
pub const SIZE: usize = 256;
/// Draw calls a frame keeps (a program that never clears stops adding).
const MAX_OPS: usize = 1024;

/// The registers (written as `REG ← DATA`).
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
    /// Turns, 256 to a full turn: about y, x, z.
    pub const YAW: u8 = 14;
    pub const PITCH: u8 = 15;
    pub const ROLL: u8 = 16;
    /// Pixels per 16 units.
    pub const SCALE: u8 = 17;
    /// What the turns gain each tick (signed).
    pub const SPIN_YAW: u8 = 18;
    pub const SPIN_PITCH: u8 = 19;
    pub const SPIN_ROLL: u8 = 20;
    pub const COUNT_OF: usize = 21;
}

/// What a write to CMD does.
pub mod cmd {
    /// Starts a new back frame, filled with BG.
    pub const CLEAR: u8 = 0;
    /// Draws COUNT indices from FIRST.
    pub const DRAW: u8 = 1;
    /// Shows the back frame.
    pub const SWAP: u8 = 2;
    /// A pixel at (X, Y).
    pub const PIXEL: u8 = 3;
    /// A line from (X, Y) to (X2, Y2).
    pub const LINE: u8 = 4;
}

/// How DRAW reads its indices.
pub mod mode {
    /// Two to a line.
    pub const LINES: u8 = 0;
    /// Three to a triangle, its edges.
    pub const WIRE: u8 = 1;
    /// Three to a triangle, filled and shaded; the side facing away isn't
    /// drawn (counter-clockwise faces the viewer).
    pub const FACES: u8 = 2;
}

/// A mesh drawn with a transform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draw {
    pub mode: u8,
    pub color: u8,
    /// Yaw, pitch, roll when drawn; then the spin they gain each tick.
    pub turn: [u8; 3],
    pub spin: [i8; 3],
    pub scale: u8,
    pub vertices: Vec<[i8; 3]>,
    pub indices: Vec<u8>,
}

/// One thing drawn into a frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    Clear(u8),
    Pixel { x: u8, y: u8, color: u8 },
    Line { from: (u8, u8), to: (u8, u8), color: u8 },
    Draw(Draw),
}

/// The GPU's state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gpu {
    pub regs: Vec<u8>,
    pub vertices: Vec<[i8; 3]>,
    pub indices: Vec<u8>,
    vptr: u8,
    /// The coordinate VDATA writes next (0 x, 1 y, 2 z).
    component: u8,
    iptr: u8,
    back: Vec<Op>,
    /// What's shown, and a number that changes when it does.
    pub front: Vec<Op>,
    pub version: u64,
}

impl Default for Gpu {
    fn default() -> Self {
        let mut regs = vec![0; reg::COUNT_OF];
        regs[reg::COLOR as usize] = 0xFF;
        regs[reg::SCALE as usize] = 32;
        regs[reg::COUNT as usize] = 0;
        Gpu {
            regs,
            vertices: vec![[0; 3]; 256],
            indices: vec![0; 256],
            vptr: 0,
            component: 0,
            iptr: 0,
            back: vec![Op::Clear(0)],
            front: vec![Op::Clear(0)],
            version: 0,
        }
    }
}

impl Gpu {
    fn r(&self, r: u8) -> u8 {
        self.regs[r as usize]
    }

    /// `REG ← DATA`.
    pub fn write(&mut self, r: u8, data: u8) {
        if (r as usize) < reg::COUNT_OF {
            self.regs[r as usize] = data;
        }
        match r {
            reg::CMD => self.command(data),
            reg::VPTR => {
                self.vptr = data;
                self.component = 0;
            }
            reg::VDATA => {
                self.vertices[self.vptr as usize][self.component as usize] = data as i8;
                self.component += 1;
                if self.component == 3 {
                    self.component = 0;
                    self.vptr = self.vptr.wrapping_add(1);
                }
            }
            reg::IPTR => self.iptr = data,
            reg::IDATA => {
                self.indices[self.iptr as usize] = data;
                self.iptr = self.iptr.wrapping_add(1);
            }
            _ => {}
        }
    }

    fn command(&mut self, c: u8) {
        let op = match c {
            cmd::CLEAR => {
                self.back.clear();
                Op::Clear(self.r(reg::BG))
            }
            cmd::DRAW => {
                let first = self.r(reg::FIRST) as usize;
                let count = self.r(reg::COUNT) as usize;
                let indices: Vec<u8> = (first..first + count).map(|i| self.indices[i % 256]).collect();
                // Only the vertices it uses, renumbered.
                let mut used: Vec<u8> = indices.clone();
                used.sort_unstable();
                used.dedup();
                Op::Draw(Draw {
                    mode: self.r(reg::MODE),
                    color: self.r(reg::COLOR),
                    turn: [self.r(reg::YAW), self.r(reg::PITCH), self.r(reg::ROLL)],
                    spin: [self.r(reg::SPIN_YAW) as i8, self.r(reg::SPIN_PITCH) as i8, self.r(reg::SPIN_ROLL) as i8],
                    scale: self.r(reg::SCALE),
                    vertices: used.iter().map(|&v| self.vertices[v as usize]).collect(),
                    indices: indices.iter().map(|i| used.binary_search(i).unwrap() as u8).collect(),
                })
            }
            cmd::SWAP => {
                self.front = self.back.clone();
                self.version += 1;
                return;
            }
            cmd::PIXEL => Op::Pixel { x: self.r(reg::X), y: self.r(reg::Y), color: self.r(reg::COLOR) },
            cmd::LINE => Op::Line {
                from: (self.r(reg::X), self.r(reg::Y)),
                to: (self.r(reg::X2), self.r(reg::Y2)),
                color: self.r(reg::COLOR),
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
    px: Vec<[u8; 4]>,
    depth: Vec<f32>,
}

impl Raster {
    fn put(&mut self, x: i32, y: i32, c: [u8; 4]) {
        if (0..SIZE as i32).contains(&x) && (0..SIZE as i32).contains(&y) {
            self.px[y as usize * SIZE + x as usize] = c;
        }
    }

    fn line(&mut self, (mut x0, mut y0): (i32, i32), (x1, y1): (i32, i32), c: [u8; 4]) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        // Off-screen ends are clipped by `put`; a line can't be longer than this.
        for _ in 0..4 * SIZE {
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
        let hi = |i: usize| p.iter().map(|q| q[i]).fold(f32::MIN, f32::max).ceil().min(SIZE as f32 - 1.0) as i32;
        for y in lo(1)..=hi(1) {
            for x in lo(0)..=hi(0) {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let w = [edge(p[1], p[2], fx, fy) / area, edge(p[2], p[0], fx, fy) / area, edge(p[0], p[1], fx, fy) / area];
                if w.iter().all(|&w| w >= 0.0) {
                    let z = w[0] * p[0][2] + w[1] * p[1][2] + w[2] * p[2][2];
                    let i = y as usize * SIZE + x as usize;
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

/// Where the eye is, in pixels in front of the screen.
const EYE: f32 = 600.0;

/// A view-space point on the screen: x, y and its depth.
fn project(v: [f32; 3]) -> [f32; 3] {
    let f = EYE / (EYE - v[2]).max(1.0);
    let c = SIZE as f32 / 2.0;
    [c + v[0] * f, c - v[1] * f, v[2]]
}

/// Draws a frame `ticks` after it was shown: RGBA, rows from the top.
pub fn render(ops: &[Op], ticks: f32) -> Vec<u8> {
    let mut r = Raster { px: vec![rgba(0, 1.0); SIZE * SIZE], depth: vec![f32::MIN; SIZE * SIZE] };
    for op in ops {
        match op {
            Op::Clear(c) => {
                r.px.fill(rgba(*c, 1.0));
                r.depth.fill(f32::MIN);
            }
            Op::Pixel { x, y, color } => r.put(*x as i32, *y as i32, rgba(*color, 1.0)),
            Op::Line { from, to, color } => r.line((from.0 as i32, from.1 as i32), (to.0 as i32, to.1 as i32), rgba(*color, 1.0)),
            Op::Draw(d) => {
                let turn = [0, 1, 2].map(|i| d.turn[i] as f32 + d.spin[i] as f32 * ticks);
                let view = transform(d, turn);
                let screen: Vec<[f32; 3]> = view.iter().map(|&v| project(v)).collect();
                let at = |i: u8| screen.get(i as usize).copied();
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
    r.px.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * SIZE + x) * 4;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    fn cube(g: &mut Gpu) {
        g.write(reg::VPTR, 0);
        for v in [[-1, -1, -1], [1, -1, -1], [1, 1, -1], [-1, 1, -1], [-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1]] {
            for c in v {
                g.write(reg::VDATA, (c * 40) as u8);
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
    }

    #[test]
    fn a_pixel_and_a_line_land_where_asked() {
        let mut g = Gpu::default();
        g.write(reg::BG, 0b000_000_01);
        g.write(reg::CMD, cmd::CLEAR);
        g.write(reg::COLOR, 0b111_000_00);
        g.write(reg::X, 10);
        g.write(reg::Y, 20);
        g.write(reg::CMD, cmd::PIXEL);
        g.write(reg::X2, 50);
        g.write(reg::Y2, 20);
        g.write(reg::CMD, cmd::LINE);
        // Not shown till SWAP.
        assert_eq!(g.front, vec![Op::Clear(0)]);
        g.write(reg::CMD, cmd::SWAP);
        let img = render(&g.front, 0.0);
        assert_eq!(px(&img, 10, 20), [255, 0, 0, 255]);
        assert_eq!(px(&img, 30, 20), [255, 0, 0, 255]);
        assert_eq!(px(&img, 30, 21), [0, 0, 85, 255]);
    }

    #[test]
    fn a_cube_of_faces_covers_the_middle_and_hides_its_back() {
        let mut g = Gpu::default();
        cube(&mut g);
        g.write(reg::MODE, mode::FACES);
        g.write(reg::COUNT, 36);
        g.write(reg::COLOR, 0b111_111_11);
        g.write(reg::CMD, cmd::CLEAR);
        g.write(reg::CMD, cmd::DRAW);
        g.write(reg::CMD, cmd::SWAP);
        let Op::Draw(d) = &g.front[1] else { panic!() };
        assert_eq!((d.vertices.len(), d.indices.len()), (8, 36));
        let img = render(&g.front, 0.0);
        // Face on: the front face, lit, fills the middle; the corners stay clear.
        let mid = px(&img, 128, 128);
        assert!(mid[0] > 100 && mid == px(&img, 140, 110), "{mid:?}");
        assert_eq!(px(&img, 5, 5), [0, 0, 0, 255]);
        // Turned an eighth about y, two faces show, lit differently.
        g.write(reg::YAW, 32);
        g.write(reg::CMD, cmd::CLEAR);
        g.write(reg::CMD, cmd::DRAW);
        g.write(reg::CMD, cmd::SWAP);
        let img = render(&g.front, 0.0);
        assert_ne!(px(&img, 108, 128), px(&img, 148, 128));
    }

    #[test]
    fn spin_turns_the_shown_frame_and_the_registers() {
        let mut g = Gpu::default();
        cube(&mut g);
        g.write(reg::MODE, mode::WIRE);
        g.write(reg::COUNT, 36);
        g.write(reg::SPIN_YAW, 4);
        g.write(reg::CMD, cmd::DRAW);
        g.write(reg::CMD, cmd::SWAP);
        assert_ne!(render(&g.front, 0.0), render(&g.front, 3.0));
        // 64 ticks of 4: a full turn, the cube as it was.
        assert_eq!(render(&g.front, 0.0), render(&g.front, 64.0));
        g.tick();
        assert_eq!(g.regs[reg::YAW as usize], 4);
    }

    /// `OUT=dir`: the cube in each mode, as PPM pictures (for the docs).
    #[test]
    fn render_cubes() {
        let Ok(dir) = std::env::var("OUT") else { return };
        for (name, m, color) in [("lines", mode::WIRE, 0b000_111_00), ("faces", mode::FACES, 0b111_101_00)] {
            let mut g = Gpu::default();
            cube(&mut g);
            g.write(reg::MODE, m);
            g.write(reg::COUNT, 36);
            g.write(reg::COLOR, color);
            g.write(reg::YAW, 20);
            g.write(reg::PITCH, 18);
            g.write(reg::CMD, cmd::DRAW);
            g.write(reg::CMD, cmd::SWAP);
            let img = render(&g.front, 0.0);
            let rgb: Vec<u8> = img.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            let mut ppm = format!("P6 {SIZE} {SIZE} 255\n").into_bytes();
            ppm.extend(rgb);
            std::fs::write(format!("{dir}/cube-{name}.ppm"), ppm).unwrap();
        }
    }

    #[test]
    fn the_buffers_fill_in_order_and_wrap() {
        let mut g = Gpu::default();
        g.write(reg::VPTR, 255);
        for c in [1, 2, 3, 4] {
            g.write(reg::VDATA, c);
        }
        assert_eq!(g.vertices[255], [1, 2, 3]);
        assert_eq!(g.vertices[0], [4, 0, 0]);
        g.write(reg::IPTR, 254);
        for i in [7, 8, 9] {
            g.write(reg::IDATA, i);
        }
        assert_eq!((g.indices[254], g.indices[255], g.indices[0]), (7, 8, 9));
    }
}
