//! ALU block: accepts microchips, routes redstone signals, links chips into node graphs.
//!
//! Modes:
//! - Passthrough: 1-to-1 signal routing — chip port → block face
//! - Internal graph: connect output ports of one chip to input ports of another
//!
//! I/O nodes on each side of the ALU can be switched between Input / Output / Bidirectional.

use yog_api::{BlockDef, ItemDef, Registry};

use crate::chip::ChipMeta;
use crate::commands::{ALU_STATE, VM_CACHE};
use crate::vm::RedstoneVM;

/// The ALU block ID.
pub const ALU_ID: &str = "yog-vlsi:alu";

pub fn register(registry: &mut Registry) {
    // Register the ALU block.
    registry.register_block(
        BlockDef::new(ALU_ID)
            .strength(5.0, 12.0)
            .sound("metal")
            .requires_tool()
            .light_level(7)
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

    // Handle right-click on ALU: install chip from hand
    registry.on_use_block(|event, phase, srv| -> bool {
        if phase != yog_api::EventPhase::Pre { return true; }

        let block_id = srv.get_block(&event.world, event.block_pos);
        if block_id.as_deref() != Some(ALU_ID) { return true; }

        // Get held item NBT
        let nbt = match srv.get_held_item_nbt(&event.player) {
            Some(n) => n,
            None => return true, // no chip in hand
        };

        let meta = match ChipMeta::from_nbt(&nbt) {
            Some(m) => m,
            None => return true, // not a VLSI chip
        };

        // Install chip into ALU
        let key = (
            event.world.clone(),
            event.block_pos.0,
            event.block_pos.1,
            event.block_pos.2,
        );

        {
            let mut state = ALU_STATE.lock().unwrap();
            let chips = state.entry(key).or_default();
            chips.push((meta.id.clone(), meta.tier));
        }

        // Pre-load VM
        if let Some(circuit) = crate::commands::load_circuit(srv, &meta.id) {
            let mut vm = RedstoneVM::new(meta.tier);
            crate::commands::load_circuit_into_vm(&mut vm, &circuit);
            VM_CACHE.lock().unwrap().insert(meta.id.clone(), vm);
        }

        // Consume the chip from hand
        let _ = srv.set_held_item_nbt(&event.player, "");
        srv.broadcast(&format!(
            "§a{} installed chip '{}' ({} tier) into ALU at {:?}",
            event.player, meta.name, meta.tier.name(), event.block_pos
        ));

        false // cancel normal block use
    });

    // TODO: ALU GUI (register_ui) for configuring I/O node modes and chip linking
    // TODO: redstone signal routing (passthrough mode)
    // TODO: chip-to-chip internal linking (node graph)
}

/// Called every server tick to step all installed chip VMs.
pub fn tick_all(srv: &dyn yog_api::Server) {
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
