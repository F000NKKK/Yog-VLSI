//! Displays: a screen of up to 1024 × 1024 pixels with a GPU (`gpu.rs`) on
//! a digital bus, shown 1 to 1024 blocks wide.
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
use yog_api::sync::{LazyLock, Mutex};

use yog_api::Schema;
use yog_api::yog_ids::{game, gamemode};
use yog_api::{BlockPos, EventPhase, Registry};

use crate::cables;
use crate::content::Blocks;
use crate::gpu::{Gpu, Op};

/// Channels a Display takes, from its first.
pub const CHANNELS: u16 = 18;
const TOGGLE: u16 = 16;
const ACK: u16 = 17;
/// Size, in blocks, a new Display shows its picture at.
const DEFAULT_SIZE: u16 = 8;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Schema)]
pub enum Facing {
    N,
    S,
    E,
    W,
}

#[derive(Debug, Clone, Schema)]
pub struct Display {
    pub facing: Facing,
    /// The picture's side, in blocks.
    pub size: u16,
    /// The first bus channel (from 1).
    pub channel: u16,
    /// TOGGLE as last taken.
    #[data(default)]
    toggle: bool,
    #[data(default)]
    gpu: Gpu,
    /// The tick what it shows was swapped in (not kept).
    #[data(skip)]
    shown_at: u64,
    #[data(skip)]
    shown_version: u64,
}

impl Display {
    pub fn new(facing: Facing) -> Self {
        Display {
            facing,
            size: DEFAULT_SIZE,
            channel: 1,
            toggle: false,
            gpu: Gpu::default(),
            shown_at: 0,
            shown_version: 0,
        }
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
#[derive(Debug, Clone, Schema)]
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
    DISPLAYS
        .lock()
        .get(&(dim.to_string(), pos.x, pos.y, pos.z))
        .cloned()
}

/// Sets a Display's size and first channel; a size that doesn't fit is
/// refused (`Err`), the channel set all the same.
pub fn configure(dim: &str, pos: BlockPos, size: u16, channel: u16) -> Result<(), ()> {
    let mut displays = DISPLAYS.lock();
    let Some(d) = displays.get_mut(&(dim.to_string(), pos.x, pos.y, pos.z)) else {
        return Ok(());
    };
    d.channel = channel.clamp(1, cables::BUS_CHANNELS - CHANNELS + 1);
    let size = size.clamp(1, MAX_SIZE);
    if size == d.size {
        return Ok(());
    }
    let old = area(pos, d.facing, d.size);
    if !fits(dim, pos, d.facing, size, Some(old)) {
        return Err(());
    }
    yog_api::event::world::fill_region(dim, old.0, old.1, yog_api::yog_ids::game::AIR);
    d.size = size;
    stand(dim, pos, d);
    Ok(())
}

// ── The blocks a Display stands in ───────────────────────────────────────────

impl Facing {
    fn word(self) -> &'static str {
        match self {
            Facing::N => "north",
            Facing::S => "south",
            Facing::E => "east",
            Facing::W => "west",
        }
    }

    /// Right, seen from in front.
    fn right(self) -> (i32, i32) {
        match self {
            Facing::S => (1, 0),
            Facing::N => (-1, 0),
            Facing::E => (0, -1),
            Facing::W => (0, 1),
        }
    }
}

/// The box a Display's picture fills: from its block (bottom left, seen from
/// the front) `size` blocks right and up.
fn area(pos: BlockPos, facing: Facing, size: u16) -> (BlockPos, BlockPos) {
    let n = size as i32 - 1;
    let (dx, dz) = facing.right();
    let far = (pos.x + dx * n, pos.y + n, pos.z + dz * n);
    (
        BlockPos {
            x: pos.x.min(far.0),
            y: pos.y,
            z: pos.z.min(far.2),
        },
        BlockPos {
            x: pos.x.max(far.0),
            y: far.1,
            z: pos.z.max(far.2),
        },
    )
}

fn inside(b: (BlockPos, BlockPos), p: BlockPos) -> bool {
    (b.0.x..=b.1.x).contains(&p.x)
        && (b.0.y..=b.1.y).contains(&p.y)
        && (b.0.z..=b.1.z).contains(&p.z)
}

/// The world's top, a Display may not reach past.
const WORLD_TOP: i32 = 319;

