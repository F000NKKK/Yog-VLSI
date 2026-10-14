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
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use yog_api::{BlockPos, EventPhase, Registry, Storage};

use crate::board;
use crate::chip::{Blueprint, ChipMeta, CircuitBlock, CircuitData, Port, SavedAlu, container_slots};
use crate::cables::{self, RedstonePort};
use crate::content::{Blocks, Items};
use crate::port;
use crate::tier::Tier;

/// Id of the pocket dimension the editor runs in — declared via
/// `Registry::register_dimension` in `lib.rs`.
pub const DIMENSION_ID: &str = "yog-vlsi:editor";
/// Id of the (void) chunk generator backing [`DIMENSION_ID`].
pub const GENERATOR_ID: &str = "yog-vlsi:editor_terrain";

/// Y of the cube's bottom layer inside every editor lane.
const EDITOR_Y: i32 = 10;
/// What the shell is made of: transparent, solid, unbreakable here.
const WALL: &str = "minecraft:barrier";
/// The shell seen through the barriers: its wool, its grid and its lights.
const SKIN: &str = "minecraft:light_gray_wool";
const GRID: &str = "minecraft:cyan_wool";
const LIGHT: &str = "minecraft:sea_lantern";
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub dimension: String,
    #[serde(with = "pos_serde")]
    pub pos: BlockPos,
    pub slot: u32,
}

mod pos_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use yog_api::BlockPos;

    pub fn serialize<S: Serializer>(p: &BlockPos, s: S) -> Result<S::Ok, S::Error> {
        (p.x, p.y, p.z).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BlockPos, D::Error> {
        let (x, y, z) = <(i32, i32, i32)>::deserialize(d)?;
        Ok(BlockPos { x, y, z })
    }
}

#[derive(Serialize, Deserialize)]
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
const STORE_FILE: &str = "yog-vlsi/editor";
const SESSIONS_KEY: &str = "sessions";
const LANES_KEY: &str = "lanes";
/// The player's entity data key their inventory is under.
const INVENTORY_TAG: &str = "Inventory";

/// Stable per-player lane index, assigned on first use.
static LANES: LazyLock<Mutex<HashMap<String, u32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Where each player last was outside the editor dimension.
static LAST_OUTSIDE: LazyLock<Mutex<HashMap<String, (String, (f64, f64, f64))>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static SESSIONS: LazyLock<Mutex<HashMap<String, EditorSession>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn lane_for(player_name: &str) -> u32 {
    let mut lanes = LANES.lock().unwrap();
    let next = lanes.len() as u32;
    *lanes.entry(player_name.to_string()).or_insert(next)
}

/// Writes the sessions and lanes to disk: a session survives a crash.
fn persist(srv: &dyn yog_api::Server) {
    let mut store = Storage::open(&srv.game_dir(), STORE_FILE);
    if let Ok(json) = serde_json::to_string(&*SESSIONS.lock().unwrap()) {
        store.set(SESSIONS_KEY, &*json);
    }
    if let Ok(json) = serde_json::to_string(&*LANES.lock().unwrap()) {
        store.set(LANES_KEY, &*json);
    }
    if let Err(e) = store.flush() {
        yog_api::warn!("[yog-vlsi] saving editor sessions: {e}");
    }
}

/// Reads the sessions and lanes back (the server started).
pub fn load(srv: &dyn yog_api::Server) {
    let store = Storage::open(&srv.game_dir(), STORE_FILE);
    let read = |key: &str| store.get(key).and_then(|v| v.as_str().map(str::to_string));
    *SESSIONS.lock().unwrap() = read(SESSIONS_KEY).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
    *LANES.lock().unwrap() = read(LANES_KEY).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
}

/// The player's whole inventory, as their entity data has it.
fn inventory_of(srv: &dyn yog_api::Server, player: &str) -> Option<String> {
    let uuid = srv.player_info(player)?.uuid;
    let nbt = srv.entity_get_nbt(&uuid)?;
    let r = crate::nbt::top_value(&nbt, INVENTORY_TAG)?;
    Some(nbt[r].to_string())
}

/// Gives the player back `inventory` (from [`inventory_of`]) — everything
/// else about them as it is now.
fn restore_inventory(srv: &dyn yog_api::Server, player: &str, inventory: &str) -> bool {
    let Some(uuid) = srv.player_info(player).map(|i| i.uuid) else { return false };
    let Some(now) = srv.entity_get_nbt(&uuid) else { return false };
    srv.entity_set_nbt(&uuid, &crate::nbt::with_top_value(&now, INVENTORY_TAG, inventory))
}

