//! Virtual circuit editor.
//!
//! Runs in a dedicated pocket dimension (`DIMENSION_ID`, declared in
//! `lib.rs`). Each player gets a lane of their own: a cube the size of the
//! chip's tier on every side (a tier-256 chip: 256³), wrapped in a shell of
//! barriers one block thick — 258³ in all. The shell can't be broken, not
//! even in creative.
//!
//! The design comes from a Blueprint in a workbench's blueprint slot and
//! goes back onto it on `/vlsi save` (onto a new Blueprint in the player's
//! inventory if that one was taken out meanwhile).
//!
//! Saving reads the whole cube in one `scan_region` call, with every
//! block's full state (a repeater's delay and facing, …), and what the
//! containers in it hold.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::{BlockPos, EventPhase, Registry};

use crate::chip::{Blueprint, CircuitBlock, CircuitData, Port, container_slots};
use crate::content::Items;
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
/// Lane 0's corner; each further lane is offset along X.
const BASE_X: i32 = 0;
const BASE_Z: i32 = 0;
/// Spacing between lanes — comfortably larger than the biggest tier (256)
/// plus its walls.
const LANE_SPACING: i32 = 320;
/// Player inventory slots that can take a new Blueprint (main + hotbar).
const PLAYER_SLOTS: u32 = 36;

/// Where an edited design lives: a slot of a block container.
#[derive(Debug, Clone)]
pub struct Source {
    pub dimension: String,
    pub pos: BlockPos,
    pub slot: u32,
}

struct EditorSession {
    name: String,
    tier: Tier,
    origin_x: i32,
    origin_z: i32,
    size: u32,
    source: Source,
    return_dim: String,
    return_pos: (f64, f64, f64),
}

/// Stable per-player lane index, assigned on first use.
static LANES: LazyLock<Mutex<HashMap<String, u32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static SESSIONS: LazyLock<Mutex<HashMap<String, EditorSession>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn lane_for(player_name: &str) -> u32 {
    let mut lanes = LANES.lock().unwrap();
    let next = lanes.len() as u32;
    *lanes.entry(player_name.to_string()).or_insert(next)
}

/// The cube's corners (inside the shell).
fn interior(origin_x: i32, origin_z: i32, size: i32) -> (BlockPos, BlockPos) {
    (
        BlockPos { x: origin_x, y: EDITOR_Y, z: origin_z },
        BlockPos { x: origin_x + size - 1, y: EDITOR_Y + size - 1, z: origin_z + size - 1 },
    )
}

/// Wraps the lane's cube in its barrier shell and empties it.
fn build_room(srv: &dyn yog_api::Server, origin_x: i32, origin_z: i32, size: i32) {
    let (lo, hi) = interior(origin_x, origin_z, size);
    let (x0, y0, z0, x1, y1, z1) = (lo.x - 1, lo.y - 1, lo.z - 1, hi.x + 1, hi.y + 1, hi.z + 1);
    let at = |x, y, z| BlockPos { x, y, z };
    // Six faces, not the whole cube: 6 × 258² blocks rather than 258³.
    for (from, to) in [
        (at(x0, y0, z0), at(x1, y0, z1)),
        (at(x0, y1, z0), at(x1, y1, z1)),
        (at(x0, y0, z0), at(x0, y1, z1)),
        (at(x1, y0, z0), at(x1, y1, z1)),
        (at(x0, y0, z0), at(x1, y1, z0)),
        (at(x0, y0, z1), at(x1, y1, z1)),
    ] {
        srv.fill_region(DIMENSION_ID, from, to, WALL);
    }
    // What a previous design left inside.
    for (pos, _) in srv.scan_region(DIMENSION_ID, lo, hi) {
        srv.set_block_state(DIMENSION_ID, pos, "minecraft:air");
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
    }
    for p in &blueprint.circuit.ports {
        srv.set_block_state(DIMENSION_ID, at(p.x, p.y, p.z), port::block_for_dir(p.dir));
        port::write_label(srv, DIMENSION_ID, at(p.x, p.y, p.z), &p.label);
    }

    let (cx, cz) = ((origin_x + size as i32 / 2) as f64, (origin_z + size as i32 / 2) as f64);
    if !srv.teleport_to_dim(player_name, DIMENSION_ID, cx, (EDITOR_Y + 1) as f64, cz) {
        return Err("§cCouldn't enter the editor dimension — see the game log.".into());
    }
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
        },
    );
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

