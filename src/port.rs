//! VLSI Port block: the only physical block players place inside the virtual
//! circuit editor. Mode (Input/Output/Bidirectional) is represented by which
//! of the three concrete block variants occupies the cell — right-clicking
//! cycles through them in place. The port's display label lives in block NBT
//! and can be renamed via `/vlsi port <name>` while looking at the block.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::{BlockPos, Registry, World};

use crate::content::Blocks;

use crate::vm::PortMode;

pub const PORT_INPUT: &str = Blocks::PORT_INPUT;
pub const PORT_OUTPUT: &str = Blocks::PORT_OUTPUT;
pub const PORT_BIDI: &str = Blocks::PORT_BIDI;

/// Last port block each player right-clicked: player_name → (dimension, pos).
/// Used as the target for `/vlsi port <name>`.
static LAST_TOUCHED: LazyLock<Mutex<HashMap<String, (String, BlockPos)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn is_port_block(block_id: &str) -> bool {
    matches!(block_id, PORT_INPUT | PORT_OUTPUT | PORT_BIDI)
}

pub fn dir_for_block(block_id: &str) -> Option<PortMode> {
    match block_id {
        PORT_INPUT => Some(PortMode::Input),
        PORT_OUTPUT => Some(PortMode::Output),
        PORT_BIDI => Some(PortMode::Bidirectional),
        _ => None,
    }
}

pub fn block_for_dir(dir: PortMode) -> &'static str {
    match dir {
        PortMode::Input => PORT_INPUT,
        PortMode::Output => PORT_OUTPUT,
        PortMode::Bidirectional => PORT_BIDI,
    }
}

/// Read the `YogVlsiPortLabel` NBT string off a port block, if set.
pub fn read_label(srv: &dyn yog_api::Server, dimension: &str, pos: BlockPos) -> Option<String> {
    let nbt = srv.get_block_nbt(dimension, pos)?;
    let key = "YogVlsiPortLabel:\"";
    let start = nbt.find(key)? + key.len();
    let end = nbt[start..].find('"')? + start;
    Some(nbt[start..end].to_string())
}

fn write_label(srv: &dyn yog_api::Server, dimension: &str, pos: BlockPos, label: &str) {
    let escaped = label.replace('\\', "\\\\").replace('"', "\\\"");
    srv.set_block_nbt(dimension, pos, &format!("{{YogVlsiPortLabel:\"{}\"}}", escaped));
}

pub fn register(registry: &mut Registry) {
    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:port_craft", PORT_INPUT, 2)
            .row(" R ")
            .row("RSR")
            .row(" R ")
            .key('R', "minecraft:redstone")
            .key('S', "minecraft:stone"),
    );

    registry.on_use_block(|e, phase, srv| -> bool {
        if phase != yog_api::EventPhase::Pre { return true; }
        if !is_port_block(&e.block_id) { return true; }

        let dimension = yog_api::world::dimension::OVERWORLD;
        LAST_TOUCHED.lock().unwrap().insert(e.player_name.clone(), (dimension.to_string(), e.pos));

        let current = dir_for_block(&e.block_id).unwrap_or(PortMode::Input);
        let next = current.next();
        let world = World::new(srv, dimension);
        let label = read_label(srv, dimension, e.pos);
        world.set_block(e.pos, block_for_dir(next));
        if let Some(label) = label {
            write_label(srv, dimension, e.pos, &label);
        }
        srv.send_actionbar(&e.player_name, &format!("§ePort mode: {}", next.label()));
        false
    });

    // `/vlsi port <name>` — rename the last port block this player right-clicked.
    registry.on_typed_command("vlsi", "word string", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "port" { return None; }
        let name = ctx.arg_str(1).unwrap_or("").trim().to_string();
        if name.is_empty() { return Some("§cUsage: /vlsi port <name>".into()); }
        let touched = LAST_TOUCHED.lock().unwrap().get(&ctx.source).cloned();
        match touched {
            Some((dimension, pos)) => {
                write_label(srv, &dimension, pos, &name);
                Some(format!("§aPort renamed to '{}'.", name))
            }
            None => Some("§cRight-click a VLSI port block first.".into()),
        }
    });
}
