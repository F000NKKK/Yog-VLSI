//! ALU block: accepts microchips, routes redstone signals, links chips into node graphs.
//!
//! Modes:
//! - Passthrough: 1-to-1 signal routing — chip port → block face
//! - Internal graph: connect output ports of one chip to input ports of another
//!
//! I/O nodes on each side of the ALU can be switched between Input / Output / Bidirectional.

use yog_api::{BlockDef, ItemDef, Registry};

/// The ALU block ID.
pub const ALU_ID: &str = "yog-vlsi:alu";

pub fn register(registry: &mut Registry) {
    // Register the ALU block.
    registry.register_block(
        BlockDef::new(ALU_ID)
            .strength(5.0, 12.0)
            .sound("metal")
            .requires_tool()
            .light_level(7) // subtle glow when powered
    );

    // Register the ALU item.
    registry.register_item(
        ItemDef::new(ALU_ID)
            .name("VLSI Arithmetic Logic Unit")
            .tooltip("§7Insert programmed microchips to execute redstone logic.\n§7Right-click to configure I/O nodes and chip linking.\n§7Modes: Passthrough (1:1) or Internal Graph (chip-to-chip)")
    );

    // Register crafting recipe: ALU
    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:alu_craft", ALU_ID, 1)
            .row("GCG")
            .row("CRC")
            .row("GDG")
            .key('G', "minecraft:gold_ingot")
            .key('C', "minecraft:copper_ingot")
            .key('R', "minecraft:repeater")
            .key('D', "minecraft:diamond")
    );

    // TODO: ALU tick handler (on_tick or scheduled) → step all installed chip VMs
    // TODO: register_ui for ALU configuration screen (I/O node modes, chip linking graph)
    // TODO: on_use_block handler → open ALU GUI
    // TODO: redstone signal routing based on configuration
    // TODO: chip-to-chip linking (node graph data model)
}