/// Whether a picture `size` blocks a side fits at `pos`: nothing in its box
/// but the Display itself and, resizing, the blocks it stands in now (`own`).
fn fits(
    dim: &str,
    pos: BlockPos,
    facing: Facing,
    size: u16,
    own: Option<(BlockPos, BlockPos)>,
) -> bool {
    let b = area(pos, facing, size);
    b.1.y <= WORLD_TOP
        && yog_api::event::world::scan_region(dim, b.0, b.1)
            .iter()
            .all(|b| b.pos == pos || own.is_some_and(|o| inside(o, b.pos)))
}

/// Stands a Display's parts in its box and the Display at its corner.
fn stand(dim: &str, pos: BlockPos, d: &Display) {
    let b = area(pos, d.facing, d.size);
    yog_api::event::world::fill_region(
        dim,
        b.0,
        b.1,
        format!("{}[facing={}]", Blocks::DISPLAY_PART, d.facing.word()),
    );
    yog_api::event::block::set_state(
        dim,
        pos,
        format!("{}[facing={}]", Blocks::DISPLAY, d.facing.word()),
    );
}

/// The Display whose box holds `pos`.
pub fn owner(dim: &str, pos: BlockPos) -> Option<BlockPos> {
    DISPLAYS.lock().iter().find_map(|((d, x, y, z), disp)| {
        let at = BlockPos {
            x: *x,
            y: *y,
            z: *z,
        };
        (d == dim && inside(area(at, disp.facing, disp.size), pos)).then_some(at)
    })
}