/// Removes whatever isn't a player from a field (dropped items, mobs, …).
fn clear_entities(srv: &dyn yog_api::Server, origin_x: i32, origin_z: i32, size: i32) {
    let (lo, hi) = interior(origin_x, origin_z, size);
    let min = ((lo.x - 2) as f64, (lo.y - 2) as f64, (lo.z - 2) as f64);
    let max = ((hi.x + 3) as f64, (hi.y + 3) as f64, (hi.z + 3) as f64);
    for uuid in srv.entities_in_box(DIMENSION_ID, min, max) {
        if srv.entity_info(&uuid).is_some_and(|e| e.type_id != "minecraft:player") {
            srv.entity_kill(&uuid);
        }
    }
}

/// The cube's corners (inside the shell).
fn interior(origin_x: i32, origin_z: i32, size: i32) -> (BlockPos, BlockPos) {
    (
        BlockPos { x: origin_x, y: EDITOR_Y, z: origin_z },
        BlockPos { x: origin_x + size - 1, y: EDITOR_Y + size - 1, z: origin_z + size - 1 },
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
    let grow = |p: BlockPos, d: i32| BlockPos { x: p.x + d, y: p.y + d, z: p.z + d };
    shell(grow(lo, -1), grow(hi, 1))
}

/// Paints one face of the skin: wool, a grid line every [`GRID_EVERY`]
/// blocks from the field's corner `lo`, a lantern where two lines cross.
fn paint_face(srv: &dyn yog_api::Server, (from, to): (BlockPos, BlockPos), lo: BlockPos) {
    srv.fill_region(DIMENSION_ID, from, to, SKIN);
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
                srv.fill_region(DIMENSION_ID, set(from, a, v), set(to, a, v), GRID);
            }
        }
    }
    let (a, b) = (span[0], span[1]);
    for u in (get(from, a)..=get(to, a)).filter(|&u| on_grid(a, u)) {
        for v in (get(from, b)..=get(to, b)).filter(|&v| on_grid(b, v)) {
            srv.set_block_state(DIMENSION_ID, set(set(from, a, u), b, v), LIGHT);
        }
    }
}

/// Wraps the lane's cube in its barrier shell and the skin that shows it,
/// and empties it.
fn build_room(srv: &dyn yog_api::Server, origin_x: i32, origin_z: i32, size: i32) {
    let (lo, hi) = interior(origin_x, origin_z, size);
    for (from, to) in shell(lo, hi) {
        srv.fill_region(DIMENSION_ID, from, to, WALL);
    }
    for face in skin(lo, hi) {
        paint_face(srv, face, lo);
    }
    // What a previous design left inside.
    for (pos, state) in srv.scan_region(DIMENSION_ID, lo, hi) {
        srv.set_block_state(DIMENSION_ID, pos, "minecraft:air");
        let id = state.split('[').next().unwrap_or(&state);
        cables::refresh(DIMENSION_ID, pos, id);
        if id == Blocks::REDSTONE_PORT {
            cables::forget_port(DIMENSION_ID, pos);
        }
    }
}

