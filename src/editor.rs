//! Virtual circuit editor.
//!
//! The loader has no primitive for spinning up a per-player temporary
//! dimension, so this reuses a reserved region of the Overworld far from
//! spawn instead: one lane per player, allocated on first use and reused on
//! every subsequent edit. Each lane holds a single flat (y = EDITOR_Y) build
//! platform sized to the chip's tier — matching how `vm::RedstoneVM` and the
//! existing test-chip factory already only ever populate the y = 0 layer.
//! Multi-layer (3D) circuits are a follow-up once the loader exposes a
//! bulk-region primitive; scanning/clearing a full `size^3` cube one
//! `set_block` call at a time is not practical for the larger tiers today.
//!
//! Block state (facing, delay, powered, …) cannot be read back from the
//! world — the loader's `get_block` only returns the registry id — so
//! `/vlsi save` captures block identity only. Directional pieces (repeaters,
//! pistons, …) come back at their default orientation and may need a
//! re-rotate after import; this is a documented loader limitation, not a bug
//! here.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::player::Player;
use yog_api::{BlockPos, Registry, World};

fn player_pos(srv: &dyn yog_api::Server, name: &str, uuid: &str) -> (f64, f64, f64) {
    Player::with_uuid(srv, name, uuid).position().unwrap_or((BASE_X as f64, 64.0, BASE_Z as f64))
}

use crate::chip::{CircuitBlock, CircuitData, Port, PortSide};
use crate::designs::{self, DesignEntry, DesignMeta};
use crate::port;
use crate::vm::Tier;

/// Y level of the build platform inside every editor lane.
const EDITOR_Y: i32 = 10;
/// Fixed base coordinates for lane 0; each further lane is offset along X.
const BASE_X: i32 = 2_000_000;
const BASE_Z: i32 = 2_000_000;
/// Spacing between lanes — comfortably larger than the biggest tier (256).
const LANE_SPACING: i32 = 320;

struct EditorSession {
    design_id: Option<String>,
    name: String,
    tier: Tier,
    origin_x: i32,
    origin_z: i32,
    size: u32,
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

fn dimension() -> &'static str {
    yog_api::world::dimension::OVERWORLD
}

/// Enter (or re-enter) the editor for a design. `existing` is `Some` when
/// editing a previously saved design, `None` for a brand-new blank one.
/// `return_pos` is where the player gets teleported back to on `/vlsi save`
/// — callers should pass the player's real, currently-known position since
/// the loader can only resolve `Player::position()` when a UUID is on hand.
pub fn enter(srv: &dyn yog_api::Server, player_name: &str, name: &str, tier: Tier, design_id: Option<String>, existing: Option<CircuitData>, return_pos: (f64, f64, f64)) {
    let lane = lane_for(player_name);
    let origin_x = BASE_X + lane as i32 * LANE_SPACING;
    let origin_z = BASE_Z;
    let size = tier.world_size();
    let world = World::new(srv, dimension());

    // Clear the platform, then lay bedrock floor + a low glass boundary wall.
    for x in 0..size as i32 {
        for z in 0..size as i32 {
            let p = BlockPos { x: origin_x + x, y: EDITOR_Y, z: origin_z + z };
            world.set_block(p, "minecraft:air");
            world.set_block(BlockPos { x: p.x, y: EDITOR_Y - 1, z: p.z }, "minecraft:bedrock");
        }
    }
    for x in 0..size as i32 {
        for &z in &[0, size as i32 - 1] {
            world.set_block(BlockPos { x: origin_x + x, y: EDITOR_Y, z: origin_z + z }, "minecraft:glass");
        }
    }
    for z in 0..size as i32 {
        for &x in &[0, size as i32 - 1] {
            world.set_block(BlockPos { x: origin_x + x, y: EDITOR_Y, z: origin_z + z }, "minecraft:glass");
        }
    }

    if let Some(circuit) = &existing {
        for block in &circuit.blocks {
            let p = BlockPos { x: origin_x + block.x as i32, y: EDITOR_Y, z: origin_z + block.z as i32 };
            world.set_block(p, &block.block_id);
        }
        for port in &circuit.ports {
            let p = BlockPos { x: origin_x + port.index as i32 % size as i32, y: EDITOR_Y, z: origin_z + port.index as i32 / size as i32 };
            world.set_block(p, port::block_for_dir(port.dir));
        }
    }

    SESSIONS.lock().unwrap().insert(player_name.to_string(), EditorSession {
        design_id, name: name.to_string(), tier, origin_x, origin_z, size, return_pos,
    });

    srv.teleport(player_name, (origin_x + size as i32 / 2) as f64, (EDITOR_Y + 1) as f64, (origin_z + size as i32 / 2) as f64);
    srv.send_actionbar(player_name, &format!("§6Editing '{}' — {} tier, {}×{}. §7/vlsi save when done.", name, tier.name(), size, size));
}