/// The bus's writes into the GPUs, after the ALUs stepped; then what they
/// show out to the players near them.
pub fn tick() {
    let n = {
        let mut t = TICKS.lock();
        *t += 1;
        *t
    };
    {
        let mut displays = DISPLAYS.lock();
        for ((dim, x, y, z), d) in displays.iter_mut() {
            let pos = BlockPos {
                x: *x,
                y: *y,
                z: *z,
            };
            if n == 1 {
                // Joined to cables written into the world while the server was down.
                cables::refresh_around(dim, pos);
            }
            let bit = |offset: u16| cables::bus_level_at(dim, pos, d.ch(offset)) > 0;
            let toggle = bit(TOGGLE);
            if toggle != d.toggle {
                let byte = |from: u16| (0..8).fold(0u8, |b, i| b | (u8::from(bit(from + i)) << i));
                let (data, reg) = (byte(0), byte(8));
                d.gpu.write_register(reg, data);
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
    send(n);
}

/// What the players near each Display haven't got of it.
fn send(n: u64) {
    let displays = DISPLAYS.lock();
    if displays.is_empty() {
        return;
    }
    let mut sent = SENT.lock();
    for player in yog_api::event::player::online() {
        let Some(info) = yog_api::event::player::info(&player)
            .and_then(|p| yog_api::event::entity::info(&p.uuid))
        else {
            continue;
        };
        for (key, d) in displays.iter() {
            let (dim, x, y, z) = key;
            // From the picture's middle (roughly: it spreads up and to one side).
            let half = d.size as f64 / 2.0;
            let (dx, dy, dz) = (
                info.pos.x - *x as f64,
                info.pos.y - (*y as f64 + half),
                info.pos.z - *z as f64,
            );
            let reach = VIEW_DISTANCE + d.size as f64;
            if dim.as_str() != info.dimension.as_str()
                || dx * dx + dy * dy + dz * dz > reach * reach
            {
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
            let bytes = yog_api::yog_data::to_bytes(&frame);
            yog_api::event::net::send_to_player(&player, FRAME_CHANNEL, &bytes);
            sent.insert(id, d.gpu.version);
        }
    }
}

/// The way a Display's front faces: its block's `facing`.
fn facing_of(dim: &str, pos: BlockPos) -> Facing {
    let state = yog_api::event::block::state(dim, pos).unwrap_or_default();
    match state
        .split("facing=")
        .nth(1)
        .and_then(|r| r.split([',', ']']).next())
    {
        Some("north") => Facing::N,
        Some("east") => Facing::E,
        Some("west") => Facing::W,
        _ => Facing::S,
    }
}

/// Where it's kept in the world store.
const STORE_KEY: &str = "displays";

pub fn load() {
    let saved: Vec<(Key, Display)> = crate::saved::load(STORE_KEY);
    *DISPLAYS.lock() = saved.into_iter().collect();
    *TICKS.lock() = 0;
}

pub fn save() {
    let saved: Vec<(Key, Display)> = DISPLAYS
        .lock()
        .iter()
        .map(|(k, d)| (k.clone(), d.clone()))
        .collect();
    crate::saved::save(STORE_KEY, &saved);
}

pub fn register(registry: &mut Registry) {
    registry.on::<yog_api::event::block::Place, _>(|e, cx| {
        if e.block_id == Blocks::DISPLAY && cx.phase() == EventPhase::Post {
            placed(e);
        }
        true
    });
    registry.on::<yog_api::event::block::Break, _>(|e, cx| {
        let part = e.block_id == Blocks::DISPLAY_PART;
        if cx.phase() == EventPhase::Post && (part || e.block_id == Blocks::DISPLAY) {
            broken(e, part);
        }
        true
    });
    registry.on::<yog_api::event::player::session::Leave, _>(|e, _| {
        SENT.lock().retain(|(p, _), _| *p != e.player_name);
        true
    });
    client::register(registry);
}

/// Whether `player` plays in creative (what they break isn't given back).
fn creative(player: &str) -> bool {
    yog_api::event::player::info(player).is_some_and(|p| p.gamemode == gamemode::CREATIVE)
}

/// A Display goes up only where its whole picture fits, and stands in
/// blocks over all of it; without room it comes back off.
fn placed(e: &yog_api::event::block::Place) {
    // It faces as it was placed.
    let d = Display::new(facing_of(&e.dimension, e.pos));
    if !fits(&e.dimension, e.pos, d.facing, d.size, None) {
        yog_api::event::block::set_state(&e.dimension, e.pos, game::AIR);
        if !creative(&e.player_name) {
            yog_api::event::player::give_item(&e.player_name, Blocks::DISPLAY, 1u32);
        }
        yog_api::event::chat::actionbar(
            &e.player_name,
            crate::res::Strings::display_no_room(DEFAULT_SIZE),
        );
        return;
    }
    stand(&e.dimension, e.pos, &d);
    DISPLAYS
        .lock()
        .insert((e.dimension.clone(), e.pos.x, e.pos.y, e.pos.z), d);
}

/// Breaking any block of it takes the whole Display down; a part (`part`)
/// drops the Display (the Display's own block drops itself).
fn broken(e: &yog_api::event::block::Break, part: bool) {
    let at = if part {
        owner(&e.dimension, e.pos)
    } else {
        Some(e.pos)
    };
    let Some(at) = at else { return };
    let key = (e.dimension.clone(), at.x, at.y, at.z);
    if let Some(d) = DISPLAYS.lock().remove(&key) {
        let b = area(at, d.facing, d.size);
        yog_api::event::world::fill_region(&e.dimension, b.0, b.1, game::AIR);
        if part && !creative(&e.player_name) {
            yog_api::event::player::give_item(&e.player_name, Blocks::DISPLAY, 1u32);
        }
        let gone = Frame {
            pos: (key.1, key.2, key.3),
            facing: d.facing,
            size: 0,
            version: 0,
            age: 0,
            ops: Vec::new(),
        };
        let bytes = yog_api::yog_data::to_bytes(&gone);
        for player in yog_api::event::player::online() {
            yog_api::event::net::send_to_player(&player, FRAME_CHANNEL, &bytes);
        }
    }
    SENT.lock().retain(|(_, k), _| *k != key);
}

/// Drawing what the server sends on the Displays' faces.
mod client;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_goes_right_and_up_from_the_display() {
        let at = BlockPos {
            x: 10,
            y: 64,
            z: -5,
        };
        let b = |f| area(at, f, 3);
        // Seen from the south (it faces south), right is east.
        assert_eq!(
            b(Facing::S),
            (
                at,
                BlockPos {
                    x: 12,
                    y: 66,
                    z: -5
                }
            )
        );
        assert_eq!(
            b(Facing::N),
            (
                BlockPos { x: 8, y: 64, z: -5 },
                BlockPos {
                    x: 10,
                    y: 66,
                    z: -5
                }
            )
        );
        assert_eq!(
            b(Facing::E),
            (
                BlockPos {
                    x: 10,
                    y: 64,
                    z: -7
                },
                BlockPos {
                    x: 10,
                    y: 66,
                    z: -5
                }
            )
        );
        assert_eq!(
            b(Facing::W),
            (
                at,
                BlockPos {
                    x: 10,
                    y: 66,
                    z: -3
                }
            )
        );
        assert!(inside(
            b(Facing::S),
            BlockPos {
                x: 11,
                y: 65,
                z: -5
            }
        ));
        assert!(!inside(b(Facing::S), BlockPos { x: 9, y: 65, z: -5 }));
    }
}