/// Opens `blueprint` (from `source`) in `player_name`'s lane and moves them
/// there; `/vlsi save` takes them back to `return_dim`/`return_pos`.
pub fn enter(
    srv: &dyn yog_api::Server,
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
    build_room(srv, origin_x, origin_z, size as i32);

    let at = |x: u32, y: u32, z: u32| BlockPos { x: origin_x + x as i32, y: EDITOR_Y + y as i32, z: origin_z + z as i32 };
    // What stands on or hangs from another block goes in after it: placed
    // first, the next block set beside it would knock it off.
    let (hanging, standing): (Vec<_>, Vec<_>) = blueprint.circuit.blocks.iter().partition(|b| needs_support(b.id()));
    for block in standing.into_iter().chain(hanging) {
        let pos = at(block.x, block.y, block.z);
        srv.set_block_state(DIMENSION_ID, pos, &block.state);
        for (slot, item, count) in &block.items {
            srv.set_inventory_slot(DIMENSION_ID, pos, *slot as u32, item, *count as u32, "");
        }
        // A Redstone Port as it was set (an old design's: an unnamed input).
        if block.id() == Blocks::REDSTONE_PORT {
            let port = block.redstone_port.clone().unwrap_or(RedstonePort { output: false, name: String::new() });
            cables::set_redstone_port(srv, DIMENSION_ID, pos, port);
        }
    }
    // Cables and ports placed, Yog Pipes joins them up.
    for block in &blueprint.circuit.blocks {
        cables::refresh(DIMENSION_ID, at(block.x, block.y, block.z), block.id());
    }
    for p in &blueprint.circuit.ports {
        srv.set_block_state(DIMENSION_ID, at(p.x, p.y, p.z), port::block_for_dir(p.dir));
        port::write_label(srv, DIMENSION_ID, at(p.x, p.y, p.z), &p.label);
    }
    // Its ALUs as they were: their chips in their slots, their boards.
    for a in &blueprint.circuit.alus {
        let pos = at(a.x, a.y, a.z);
        for (slot, meta) in &a.chips {
            srv.set_inventory_slot(DIMENSION_ID, pos, *slot as u32, meta.tier.chip_item(), 1, &meta.to_nbt());
        }
        board::restore(board::AluId { dimension: DIMENSION_ID.to_string(), x: pos.x, y: pos.y, z: pos.z }, a.tier, &a.wires);
    }

    let (cx, cz) = ((origin_x + size as i32 / 2) as f64, (origin_z + size as i32 / 2) as f64);
    // What they come in with, theirs again on the way out.
    let inventory = inventory_of(srv, player_name);
    let gamemode = srv.player_info(player_name).map_or_else(|| "survival".to_string(), |i| i.gamemode);
    if !srv.teleport_to_dim(player_name, DIMENSION_ID, cx, (EDITOR_Y + 1) as f64, cz) {
        return Err("§cCouldn't enter the editor dimension — see the game log.".into());
    }
    srv.set_gamemode(player_name, "creative");
    SESSIONS.lock().unwrap().insert(
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
    persist(srv);
    srv.send_actionbar(
        player_name,
        &format!(
            "§6Editing '{}' — {} tier, {}×{}. §7/vlsi save when done.",
            blueprint.name,
            blueprint.tier.name(),
            size,
            size
        ),
    );
    Ok(())
}

/// `/vlsi save`: writes the design back onto its Blueprint, takes the
/// player back and removes their editing field. Returns a status message.
pub fn save(srv: &dyn yog_api::Server, player_name: &str) -> String {
    let Some(session) = SESSIONS.lock().unwrap().remove(player_name) else {
        return "§cYou are not editing a VLSI circuit right now.".into();
    };
    let message = finish(srv, player_name, session);
    persist(srv);
    message
}

/// Reads the design off the player's field.
fn scan(srv: &dyn yog_api::Server, session: &EditorSession) -> Blueprint {
    let mut blocks = Vec::new();
    let mut ports = Vec::new();
    let mut alus = Vec::new();
    let (lo, hi) = interior(session.origin_x, session.origin_z, session.size as i32);
    for (p, state) in srv.scan_region(DIMENSION_ID, lo, hi) {
        let (x, y, z) = ((p.x - lo.x) as u32, (p.y - lo.y) as u32, (p.z - lo.z) as u32);
        let id = state.split('[').next().unwrap_or(&state);
        if let Some(dir) = port::dir_for_block(id) {
            let label = port::read_label(srv, DIMENSION_ID, p).unwrap_or_else(|| format!("Port {}", ports.len()));
            ports.push(Port { label, x, y, z, dir });
            continue;
        }
        // An ALU: its chips and its board go into the design with it.
        if let Some(tier) = Tier::from_alu_block(id) {
            let chips = (0..board::CHIP_SLOTS)
                .filter_map(|slot| {
                    let (_, _, nbt) = srv.get_inventory_slot(DIMENSION_ID, p, slot as u32)?;
                    Some((slot, ChipMeta::from_nbt(&nbt)?))
                })
                .collect();
            let wires = board::nested_wires(&board::AluId { dimension: DIMENSION_ID.to_string(), x: p.x, y: p.y, z: p.z });
            alus.push(SavedAlu { x, y, z, tier, chips, wires });
        }
        let items = (0..container_slots(id))
            .filter_map(|slot| {
                let (item, count, _) = srv.get_inventory_slot(DIMENSION_ID, p, slot as u32)?;
                Some((slot, item, count.min(255) as u8))
            })
            .collect();
        let redstone_port = (id == Blocks::REDSTONE_PORT).then(|| cables::redstone_port(DIMENSION_ID, p));
        blocks.push(CircuitBlock { x, y, z, state, items, redstone_port });
    }
    Blueprint {
        name: session.name.clone(),
        tier: session.tier,
        circuit: CircuitData { size: session.tier.world_size(), blocks, ports, alus },
    }
}

/// Saves the session's design and ends it: the player's inventory and game
/// mode back, the design onto its Blueprint, onto a new one in their
/// inventory, or — when neither works — dropped as a Blueprint where they
/// go back to. Then the field goes, and they go back.
fn finish(srv: &dyn yog_api::Server, player_name: &str, session: EditorSession) -> String {
    let blueprint = scan(srv, &session);
    let summary = format!("{} ports, {} blocks", blueprint.circuit.ports.len(), blueprint.circuit.blocks.len());
    let nbt = blueprint.to_nbt();
    let src = &session.source;
    let (rx, ry, rz) = session.return_pos;

    // Their own things back before a new Blueprint goes among them.
    if let Some(inventory) = &session.inventory
        && !restore_inventory(srv, player_name, inventory)
    {
        yog_api::warn!("[yog-vlsi] couldn't give {player_name} back their inventory");
    }
    srv.set_gamemode(player_name, &session.gamemode);

    let still_there = srv
        .get_inventory_slot(&src.dimension, src.pos, src.slot)
        .is_some_and(|(id, _, _)| id == Items::BLUEPRINT);
    let message = if still_there && srv.set_inventory_slot(&src.dimension, src.pos, src.slot, Items::BLUEPRINT, 1, &nbt) {
        format!("§aSaved '{}' to its Blueprint — {summary}.", blueprint.name)
    } else {
        let taken: Vec<u32> = srv.player_inventory(player_name).into_iter().map(|(slot, ..)| slot).collect();
        let free = (0..PLAYER_SLOTS).find(|s| !taken.contains(s));
        if free.is_some_and(|slot| srv.set_slot_item(player_name, slot, Items::BLUEPRINT, 1, &nbt)) {
            format!("§aSaved '{}' to a new Blueprint in your inventory — {summary}.", blueprint.name)
        } else {
            srv.spawn_item(&session.return_dim, (rx, ry, rz), (0.0, 0.0, 0.0), Items::BLUEPRINT, 1, &nbt);
            format!("§eSaved '{}' to a Blueprint dropped where you're back — {summary}.", blueprint.name)
        }
    };

    // The field goes — and what its blocks dropped going — while it's
    // still loaded around the player.
    remove_room(srv, session.origin_x, session.origin_z, session.size as i32);
    clear_entities(srv, session.origin_x, session.origin_z, session.size as i32);
    srv.teleport_to_dim(player_name, &session.return_dim, rx, ry, rz);
    message
}

/// Removes a field: what's in it, and its shell.
fn remove_room(srv: &dyn yog_api::Server, origin_x: i32, origin_z: i32, size: i32) {
    let (lo, hi) = interior(origin_x, origin_z, size);
    for (pos, _) in srv.scan_region(DIMENSION_ID, lo, hi) {
        srv.set_block_state(DIMENSION_ID, pos, "minecraft:air");
    }
    for (from, to) in shell(lo, hi).into_iter().chain(skin(lo, hi)) {
        srv.fill_region(DIMENSION_ID, from, to, "minecraft:air");
    }
    board::forget_in(DIMENSION_ID, lo, hi);
}

/// Keeps every session for the next run (the server stops).
pub fn save_sessions(srv: &dyn yog_api::Server) {
    persist(srv);
}

/// Blocks that break without the block they stand on or hang from.
fn needs_support(id: &str) -> bool {
    const PARTS: [&str; 16] = [
        "torch", "button", "lever", "repeater", "comparator", "redstone_wire", "rail", "pressure_plate", "door",
        "carpet", "ladder", "sign", "banner", "tripwire", "lantern", "vine",
    ];
    let path = id.strip_prefix("minecraft:").unwrap_or(id);
    PARTS.iter().any(|p| path.contains(p))
}

/// Whether a block in the editor dimension is part of a lane's shell.
fn is_structure(block_id: &str) -> bool {
    block_id == WALL
}

pub fn register(registry: &mut Registry) {
    // `/vlsi save` — save the active editor session and return.
    registry.on_typed_command("vlsi", "word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "save" {
            return None;
        }
        Some(save(srv, &ctx.source))
    });

    // Nobody stays in the editor dimension without a session (a warp, a
    // teleport, a rejoin): back to where they last were outside it.
    registry.on_player_move(|e, phase, srv| {
        if phase != EventPhase::Pre {
            return true;
        }
        if e.dimension != DIMENSION_ID {
            LAST_OUTSIDE.lock().unwrap().insert(e.player_name.clone(), (e.dimension.clone(), (e.x, e.y, e.z)));
            return true;
        }
        if SESSIONS.lock().unwrap().contains_key(&e.player_name) {
            return true;
        }
        let (dim, (x, y, z)) = LAST_OUTSIDE
            .lock()
            .unwrap()
            .get(&e.player_name)
            .cloned()
            .unwrap_or_else(|| ("minecraft:overworld".into(), (0.5, 100.0, 0.5)));
        srv.teleport_to_dim(&e.player_name, &dim, x, y, z);
        srv.send_actionbar(&e.player_name, "§cThe circuit editor is only open from a VLSI Workbench.");
        false
    });

    // The shell stays — even for creative players.
    registry.on_block_break(|e, phase, _srv| {
        !(phase == EventPhase::Pre && e.dimension == DIMENSION_ID && is_structure(&e.block_id))
    });
}
