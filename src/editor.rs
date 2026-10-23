//! Virtual circuit editor.
//!
//! Runs in a dedicated pocket dimension (`DIMENSION_ID`, declared in
//! `lib.rs`). Each player gets a lane of their own: a cube the size of the
//! chip's tier on every side (a tier-256 chip: 256³), wrapped in a shell of
//! barriers one block thick — 258³ in all. The shell can't be broken, not
//! even in creative. Past the barriers (seen through them) a second shell
//! shows where the field ends: grey wool, a cyan grid every 8 blocks from
//! the field's corner, sea lanterns where its lines cross.
//!
//! The design comes from a Blueprint in a workbench's blueprint slot and
//! goes back onto it on `/vlsi save` (onto a new Blueprint in the player's
//! inventory if that one was taken out meanwhile). Then the field is
//! removed — its blocks and whatever lies about in it — and the player gets
//! back the inventory and game mode they came in with: the editor is
//! creative, what was made there stays there. A session outlives leaving
//! the game, a crash and the server stopping (it's on disk from the start):
//! back in, the player is still editing. No one stays in the editor
//! dimension without a session.
//!
//! Saving reads the whole cube in one `scan_region` call, with every
//! block's full state (a repeater's delay and facing, …), and what the
//! containers in it hold.

use std::collections::HashMap;
use yog_api::sync::{LazyLock, Mutex};

use yog_api::Schema;
use yog_api::yog_ids::{game, id, ns};
use yog_api::{BlockPos, EventPhase, Registry};

use crate::board;
use crate::cables::{self, RedstonePort};
use crate::chip::{
    Blueprint, ChipMeta, CircuitBlock, CircuitData, Port, SavedAlu, container_slots,
};
use crate::content::Tier;
use crate::content::{Blocks, Items};
use crate::port;
use crate::res::Strings;

/// Id of the pocket dimension the editor runs in — declared via
/// `Registry::register_dimension` in `lib.rs`.
pub const DIMENSION_ID: &str = "yog-vlsi:editor";
/// Id of the (void) chunk generator backing [`DIMENSION_ID`].
pub const GENERATOR_ID: &str = "yog-vlsi:editor_terrain";

/// Y of the cube's bottom layer inside every editor lane.
const EDITOR_Y: i32 = 10;
/// What the shell is made of: transparent, solid, unbreakable here.
const WALL: &str = id!(ns::MINECRAFT, "barrier");
/// The shell seen through the barriers: its wool, its grid and its lights.
const SKIN: &str = id!(ns::MINECRAFT, "light_gray_wool");
const GRID: &str = id!(ns::MINECRAFT, "cyan_wool");
const LIGHT: &str = id!(ns::MINECRAFT, "sea_lantern");
/// The grid's spacing, from the field's corner.
const GRID_EVERY: i32 = 8;
/// Lane 0's corner; each further lane is offset along X.
const BASE_X: i32 = 0;
const BASE_Z: i32 = 0;
/// Spacing between lanes — comfortably larger than the biggest tier (256)
/// plus its walls.
const LANE_SPACING: i32 = 320;
/// Player inventory slots that can take a new Blueprint (main + hotbar).
const PLAYER_SLOTS: u32 = 36;

/// Where an edited design lives: a slot of a block container.
#[derive(Debug, Clone, Schema)]
pub struct Source {
    pub dimension: String,
    pub pos: BlockPos,
    pub slot: u32,
}

#[derive(Schema)]
struct EditorSession {
    name: String,
    tier: Tier,
    origin_x: i32,
    origin_z: i32,
    size: u32,
    source: Source,
    return_dim: String,
    return_pos: (f64, f64, f64),
    /// What the player came in with: their inventory (its full SNBT list,
    /// `Inventory` of their entity data) and game mode — theirs again on the way out.
    inventory: Option<String>,
    gamemode: String,
}

