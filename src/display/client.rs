//! Drawing what the server sends on the Displays' faces.
use std::collections::HashMap;
use std::time::{Duration, Instant};
use yog_api::sync::{LazyLock, Mutex};
use yog_api::yog_ids::time::TICKS_PER_SECOND;

use yog_api::gfx_core::{DataType, DrawMode};
use yog_api::gfx_gl::{Attrib, Buffer, ShaderProgram, Texture, VertexArray};

/// A vertex: position (3 floats), then its picture's uv (2): bytes per
/// vertex, and where the uv starts.
const STRIDE: u32 = 5 * FLOAT;
const UV_OFFSET: u32 = 3 * FLOAT;
const FLOAT: u32 = std::mem::size_of::<f32>() as u32;
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
    /// The picture's width over its height.
    aspect: f32,
    drawn: Option<Instant>,
}

struct Gl {
    program: ShaderProgram,
    vao: VertexArray,
    vbo: Buffer,
}

/// What each display shows, by its block position.
type ShownAt = HashMap<(i32, i32, i32), Shown>;

static SHOWN: LazyLock<Mutex<ShownAt>> = LazyLock::new(Default::default);
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
    let mut shown = SHOWN.lock();
    let key = frame.pos;
    if frame.size == 0 {
        shown.remove(&key);
        return;
    }
    let now = Instant::now();
    match shown.get_mut(&key) {
        Some(s)
            if s.frame.version == frame.version
                && s.frame.size == frame.size
                && s.frame.facing == frame.facing =>
        {
            s.heard = now
        }
        _ => {
            let since = now - Duration::from_millis(frame.age * 50);
            let spins = frame
                .ops
                .iter()
                .any(|op| matches!(op, gpu::Op::Draw(d) if d.spin != [0; 3]));
            let texture = shown.remove(&key).and_then(|s| s.texture);
            shown.insert(
                key,
                Shown {
                    frame,
                    heard: now,
                    since,
                    spins,
                    texture,
                    aspect: 1.0,
                    drawn: None,
                },
            );
        }
    }
}

/// The picture's corner (bottom left) and its sides (right, `width`
/// long, and up, `height`), in world coordinates: on the face it faces,
/// a hair out.
fn quad(
    pos: (i32, i32, i32),
    facing: Facing,
    size: f32,
    height: f32,
) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let (x, y, z) = (pos.0 as f32, pos.1 as f32, pos.2 as f32);
    const OUT: f32 = 0.01;
    let up = [0.0, height, 0.0];
    match facing {
        Facing::S => ([x, y, z + 1.0 + OUT], [size, 0.0, 0.0], up),
        Facing::N => ([x + 1.0, y, z - OUT], [-size, 0.0, 0.0], up),
        Facing::E => ([x + 1.0 + OUT, y, z + 1.0], [0.0, 0.0, -size], up),
        Facing::W => ([x - OUT, y, z], [0.0, 0.0, size], up),
    }
}

fn render(gfx: &GfxContext) {
    let mut shown = SHOWN.lock();
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
    let mut gl = GL.lock();
    if gl.is_none() {
        let Ok(program) = gfx.create_shader(VERT, FRAG) else {
            return;
        };
        let vao = gfx.create_vao();
        let vbo = gfx.create_buffer();
        vbo.upload_bytes(gfx, &[0u8; 6 * 5 * 4], true);
        let attrib = |index, components, offset| Attrib {
            index,
            components,
            dtype: DataType::F32,
            normalized: false,
            stride: STRIDE,
            offset,
        };
        vao.attrib(gfx, &vbo, attrib(0, 3, 0));
        vao.attrib(gfx, &vbo, attrib(1, 2, UV_OFFSET));
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
            let ticks = (now - s.since).as_secs_f32() * TICKS_PER_SECOND as f32;
            let (w, h, pixels) = gpu::render(&s.frame.ops, ticks);
            if let Some(t) = s.texture.take() {
                gfx.delete_texture(t);
            }
            s.texture = Some(gfx.create_texture_rgba(w as u32, h as u32, &pixels, false));
            s.aspect = w as f32 / h as f32;
            s.drawn = Some(now);
        }
        let Some(texture) = &s.texture else { continue };
        let size = s.frame.size as f32;
        let (o, r, u) = quad(s.frame.pos, s.frame.facing, size, size / s.aspect);
        let at = |a: f32, b: f32| [0, 1, 2].map(|i| o[i] + r[i] * a + u[i] * b - cam[i]);
        // Two triangles; the texture's top row at the top.
        let corners = [
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
        ];
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
    registry.on_client_packet(FRAME_CHANNEL, |ev| {
        if let Ok(frame) = yog_api::yog_data::read_bytes::<Frame>(&ev.payload) {
            received(frame);
        }
    });
    registry.on_world_render(render);
}
