//! Displays: a 256 × 256 screen with a GPU (`gpu.rs`) on a digital bus.
//!
//! A Display reads writes to its registers off the bus it joins, from its
//! first channel `c` on:
//!
//! | channels | what |
//! |---|---|
//! | c … c+7 | DATA, bit 0 first |
//! | c+8 … c+15 | REG, the register DATA goes to |
//! | c+16 | TOGGLE: each change is one write, `REG ← DATA` |
//! | c+17 | ACK: the Display drives it to TOGGLE once it has taken the write |
//!
//! A bus changes once a tick, so a Display takes one write a tick; a writer
//! flips TOGGLE and waits for ACK to follow before its next write.
//!
//! The server keeps every Display's GPU; what one shows goes to the players
//! near it, who draw it on its front face (`client`).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use yog_api::{BlockPos, EventPhase, Registry, Storage};

use crate::cables;
use crate::content::Blocks;
use crate::gpu::{Gpu, Op};

/// Channels a Display takes, from its first.
pub const CHANNELS: u16 = 18;
const TOGGLE: u16 = 16;
const ACK: u16 = 17;
/// Size, in blocks, a new Display shows its picture at.
const DEFAULT_SIZE: u16 = 16;
/// The biggest picture, in blocks a side.
pub const MAX_SIZE: u16 = 1024;
/// Server → client: what a Display shows.
const FRAME_CHANNEL: &str = "yog-vlsi:display_frame";
/// How far past its picture's middle players see a Display from, in blocks.
const VIEW_DISTANCE: f64 = 160.0;
/// Every so many ticks a player near a Display is sent it again (one who
/// came near, or a client that dropped it).
const RESEND_EVERY: u64 = 20;

/// (dimension, x, y, z).
pub type Key = cables::Key;

/// Which way a Display's picture faces: `N` `S` `E` `W`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Facing {
    N,
    S,
    E,
    W,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Display {
    pub facing: Facing,
    /// The picture's side, in blocks.
    pub size: u16,
    /// The first bus channel (from 1).
    pub channel: u16,
    /// TOGGLE as last taken.
    toggle: bool,
    gpu: Gpu,
    /// The tick what it shows was swapped in (not kept).
    #[serde(skip)]
    shown_at: u64,
    #[serde(skip)]
    shown_version: u64,
}

impl Display {
    pub fn new(facing: Facing) -> Self {
        Display { facing, size: DEFAULT_SIZE, channel: 1, toggle: false, gpu: Gpu::default(), shown_at: 0, shown_version: 0 }
    }

    /// The bus channel (0-based) `offset` past its first.
    fn ch(&self, offset: u16) -> u8 {
        (self.channel.clamp(1, cables::BUS_CHANNELS - CHANNELS + 1) - 1 + offset) as u8
    }
}

static DISPLAYS: LazyLock<Mutex<HashMap<Key, Display>>> = LazyLock::new(Default::default);
/// What each player was last sent of each Display: its version.
static SENT: LazyLock<Mutex<HashMap<(String, Key), u64>>> = LazyLock::new(Default::default);
static TICKS: Mutex<u64> = Mutex::new(0);

/// What a client is sent: a Display and what it shows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub pos: (i32, i32, i32),
    pub facing: Facing,
    /// 0: it's gone.
    pub size: u16,
    pub version: u64,
    /// Ticks since it was shown (its spin has gone on that long).
    pub age: u64,
    pub ops: Vec<Op>,
}

pub fn get(dim: &str, pos: BlockPos) -> Option<Display> {
    DISPLAYS.lock().unwrap().get(&(dim.to_string(), pos.x, pos.y, pos.z)).cloned()
}

/// Sets a Display's size and first channel.
pub fn configure(dim: &str, pos: BlockPos, size: u16, channel: u16) {
    if let Some(d) = DISPLAYS.lock().unwrap().get_mut(&(dim.to_string(), pos.x, pos.y, pos.z)) {
        d.size = size.clamp(1, MAX_SIZE);
        d.channel = channel.clamp(1, cables::BUS_CHANNELS - CHANNELS + 1);
    }
}