/// Where sessions and lanes are kept between runs.
/// Where they're kept in the world store.
const SESSIONS_KEY: &str = "editor_sessions";
const LANES_KEY: &str = "editor_lanes";
/// The player's entity data key their inventory is under.
const INVENTORY_TAG: &str = "Inventory";

/// Stable per-player lane index, assigned on first use.
static LANES: LazyLock<Mutex<HashMap<String, u32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Where each player last was outside the editor dimension.
/// A dimension and a position in it.
type Place = (String, (f64, f64, f64));
static LAST_OUTSIDE: LazyLock<Mutex<HashMap<String, Place>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static SESSIONS: LazyLock<Mutex<HashMap<String, EditorSession>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lane_for(player_name: &str) -> u32 {
    let mut lanes = LANES.lock();
    let next = lanes.len() as u32;
    *lanes.entry(player_name.to_string()).or_insert(next)
}

/// Writes the sessions and lanes to disk now: a session survives a crash.
fn persist() {
    let Some(store) = yog_api::storage::world() else {
        return;
    };
    store.set(SESSIONS_KEY, &*SESSIONS.lock());
    store.set(LANES_KEY, &*LANES.lock());
    if let Err(e) = store.flush() {
        yog_api::warn!("[yog-vlsi] saving editor sessions: {e}");
    }
}

/// Reads the sessions and lanes back (the server started).
pub fn load() {
    *SESSIONS.lock() = crate::saved::load(SESSIONS_KEY);
    *LANES.lock() = crate::saved::load(LANES_KEY);
}

/// The player's whole inventory, as their entity data has it.
fn inventory_of(player: &str) -> Option<String> {
    let uuid = yog_api::event::player::info(player)?.uuid;
    let nbt = yog_api::event::entity::nbt(&uuid)?;
    let r = crate::nbt::top_value(&nbt, INVENTORY_TAG)?;
    Some(nbt[r].to_string())
}

/// Gives the player back `inventory` (from [`inventory_of`]) — everything
/// else about them as it is now.
fn restore_inventory(player: &str, inventory: &str) -> bool {
    let Some(uuid) = yog_api::event::player::info(player).map(|i| i.uuid) else {
        return false;
    };
    let Some(now) = yog_api::event::entity::nbt(&uuid) else {
        return false;
    };
    yog_api::event::entity::set_nbt(
        &uuid,
        crate::nbt::with_top_value(&now, INVENTORY_TAG, inventory),
    )
}

/// Removes whatever isn't a player from a field (dropped items, mobs, …).
fn clear_entities(origin_x: i32, origin_z: i32, size: i32) {
    let (lo, hi) = interior(origin_x, origin_z, size);
    let min = ((lo.x - 2) as f64, (lo.y - 2) as f64, (lo.z - 2) as f64);
    let max = ((hi.x + 3) as f64, (hi.y + 3) as f64, (hi.z + 3) as f64);
    for uuid in yog_api::event::world::entities_in_box(DIMENSION_ID, min, max) {
        if yog_api::event::entity::info(&uuid).is_some_and(|e| e.type_id != game::PLAYER) {
            yog_api::event::entity::kill(&uuid);
        }
    }
}

/// The cube's corners (inside the shell).
fn interior(origin_x: i32, origin_z: i32, size: i32) -> (BlockPos, BlockPos) {
    (
        BlockPos {
            x: origin_x,
            y: EDITOR_Y,
            z: origin_z,
        },
        BlockPos {
            x: origin_x + size - 1,
            y: EDITOR_Y + size - 1,
            z: origin_z + size - 1,
        },
    )
}

/// The six faces of the shell around the cube `lo`..`hi` — 6 × 258²
/// blocks rather than 258³.
fn shell(lo: BlockPos, hi: BlockPos) -> [(BlockPos, BlockPos); 6] {
    let (x0, y0, z0, x1, y1, z1) = (lo.x - 1, lo.y - 1, lo.z - 1, hi.x + 1, hi.y + 1, hi.z + 1);
    let at = |x, y, z| BlockPos { x, y, z };
    [
        (at(x0, y0, z0), at(x1, y0, z1)),
        (at(x0, y1, z0), at(x1, y1, z1)),
        (at(x0, y0, z0), at(x0, y1, z1)),
        (at(x1, y0, z0), at(x1, y1, z1)),
        (at(x0, y0, z0), at(x1, y1, z0)),
        (at(x0, y0, z1), at(x1, y1, z1)),
    ]
}

