//! ALU block: accepts microchips, routes signals, links chips into node graphs.

use yog_api::{BlockDef, ItemDef, Registry};
use crate::commands::{ALU_STATE, VM_CACHE};

pub const ALU_ID: &str = "yog-vlsi:alu";

pub fn register(registry: &mut Registry) {
    registry.register_block(BlockDef::new(ALU_ID).strength(5.0,12.0).sound("metal").requires_tool().light_level(7));
    registry.register_item(ItemDef::new(ALU_ID).name("Arithmetic Logic Unit").tooltip("Insert programmed microchips. Right-click to open node editor."));

    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:alu_craft", ALU_ID, 1)
            .row("GCG").row("CRC").row("GDG")
            .key('G',"minecraft:gold_ingot").key('C',"minecraft:copper_ingot")
            .key('R',"minecraft:repeater").key('D',"minecraft:diamond")
    );

    // ALU tier items (6 tiers, same block, different NBT)
    for tier in crate::vm::Tier::ALL {
        let tier_id = format!("yog-vlsi:alu_{}", tier.id());
        let (max_chips, channels) = match tier {
            crate::vm::Tier::Wood => (2, 8), crate::vm::Tier::Stone => (3, 16),
            crate::vm::Tier::Gold => (4, 24), crate::vm::Tier::Iron => (5, 32),
            crate::vm::Tier::Diamond => (6, 48), crate::vm::Tier::Netherite => (8, 64),
        };
        registry.register_item(ItemDef::new(&tier_id)
            .name(&format!("{} ALU", tier.name()))
            .tooltip(&format!("Max chips: {} | Digital channels/side: {}", max_chips, channels))
            .max_stack(1));
    }

    // Right-click → open ALU node editor
    registry.on_use_block(|e, phase, _srv| -> bool {
        if phase != yog_api::EventPhase::Pre { return true; }
        if e.block_id != ALU_ID { return true; }
        crate::alu_ui::set_alu_pos((e.pos.x, e.pos.y, e.pos.z));
        yog_api::open_ui("yog-vlsi:alu", true, false);
        false
    });
}

pub fn tick_all(_srv: &dyn yog_api::Server) {
    let state = ALU_STATE.lock().unwrap();
    let chip_ids: Vec<String> = state.values().flat_map(|c| c.iter().map(|(id,_)| id.clone())).collect();
    drop(state);
    let mut cache = VM_CACHE.lock().unwrap();
    for chip_id in &chip_ids {
        if let Some(vm) = cache.get_mut(chip_id) { vm.step(); }
    }
}
