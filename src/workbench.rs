//! VLSI Workbench: block for designing and fabricating microchips.
//!
//! Right-click the workbench to open the chip design interface.
//! Resources are loaded like MFU paint (no storage limit, refill model).

use yog_api::{BlockDef, ItemDef, Registry};

/// The workbench block ID.
pub const WORKBENCH_ID: &str = "yog-vlsi:vlsi_workbench";

pub fn register(registry: &mut Registry) {
    // Register the workbench block.
    registry.register_block(
        BlockDef::new(WORKBENCH_ID)
            .strength(3.5, 6.0)
            .sound("metal")
            .requires_tool()
    );

    // Register the workbench item (for inventory / creative tab).
    registry.register_item(
        ItemDef::new(WORKBENCH_ID)
            .name("VLSI Workbench")
            .tooltip("§7Place microchips here to design and fabricate redstone circuits.\n§7Right-click to open the design interface.")
    );

    // Register crafting recipe: workbench itself
    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:vlsi_workbench_craft", WORKBENCH_ID, 1)
            .row("ISI")
            .row("SCS")
            .row("IRI")
            .key('I', "minecraft:iron_ingot")
            .key('S', "minecraft:smooth_stone")
            .key('C', "minecraft:crafting_table")
            .key('R', "minecraft:redstone_block")
    );

    // TODO: register_ui for the workbench GUI (chip slots, resource ammo, design/fabricate buttons)
    // TODO: on_use_block handler → open workbench GUI
    // TODO: virtual world editing mode (teleport player to creative instance)
    // TODO: resource ammo system (Storage-based, paint-like refill)
}