/// Scans the player's platform, writes the design back onto its Blueprint
/// and takes them back. Returns a status message.
pub fn save(srv: &dyn yog_api::Server, player_name: &str) -> String {
    let Some(session) = SESSIONS.lock().unwrap().remove(player_name) else {
        return "§cYou are not editing a VLSI circuit right now.".into();
    };

    let mut blocks = Vec::new();
    let mut ports = Vec::new();
    let (lo, hi) = interior(session.origin_x, session.origin_z, session.size as i32);
    for (p, state) in srv.scan_region(DIMENSION_ID, lo, hi) {
        let (x, y, z) = ((p.x - lo.x) as u32, (p.y - lo.y) as u32, (p.z - lo.z) as u32);
        let id = state.split('[').next().unwrap_or(&state);
        if let Some(dir) = port::dir_for_block(id) {
            let label = port::read_label(srv, DIMENSION_ID, p).unwrap_or_else(|| format!("Port {}", ports.len()));
            ports.push(Port { label, x, y, z, dir });
            continue;
        }
        let items = (0..container_slots(id))
            .filter_map(|slot| {
                let (item, count, _) = srv.get_inventory_slot(DIMENSION_ID, p, slot as u32)?;
                Some((slot, item, count.min(255) as u8))
            })
            .collect();
        blocks.push(CircuitBlock { x, y, z, state, items });
    }
    let blueprint = Blueprint {
        name: session.name.clone(),
        tier: session.tier,
        circuit: CircuitData { size: session.tier.world_size(), blocks, ports },
    };
    let summary = format!(
        "{} ports, {} blocks",
        blueprint.circuit.ports.len(),
        blueprint.circuit.blocks.len()
    );

    // Back onto the Blueprint it came from — or a new one, if that's gone.
    let src = &session.source;
    let still_there = srv
        .get_inventory_slot(&src.dimension, src.pos, src.slot)
        .is_some_and(|(id, _, _)| id == Items::BLUEPRINT);
    let written = if still_there {
        srv.set_inventory_slot(&src.dimension, src.pos, src.slot, Items::BLUEPRINT, 1, &blueprint.to_nbt())
    } else {
        let taken: Vec<u32> = srv.player_inventory(player_name).into_iter().map(|(slot, ..)| slot).collect();
        match (0..PLAYER_SLOTS).find(|s| !taken.contains(s)) {
            Some(slot) => srv.set_slot_item(player_name, slot, Items::BLUEPRINT, 1, &blueprint.to_nbt()),
            None => false,
        }
    };
    if !written {
        SESSIONS.lock().unwrap().insert(player_name.to_string(), session);
        return "§cNowhere to write the design: put its Blueprint back in the workbench or free an inventory slot, then /vlsi save again.".into();
    }

    let (rx, ry, rz) = session.return_pos;
    srv.teleport_to_dim(player_name, &session.return_dim, rx, ry, rz);
    if still_there {
        format!("§aSaved '{}' to its Blueprint — {summary}.", blueprint.name)
    } else {
        format!("§aSaved '{}' to a new Blueprint in your inventory — {summary}.", blueprint.name)
    }
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

    // The shell stays — even for creative players.
    registry.on_block_break(|e, phase, _srv| {
        !(phase == EventPhase::Pre && e.dimension == DIMENSION_ID && is_structure(&e.block_id))
    });
}