/// The bus's writes into the GPUs, after the ALUs stepped; then what they
/// show out to the players near them.
pub fn tick(srv: &dyn yog_api::Server) {
    let n = {
        let mut t = TICKS.lock().unwrap();
        *t += 1;
        *t
    };
    {
        let mut displays = DISPLAYS.lock().unwrap();
        for ((dim, x, y, z), d) in displays.iter_mut() {
            let pos = BlockPos { x: *x, y: *y, z: *z };
            if n == 1 {
                // Joined to cables written into the world while the server was down.
                cables::refresh_around(srv, dim, pos);
            }
            let bit = |offset: u16| cables::bus_level_at(dim, pos, d.ch(offset)) > 0;
            let toggle = bit(TOGGLE);
            if toggle != d.toggle {
                let byte = |from: u16| (0..8).fold(0u8, |b, i| b | (u8::from(bit(from + i)) << i));
                let (data, reg) = (byte(0), byte(8));
                d.gpu.write(reg, data);
                d.toggle = toggle;
            }
            cables::bus_drive_at(dim, pos, d.ch(ACK), if d.toggle { 15 } else { 0 });
            d.gpu.tick();
            if d.gpu.version != d.shown_version {
                d.shown_version = d.gpu.version;
                d.shown_at = n;
            }
        }
    }
    send(srv, n);
}

/// What the players near each Display haven't got of it.
fn send(srv: &dyn yog_api::Server, n: u64) {
    let displays = DISPLAYS.lock().unwrap();
    if displays.is_empty() {
        return;
    }
    let mut sent = SENT.lock().unwrap();
    for player in srv.online_players() {
        let Some(info) = srv.player_info(&player).and_then(|p| srv.entity_info(&p.uuid)) else { continue };
        for (key, d) in displays.iter() {
            let (dim, x, y, z) = key;
            // From the picture's middle (roughly: it spreads up and to one side).
            let half = d.size as f64 / 2.0;
            let (dx, dy, dz) = (info.pos.0 - *x as f64, info.pos.1 - (*y as f64 + half), info.pos.2 - *z as f64);
            let reach = VIEW_DISTANCE + d.size as f64;
            if *dim != info.dimension || dx * dx + dy * dy + dz * dz > reach * reach {
                continue;
            }
            let id = (player.clone(), key.clone());
            if sent.get(&id) == Some(&d.gpu.version) && !n.is_multiple_of(RESEND_EVERY) {
                continue;
            }
            let frame = Frame {
                pos: (*x, *y, *z),
                facing: d.facing,
                size: d.size,
                version: d.gpu.version,
                age: n.saturating_sub(d.shown_at),
                ops: d.gpu.front.clone(),
            };
            if let Ok(json) = serde_json::to_string(&frame) {
                srv.send_to_player(&player, FRAME_CHANNEL, json.as_bytes());
            }
            sent.insert(id, d.gpu.version);
        }
    }
}

/// The way a Display's front faces: its block's `facing`.
fn facing_of(srv: &dyn yog_api::Server, dim: &str, pos: BlockPos) -> Facing {
    let state = srv.get_block_state(dim, pos).unwrap_or_default();
    match state.split("facing=").nth(1).and_then(|r| r.split([',', ']']).next()) {
        Some("north") => Facing::N,
        Some("east") => Facing::E,
        Some("west") => Facing::W,
        _ => Facing::S,
    }
}

const STORE_FILE: &str = "yog-vlsi/displays";
const STORE_KEY: &str = "displays";

pub fn load(srv: &dyn yog_api::Server) {
    let store = Storage::open(&srv.game_dir(), STORE_FILE);
    let saved: Vec<(Key, Display)> = store
        .get(STORE_KEY)
        .and_then(|v| v.as_str())
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    *DISPLAYS.lock().unwrap() = saved.into_iter().collect();
    *TICKS.lock().unwrap() = 0;
}