/// The shell past the barriers: one block further out than [`shell`].
fn skin(lo: BlockPos, hi: BlockPos) -> [(BlockPos, BlockPos); 6] {
    let grow = |p: BlockPos, d: i32| BlockPos {
        x: p.x + d,
        y: p.y + d,
        z: p.z + d,
    };
    shell(grow(lo, -1), grow(hi, 1))
}

/// Paints one face of the skin: wool, a grid line every [`GRID_EVERY`]
/// blocks from the field's corner `lo`, a lantern where two lines cross.
fn paint_face((from, to): (BlockPos, BlockPos), lo: BlockPos) {
    yog_api::event::world::fill_region(DIMENSION_ID, from, to, SKIN);
    let get = |p: BlockPos, axis: usize| [p.x, p.y, p.z][axis];
    let set = |mut p: BlockPos, axis: usize, v: i32| {
        match axis {
            0 => p.x = v,
            1 => p.y = v,
            _ => p.z = v,
        }
        p
    };
    // The two axes the face spans (the third is flat).
    let span: Vec<usize> = (0..3).filter(|&a| get(from, a) != get(to, a)).collect();
    let on_grid = |axis: usize, v: i32| (v - get(lo, axis)).rem_euclid(GRID_EVERY) == 0;
    for &a in &span {
        for v in get(from, a)..=get(to, a) {
            if on_grid(a, v) {
                yog_api::event::world::fill_region(
                    DIMENSION_ID,
                    set(from, a, v),
                    set(to, a, v),
                    GRID,
                );
            }
        }
    }
    let (a, b) = (span[0], span[1]);
    for u in (get(from, a)..=get(to, a)).filter(|&u| on_grid(a, u)) {
        for v in (get(from, b)..=get(to, b)).filter(|&v| on_grid(b, v)) {
            yog_api::event::block::set_state(DIMENSION_ID, set(set(from, a, u), b, v), LIGHT);
        }
    }
}

/// Wraps the lane's cube in its barrier shell and the skin that shows it,
/// and empties it.
fn build_room(origin_x: i32, origin_z: i32, size: i32) {
    let (lo, hi) = interior(origin_x, origin_z, size);
    for (from, to) in shell(lo, hi) {
        yog_api::event::world::fill_region(DIMENSION_ID, from, to, WALL);
    }
    for face in skin(lo, hi) {
        paint_face(face, lo);
    }
    // What a previous design left inside.
    for yog_api::PlacedBlock { pos, state } in
        yog_api::event::world::scan_region(DIMENSION_ID, lo, hi)
    {
        yog_api::event::block::set_state(DIMENSION_ID, pos, game::AIR);
        let id = yog_api::split_state(&state).0;
        cables::refresh(DIMENSION_ID, pos, id);
        if id == Blocks::REDSTONE_PORT {
            cables::forget_port(DIMENSION_ID, pos);
        }
    }
}

