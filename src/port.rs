//! VLSI Port block: the only physical block players place inside the virtual
//! circuit editor. Mode (Input/Output/Bidirectional) is represented by which
//! of the three concrete block variants occupies the cell — right-clicking
//! cycles through them in place. The port's display label lives in block NBT
//! and can be renamed via `/vlsi port <name>` while looking at the block.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::{BlockDef, BlockPos, ItemDef, Registry, World};

use crate::chip::PortDir;

pub const PORT_INPUT: &str = "yog-vlsi:port_input";
pub const PORT_OUTPUT: &str = "yog-vlsi:port_output";
pub const PORT_BIDI: &str = "yog-vlsi:port_bidi";

/// Last port block each player right-clicked: player_name → (dimension, pos).
/// Used as the target for `/vlsi port <name>`.
static LAST_TOUCHED: LazyLock<Mutex<HashMap<String, (String, BlockPos)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn is_port_block(block_id: &str) -> bool {
    matches!(block_id, PORT_INPUT | PORT_OUTPUT | PORT_BIDI)
}

pub fn dir_for_block(block_id: &str) -> Option<PortDir> {
    match block_id {
        PORT_INPUT => Some(PortDir::Input),
        PORT_OUTPUT => Some(PortDir::Output),
        PORT_BIDI => Some(PortDir::Bidirectional),
        _ => None,
    }
}

pub fn block_for_dir(dir: PortDir) -> &'static str {
    match dir {
        PortDir::Input => PORT_INPUT,
        PortDir::Output => PORT_OUTPUT,
        PortDir::Bidirectional => PORT_BIDI,
    }
}

fn next_dir(dir: PortDir) -> PortDir {
    match dir {
        PortDir::Input => PortDir::Output,
        PortDir::Output => PortDir::Bidirectional,
        PortDir::Bidirectional => PortDir::Input,
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
    for (id, name) in [
        (PORT_INPUT, "VLSI Port (Input)"),
        (PORT_OUTPUT, "VLSI Port (Output)"),
        (PORT_BIDI, "VLSI Port (Bidirectional)"),
    ] {
        registry.register_block(BlockDef::new(id).strength(1.0, 3.0).sound("stone"));
        registry.register_item(
            ItemDef::new(id)
                .name(name)
                .tooltip("§7Place inside a chip's virtual circuit.\n§7Right-click to cycle Input/Output/Bidirectional.\n§7Sneak + right-click, then type a name in chat to rename."),
        );
    }

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

        let current = dir_for_block(&e.block_id).unwrap_or(PortDir::Input);
        let next = next_dir(current);
        let world = World::new(srv, dimension);
        let label = read_label(srv, dimension, e.pos);
        world.set_block(e.pos, block_for_dir(next));
        if let Some(label) = label {
            write_label(srv, dimension, e.pos, &label);
        }
        srv.send_actionbar(&e.player_name, &format!("§ePort mode: {}", next.name()));
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
