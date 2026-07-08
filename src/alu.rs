//! ALU block: accepts microchips, routes redstone signals, links chips into node graphs.
//!
//! Chips are installed through the ALU's UI (node editor), not by right-click.
//! The tick handler steps all installed chip VMs.

use yog_api::{BlockDef, ItemDef, Registry};

use crate::commands::{ALU_STATE, VM_CACHE};

pub const ALU_ID: &str = "yog-vlsi:alu";

pub fn register(registry: &mut Registry) {
    registry.register_block(
        BlockDef::new(ALU_ID)
            .strength(5.0, 12.0)
            .sound("metal")
            .requires_tool()
            .light_level(7)
    );

    registry.register_item(
        ItemDef::new(ALU_ID)
            .name("VLSI Arithmetic Logic Unit")
            .tooltip("§7Insert programmed microchips via the ALU UI.\n§7Right-click to open the node editor.\n§7Modes: Passthrough (1:1) or Internal Graph (chip-to-chip)")
    );

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

    // TODO: register_ui for ALU node editor (chip insertion, I/O node config, chip linking graph)
    // TODO: on_use_block → open ALU UI
    // TODO: redstone signal routing (passthrough + internal graph modes)
}

/// Called every server tick. Steps all installed chip VMs.
pub fn tick_all(_srv: &dyn yog_api::Server) {
    let state = ALU_STATE.lock().unwrap();
    let chip_ids: Vec<String> = state.values()
        .flat_map(|chips| chips.iter().map(|(id, _)| id.clone()))
        .collect();
    drop(state);

    let mut cache = VM_CACHE.lock().unwrap();
    for chip_id in &chip_ids {
        if let Some(vm) = cache.get_mut(chip_id) {
            vm.step();
        }
    }
}