/// Opens `blueprint` (from `source`) in `player_name`'s lane and moves them
/// there; `/vlsi save` takes them back to `return_dim`/`return_pos`.
pub fn enter(
    player_name: &str,
    blueprint: &Blueprint,
    source: Source,
    return_dim: &str,
    return_pos: (f64, f64, f64),
) -> Result<(), String> {
    let lane = lane_for(player_name);
    let origin_x = BASE_X + lane as i32 * LANE_SPACING;
    let origin_z = BASE_Z;
    let size = blueprint.tier.world_size();
    build_room(origin_x, origin_z, size as i32);

    let at = |x: u32, y: u32, z: u32| BlockPos {
        x: origin_x + x as i32,
        y: EDITOR_Y + y as i32,
        z: origin_z + z as i32,
    };
    // What stands on or hangs from another block goes in after it: placed
    // first, the next block set beside it would knock it off.
    let (hanging, standing): (Vec<_>, Vec<_>) = blueprint
        .circuit
        .blocks
        .iter()
        .partition(|b| needs_support(b.id()));
    for block in standing.into_iter().chain(hanging) {
        let pos = at(block.x, block.y, block.z);
        yog_api::event::block::set_state(DIMENSION_ID, pos, &block.state);
        for (slot, item, count) in &block.items {
            yog_api::event::block::set_inventory_slot(
                DIMENSION_ID,
                pos,
                *slot as u32,
                item,
                *count as u32,
                "",
            );
        }
        // A Redstone Port as it was set (an old design's: an unnamed input).
        if block.id() == Blocks::REDSTONE_PORT {
            let port = block
                .redstone_port
                .clone()
                .unwrap_or_else(RedstonePort::new);
            cables::set_redstone_port(DIMENSION_ID, pos, port);
        }
    }
    // Cables and ports placed, Yog Pipes joins them up.
    for block in &blueprint.circuit.blocks {
        cables::refresh(DIMENSION_ID, at(block.x, block.y, block.z), block.id());
    }
    for p in &blueprint.circuit.ports {
        yog_api::event::block::set_state(
            DIMENSION_ID,
            at(p.x, p.y, p.z),
            port::block_for_dir(p.dir),
        );
        port::write_label(DIMENSION_ID, at(p.x, p.y, p.z), &p.label);
    }
    // Its ALUs as they were: their chips in their slots, their boards.
    for a in &blueprint.circuit.alus {
        let pos = at(a.x, a.y, a.z);
        for (slot, meta) in &a.chips {
            yog_api::event::block::set_inventory_slot(
                DIMENSION_ID,
                pos,
                *slot as u32,
                meta.tier.chip(),
                1,
                meta.to_nbt(),
            );
        }
        board::restore(
            board::AluId {
                dimension: DIMENSION_ID.to_string(),
                x: pos.x,
                y: pos.y,
                z: pos.z,
            },
            a.tier,
            &a.wires,
        );
    }

    let (cx, cz) = (
        (origin_x + size as i32 / 2) as f64,
        (origin_z + size as i32 / 2) as f64,
    );
    // What they come in with, theirs again on the way out.
    let inventory = inventory_of(player_name);
    let gamemode = yog_api::event::player::info(player_name)
        .map_or_else(|| "survival".to_string(), |i| i.gamemode.to_string());
    if !yog_api::event::player::teleport_to_dim(
        player_name,
        DIMENSION_ID,
        (cx, (EDITOR_Y + 1) as f64, cz),
    ) {
        return Err(Strings::editor_no_dimension());
    }
    yog_api::event::player::set_gamemode(player_name, "creative");
    SESSIONS.lock().insert(
        player_name.to_string(),
        EditorSession {
            name: blueprint.name.clone(),
            tier: blueprint.tier,
            origin_x,
            origin_z,
            size,
            source,
            return_dim: return_dim.to_string(),
            return_pos,
            inventory,
            gamemode,
        },
    );
    persist();
    yog_api::event::chat::actionbar(
        player_name,
        Strings::editor_editing(&blueprint.name, blueprint.tier.name(), size, size),
    );
    Ok(())
}

/// `/vlsi save`: writes the design back onto its Blueprint, takes the
/// player back and removes their editing field. Returns a status message.
pub fn save(player_name: &str) -> String {
    let Some(session) = SESSIONS.lock().remove(player_name) else {
        return Strings::editor_not_editing();
    };
    let message = finish(player_name, session);
    persist();
    message
}