pub fn save(srv: &dyn yog_api::Server) {
    let saved: Vec<(Key, Display)> = DISPLAYS.lock().unwrap().iter().map(|(k, d)| (k.clone(), d.clone())).collect();
    if let Ok(json) = serde_json::to_string(&saved) {
        let mut store = Storage::open(&srv.game_dir(), STORE_FILE);
        store.set(STORE_KEY, &*json);
        let _ = store.flush();
    }
}

pub fn register(registry: &mut Registry) {
    registry.on_player_place_block(|e, phase, srv| {
        if phase == EventPhase::Post && e.block_id == Blocks::DISPLAY {
            let facing = facing_of(srv, &e.dimension, e.pos);
            DISPLAYS
                .lock()
                .unwrap()
                .insert((e.dimension.clone(), e.pos.x, e.pos.y, e.pos.z), Display::new(facing));
        }
        true
    });
    registry.on_block_break(|e, phase, srv| {
        if phase == EventPhase::Post && e.block_id == Blocks::DISPLAY {
            let key = (e.dimension.clone(), e.pos.x, e.pos.y, e.pos.z);
            if let Some(d) = DISPLAYS.lock().unwrap().remove(&key) {
                let gone = Frame { pos: (key.1, key.2, key.3), facing: d.facing, size: 0, version: 0, age: 0, ops: Vec::new() };
                if let Ok(json) = serde_json::to_string(&gone) {
                    for player in srv.online_players() {
                        srv.send_to_player(&player, FRAME_CHANNEL, json.as_bytes());
                    }
                }
            }
            SENT.lock().unwrap().retain(|(_, k), _| *k != key);
        }
        true
    });
    registry.on_player_leave(|e, _phase, _srv| {
        SENT.lock().unwrap().retain(|(p, _), _| *p != e.player_name);
        true
    });
    client::register(registry);
}

/// Drawing what the server sends on the Displays' faces.
mod client {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    use std::time::{Duration, Instant};

    use yog_api::gfx_core::{DataType, DrawMode};
    use yog_api::gfx_gl::{Buffer, ShaderProgram, Texture, VertexArray};
    use yog_api::{GfxContext, Registry};

    use super::{FRAME_CHANNEL, Facing, Frame};
    use crate::gpu;

    /// A Display not heard of for this long is dropped (out of range, or
    /// in another dimension).
    const FORGET_AFTER: Duration = Duration::from_secs(3);
    /// How often a turning picture is drawn again.
    const REDRAW_EVERY: Duration = Duration::from_millis(33);

    struct Shown {
        frame: Frame,
        heard: Instant,
        /// When it was shown, by this clock.
        since: Instant,
        spins: bool,
        texture: Option<Texture>,
        drawn: Option<Instant>,
    }

    struct Gl {
        program: ShaderProgram,
        vao: VertexArray,
        vbo: Buffer,
    }

    static SHOWN: LazyLock<Mutex<HashMap<(i32, i32, i32), Shown>>> = LazyLock::new(Default::default);
    static GL: Mutex<Option<Gl>> = Mutex::new(None);

    const VERT: &str = "#version 330 core
layout(location = 0) in vec3 pos;
layout(location = 1) in vec2 uv;
uniform mat4 view_proj;
out vec2 v_uv;
void main() {
    v_uv = uv;
    gl_Position = view_proj * vec4(pos, 1.0);
}";
    const FRAG: &str = "#version 330 core
in vec2 v_uv;
uniform sampler2D screen;
out vec4 color;
void main() {
    color = texture(screen, v_uv);
}";

    fn received(frame: Frame) {
        let mut shown = SHOWN.lock().unwrap();
        let key = frame.pos;
        if frame.size == 0 {
            shown.remove(&key);
            return;
        }
        let now = Instant::now();
        match shown.get_mut(&key) {
            Some(s) if s.frame.version == frame.version && s.frame.size == frame.size && s.frame.facing == frame.facing => s.heard = now,
            _ => {
                let since = now - Duration::from_millis(frame.age * 50);
                let spins = frame.ops.iter().any(|op| matches!(op, gpu::Op::Draw(d) if d.spin != [0; 3]));
                let texture = shown.remove(&key).and_then(|s| s.texture);
                shown.insert(key, Shown { frame, heard: now, since, spins, texture, drawn: None });
            }
        }
    }