/// Scan the player's active editor platform, save it as a design, and
/// teleport them back. Returns a status message.
pub fn save(srv: &dyn yog_api::Server, player_name: &str) -> String {
    let session = match SESSIONS.lock().unwrap().remove(player_name) {
        Some(s) => s,
        None => return "§cYou are not editing a VLSI circuit right now.".into(),
    };

    let world = World::new(srv, dimension());
    let size = session.size;
    let mut blocks = Vec::new();
    let mut ports = Vec::new();

    for x in 0..size {
        for z in 0..size {
            let p = BlockPos { x: session.origin_x + x as i32, y: EDITOR_Y, z: session.origin_z + z as i32 };
            let block_id = match world.get_block(p) {
                Some(id) => id,
                None => continue,
            };
            if block_id == "minecraft:air" || block_id == "minecraft:glass" { continue; }

            if let Some(dir) = port::dir_for_block(&block_id) {
                let label = port::read_label(srv, dimension(), p).unwrap_or_else(|| format!("Port {}", ports.len()));
                let side = if x == 0 { PortSide::West }
                    else if x == size - 1 { PortSide::East }
                    else if z == 0 { PortSide::North }
                    else { PortSide::South };
                ports.push(Port { label, side, index: z * size + x, dir });
                continue;
            }

            blocks.push(CircuitBlock { x, y: 0, z, block_id, state_json: "{}".into() });
        }
    }

    let design_id = session.design_id.clone().unwrap_or_else(crate::chip::new_chip_id);
    let circuit = CircuitData { chip_id: design_id.clone(), width: size, height: size, blocks, ports };
    let entry = DesignEntry {
        meta: DesignMeta {
            id: design_id,
            name: session.name.clone(),
            tier: session.tier,
            description: format!("{} ports, {} blocks", circuit.ports.len(), circuit.blocks.len()),
            saved_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
            port_count: circuit.ports.len(),
        },
        circuit,
    };
    let game_dir = srv.game_dir();
    designs::save_design(&game_dir, player_name, &entry);

    let (rx, ry, rz) = session.return_pos;
    srv.teleport(player_name, rx, ry, rz);

    format!("§aSaved '{}' — {} ports, {} blocks.", entry.meta.name, entry.meta.port_count, entry.circuit.blocks.len())
}

pub fn is_editing(player_name: &str) -> bool {
    SESSIONS.lock().unwrap().contains_key(player_name)
}

pub fn register(registry: &mut Registry) {
    // `/vlsi design <name> <tier>` — create a blank design and jump straight into the editor.
    registry.on_typed_command("vlsi", "word word word", |ctx, srv| {
        let sub = ctx.arg_str(0).unwrap_or("");
        if sub != "design" { return None; }
        let name = ctx.arg_str(1).unwrap_or("Unnamed").to_string();
        let tier = match crate::commands::parse_tier_pub(ctx.arg_str(2).unwrap_or("")) {
            Some(t) => t, None => return Some("§cUsage: /vlsi design <name> <tier>".into()),
        };
        let return_pos = player_pos(srv, &ctx.source, &ctx.uuid);
        enter(srv, &ctx.source, &name, tier, None, None, return_pos);
        Some(format!("§aOpened editor for new design '{}'.", name))
    });

    // `/vlsi edit <name>` — reopen an existing design for editing.
    registry.on_typed_command("vlsi", "word word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "edit" { return None; }
        let name = ctx.arg_str(1).unwrap_or("");
        let game_dir = srv.game_dir();
        let list = designs::list_designs(&game_dir, &ctx.source);
        let meta = match list.iter().find(|d| d.name == name) {
            Some(m) => m.clone(),
            None => return Some(format!("§cDesign '{}' not found.", name)),
        };
        let entry = match designs::load_design(&game_dir, &ctx.source, &meta.id) {
            Some(e) => e,
            None => return Some("§cFailed to load design data.".into()),
        };
        let return_pos = player_pos(srv, &ctx.source, &ctx.uuid);
        enter(srv, &ctx.source, &meta.name, meta.tier, Some(meta.id), Some(entry.circuit), return_pos);
        Some(format!("§aOpened editor for '{}'.", name))
    });

    // `/vlsi save` — save the active editor session and return.
    registry.on_typed_command("vlsi", "word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "save" { return None; }
        Some(save(srv, &ctx.source))
    });
}