/// Reads the design off the player's field.
fn scan(session: &EditorSession) -> Blueprint {
    let mut blocks = Vec::new();
    let mut ports = Vec::new();
    let mut alus = Vec::new();
    let (lo, hi) = interior(session.origin_x, session.origin_z, session.size as i32);
    for yog_api::PlacedBlock { pos: p, state } in
        yog_api::event::world::scan_region(DIMENSION_ID, lo, hi)
    {
        let (x, y, z) = (
            (p.x - lo.x) as u32,
            (p.y - lo.y) as u32,
            (p.z - lo.z) as u32,
        );
        let id = yog_api::split_state(&state).0;
        if let Some(dir) = port::dir_for_block(id) {
            let label = port::read_label(DIMENSION_ID, p)
                .unwrap_or_else(|| format!("Port {}", ports.len()));
            ports.push(Port {
                label,
                x,
                y,
                z,
                dir,
            });
            continue;
        }
        // An ALU: its chips and its board go into the design with it.
        if let Some(tier) = Tier::from_alu(id) {
            let chips = (0..board::CHIP_SLOTS)
                .filter_map(|slot| {
                    let nbt =
                        yog_api::event::block::inventory_slot(DIMENSION_ID, p, slot as u32)?.snbt;
                    Some((slot, ChipMeta::from_nbt(&nbt)?))
                })
                .collect();
            let wires = board::nested_wires(&board::AluId {
                dimension: DIMENSION_ID.to_string(),
                x: p.x,
                y: p.y,
                z: p.z,
            });
            alus.push(SavedAlu {
                x,
                y,
                z,
                tier,
                chips,
                wires,
            });
        }
        let items = (0..container_slots(id))
            .filter_map(|slot| {
                let yog_api::Stack { item, count, .. } =
                    yog_api::event::block::inventory_slot(DIMENSION_ID, p, slot as u32)?;
                Some((slot, item.to_string(), count.min(255) as u8))
            })
            .collect();
        let redstone_port =
            (id == Blocks::REDSTONE_PORT).then(|| cables::redstone_port(DIMENSION_ID, p));
        blocks.push(CircuitBlock {
            x,
            y,
            z,
            state: state.to_string(),
            items,
            redstone_port,
        });
    }
    Blueprint {
        name: session.name.clone(),
        tier: session.tier,
        circuit: CircuitData {
            size: session.tier.world_size(),
            blocks,
            ports,
            alus,
        },
    }
}

/// Saves the session's design and ends it: the player's inventory and game
/// mode back, the design onto its Blueprint, onto a new one in their
/// inventory, or — when neither works — dropped as a Blueprint where they
/// go back to. Then the field goes, and they go back.
fn finish(player_name: &str, session: EditorSession) -> String {
    let blueprint = scan(&session);
    let summary = Strings::editor_summary(
        blueprint.circuit.ports.len(),
        blueprint.circuit.blocks.len(),
    );
    let nbt = blueprint.to_nbt();
    let src = &session.source;
    let (rx, ry, rz) = session.return_pos;

    // Their own things back before a new Blueprint goes among them.
    if let Some(inventory) = &session.inventory
        && !restore_inventory(player_name, inventory)
    {
        yog_api::warn!("[yog-vlsi] couldn't give {player_name} back their inventory");
    }
    yog_api::event::player::set_gamemode(player_name, &session.gamemode);

    let still_there = yog_api::event::block::inventory_slot(&src.dimension, src.pos, src.slot)
        .is_some_and(|s| s.item.as_str() == Items::BLUEPRINT);
    let message = if still_there
        && yog_api::event::block::set_inventory_slot(
            &src.dimension,
            src.pos,
            src.slot,
            Items::BLUEPRINT,
            1,
            &nbt,
        ) {
        Strings::editor_saved(&blueprint.name, &summary)
    } else {
        let taken: Vec<u32> = yog_api::event::player::inventory(player_name)
            .into_iter()
            .map(|s| s.slot)
            .collect();
        let free = (0..PLAYER_SLOTS).find(|s| !taken.contains(s));
        if free.is_some_and(|slot| {
            yog_api::event::player::set_slot_item(player_name, slot, Items::BLUEPRINT, 1, &nbt)
        }) {
            Strings::editor_saved_new(&blueprint.name, &summary)
        } else {
            yog_api::event::item::spawn_item(
                &session.return_dim,
                (rx, ry, rz),
                (0.0, 0.0, 0.0),
                Items::BLUEPRINT,
                1,
                &nbt,
            );
            Strings::editor_saved_dropped(&blueprint.name, &summary)
        }
    };

    // The field goes — and what its blocks dropped going — while it's
    // still loaded around the player.
    remove_room(session.origin_x, session.origin_z, session.size as i32);
    clear_entities(session.origin_x, session.origin_z, session.size as i32);
    yog_api::event::player::teleport_to_dim(player_name, &session.return_dim, (rx, ry, rz));
    message
}

