//! Virtual circuit editor.
//!
//! Runs in a dedicated pocket dimension (`DIMENSION_ID`, declared in
//! `lib.rs`). Each player gets a lane of their own: a flat build platform
//! (y = EDITOR_Y) the size of the chip's tier, inside a closed box of
//! barriers. The box and the floor can't be broken — not even in creative.
//!
//! The design comes from a Blueprint in a workbench's blueprint slot and
//! goes back onto it on `/vlsi save` (onto a new Blueprint in the player's
//! inventory if that one was taken out meanwhile).
//!
//! Block state (facing, delay, powered, …) can't be read back from the
//! world yet — the loader's `get_block` only returns the id — so saving
//! captures block identity only (a loader limitation, not a bug here).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::{BlockPos, EventPhase, Registry, World};

use crate::chip::{Blueprint, CircuitBlock, CircuitData, Port};
use crate::content::Items;
use crate::port;
use crate::tier::Tier;

/// Id of the pocket dimension the editor runs in — declared via
/// `Registry::register_dimension` in `lib.rs`.
pub const DIMENSION_ID: &str = "yog-vlsi:editor";
/// Id of the (void) chunk generator backing [`DIMENSION_ID`].
pub const GENERATOR_ID: &str = "yog-vlsi:editor_terrain";

/// Y level of the build platform inside every editor lane.
const EDITOR_Y: i32 = 10;
/// Height of the box's room above the platform.
const ROOM_HEIGHT: i32 = 8;
/// What the floor and the box are made of.
const FLOOR: &str = "minecraft:bedrock";
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

/// Lays the lane's floor and its barrier box, and clears the platform.
fn build_room(world: &World, origin_x: i32, origin_z: i32, size: i32) {
    let top = EDITOR_Y + ROOM_HEIGHT;
    for x in -1..=size {
        for z in -1..=size {
            let (wx, wz) = (origin_x + x, origin_z + z);
            let edge = x == -1 || z == -1 || x == size || z == size;
            world.set_block(BlockPos { x: wx, y: EDITOR_Y - 1, z: wz }, FLOOR);
            world.set_block(BlockPos { x: wx, y: top, z: wz }, WALL);
            for y in EDITOR_Y..top {
                let block = if edge {
                    WALL
                } else if y == EDITOR_Y {
                    "minecraft:air"
                } else {
                    continue;
                };
                world.set_block(BlockPos { x: wx, y, z: wz }, block);
            }
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
    let world = World::new(srv, DIMENSION_ID);
    build_room(&world, origin_x, origin_z, size as i32);

    let at = |x: u32, z: u32| BlockPos { x: origin_x + x as i32, y: EDITOR_Y, z: origin_z + z as i32 };
    for block in &blueprint.circuit.blocks {
        world.set_block(at(block.x, block.z), block.id());
    }
    for p in &blueprint.circuit.ports {
        world.set_block(at(p.x, p.z), port::block_for_dir(p.dir));
        port::write_label(srv, DIMENSION_ID, at(p.x, p.z), &p.label);
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

    let world = World::new(srv, DIMENSION_ID);
    let mut blocks = Vec::new();
    let mut ports = Vec::new();
    for x in 0..session.size {
        for z in 0..session.size {
            let p = BlockPos { x: session.origin_x + x as i32, y: EDITOR_Y, z: session.origin_z + z as i32 };
            let Some(block_id) = world.get_block(p) else { continue };
            if block_id == "minecraft:air" || block_id == WALL {
                continue;
            }
            if let Some(dir) = port::dir_for_block(&block_id) {
                let label = port::read_label(srv, DIMENSION_ID, p).unwrap_or_else(|| format!("Port {}", ports.len()));
                ports.push(Port { label, x, y: 0, z, dir });
                continue;
            }
            blocks.push(CircuitBlock { x, y: 0, z, state: block_id });
        }
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

/// Whether a block in the editor dimension is part of a lane's box or floor.
fn is_structure(block_id: &str, pos: BlockPos) -> bool {
    block_id == WALL || (block_id == FLOOR && pos.y == EDITOR_Y - 1)
}

pub fn register(registry: &mut Registry) {
    // `/vlsi save` — save the active editor session and return.
    registry.on_typed_command("vlsi", "word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "save" {
            return None;
        }
        Some(save(srv, &ctx.source))
    });

    // The box and the floor stay — even for creative players.
    registry.on_block_break(|e, phase, _srv| {
        !(phase == EventPhase::Pre && e.dimension == DIMENSION_ID && is_structure(&e.block_id, e.pos))
    });
}