    /// The picture's corners (bottom left, then right and up, each `size`
    /// long), in world coordinates: on the face it faces, a hair out.
    fn quad(pos: (i32, i32, i32), facing: Facing, size: f32) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let (x, y, z) = (pos.0 as f32, pos.1 as f32, pos.2 as f32);
        const OUT: f32 = 0.01;
        let up = [0.0, size, 0.0];
        match facing {
            Facing::S => ([x, y, z + 1.0 + OUT], [size, 0.0, 0.0], up),
            Facing::N => ([x + 1.0, y, z - OUT], [-size, 0.0, 0.0], up),
            Facing::E => ([x + 1.0 + OUT, y, z + 1.0], [0.0, 0.0, -size], up),
            Facing::W => ([x - OUT, y, z], [0.0, 0.0, size], up),
        }
    }

    fn render(gfx: &GfxContext) {
        let mut shown = SHOWN.lock().unwrap();
        let now = Instant::now();
        shown.retain(|_, s| {
            let keep = now - s.heard < FORGET_AFTER;
            if !keep && let Some(t) = s.texture.take() {
                gfx.delete_texture(t);
            }
            keep
        });
        if shown.is_empty() {
            return;
        }
        let mut gl = GL.lock().unwrap();
        if gl.is_none() {
            let Ok(program) = gfx.create_shader(VERT, FRAG) else { return };
            let vao = gfx.create_vao();
            let vbo = gfx.create_buffer();
            vbo.upload_bytes(gfx, &[0u8; 6 * 5 * 4], true);
            vao.attrib(gfx, &vbo, 0, 3, DataType::F32, false, 20, 0);
            vao.attrib(gfx, &vbo, 1, 2, DataType::F32, false, 20, 12);
            *gl = Some(Gl { program, vao, vbo });
        }
        let Some(gl) = gl.as_ref() else { return };
        let cam = gfx.camera_pos();
        gl.program.uniform_mat4(gfx, "view_proj", &gfx.view_proj());
        gl.program.uniform_1i(gfx, "screen", 0);
        gfx.set_depth(true, true);
        gfx.set_blend(false, 0, 0);
        for s in shown.values_mut() {
            let stale = match s.drawn {
                None => true,
                Some(at) => s.spins && now - at >= REDRAW_EVERY,
            };
            if stale {
                let ticks = (now - s.since).as_secs_f32() * 20.0;
                let pixels = gpu::render(&s.frame.ops, ticks);
                if let Some(t) = s.texture.take() {
                    gfx.delete_texture(t);
                }
                s.texture = Some(gfx.create_texture_rgba(gpu::SIZE as u32, gpu::SIZE as u32, &pixels, false));
                s.drawn = Some(now);
            }
            let Some(texture) = &s.texture else { continue };
            let (o, r, u) = quad(s.frame.pos, s.frame.facing, s.frame.size as f32);
            let at = |a: f32, b: f32| [0, 1, 2].map(|i| o[i] + r[i] * a + u[i] * b - cam[i]);
            // Two triangles; the texture's top row at the top.
            let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
            let mut data = Vec::with_capacity(6 * 5 * 4);
            for (a, b) in corners {
                let p = at(a, b);
                for v in [p[0], p[1], p[2], a, 1.0 - b] {
                    data.extend_from_slice(&v.to_ne_bytes());
                }
            }
            gl.vbo.subdata_bytes(gfx, 0, &data);
            gfx.bind_texture(0, texture);
            gfx.draw_arrays(&gl.vao, &gl.program, DrawMode::Triangles, 0, 6);
        }
    }

    pub fn register(registry: &mut Registry) {
        registry.on_client_packet(FRAME_CHANNEL, |ev, _srv| {
            if let Ok(frame) = serde_json::from_slice::<Frame>(&ev.payload) {
                received(frame);
            }
        });
        registry.on_world_render(render);
    }
}