/// Removes a field: what's in it, and its shell.
fn remove_room(origin_x: i32, origin_z: i32, size: i32) {
    let (lo, hi) = interior(origin_x, origin_z, size);
    for yog_api::PlacedBlock { pos, .. } in yog_api::event::world::scan_region(DIMENSION_ID, lo, hi)
    {
        yog_api::event::block::set_state(DIMENSION_ID, pos, game::AIR);
    }
    for (from, to) in shell(lo, hi).into_iter().chain(skin(lo, hi)) {
        yog_api::event::world::fill_region(DIMENSION_ID, from, to, game::AIR);
    }
    board::forget_in(DIMENSION_ID, lo, hi);
}

/// Keeps every session for the next run (the server stops).
pub fn save_sessions() {
    persist();
}

/// Blocks that break without the block they stand on or hang from.
fn needs_support(id: &str) -> bool {
    const PARTS: [&str; 16] = [
        "torch",
        "button",
        "lever",
        "repeater",
        "comparator",
        "redstone_wire",
        "rail",
        "pressure_plate",
        "door",
        "carpet",
        "ladder",
        "sign",
        "banner",
        "tripwire",
        "lantern",
        "vine",
    ];
    let path = yog_api::yog_ids::game_path(id).unwrap_or(id);
    PARTS.iter().any(|p| path.contains(p))
}

/// Whether a block in the editor dimension is part of a lane's shell.
fn is_structure(block_id: &str) -> bool {
    block_id == WALL
}

pub fn register(registry: &mut Registry) {
    // `/vlsi save` — save the active editor session and return.
    registry.on_typed_command("vlsi", "word", |ctx| {
        if ctx.arg_str(0).unwrap_or("") != "save" {
            return None;
        }
        Some(save(&ctx.source))
    });

    // Nobody stays in the editor dimension without a session (a warp, a
    // teleport, a rejoin): back to where they last were outside it.
    registry.on::<yog_api::event::player::action::Move, _>(|e, cx| {
        let phase = cx.phase();
        if phase != EventPhase::Pre {
            return true;
        }
        if e.dimension != DIMENSION_ID {
            LAST_OUTSIDE.lock().insert(
                e.player_name.clone(),
                (e.dimension.clone(), (e.x, e.y, e.z)),
            );
            return true;
        }
        if SESSIONS.lock().contains_key(&e.player_name) {
            return true;
        }
        let (dim, (x, y, z)) = LAST_OUTSIDE
            .lock()
            .get(&e.player_name)
            .cloned()
            .unwrap_or_else(|| (game::OVERWORLD.into(), (0.5, 100.0, 0.5)));
        yog_api::event::player::teleport_to_dim(&e.player_name, &dim, (x, y, z));
        yog_api::event::chat::actionbar(&e.player_name, Strings::editor_workbench_only());
        false
    });

    // The shell stays — even for creative players.
    registry.on::<yog_api::event::block::Break, _>(|e, cx| {
        let phase = cx.phase();
        !(phase == EventPhase::Pre && e.dimension == DIMENSION_ID && is_structure(&e.block_id))
    });
}
