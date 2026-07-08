//! ALU block: accepts microchips, routes redstone signals, links chips into node graphs.

use yog_api::{BlockDef, ItemDef, Registry};

use crate::chip::ChipMeta;
use crate::commands::{load_circuit, load_circuit_into_vm, ALU_STATE, VM_CACHE};
use crate::vm::RedstoneVM;

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
            .tooltip("§7Insert programmed microchips to execute redstone logic.\n§7Right-click with a chip to install.\n§7Modes: Passthrough (1:1) or Internal Graph (chip-to-chip)")
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

    // Right-click ALU with a programmed chip → install it.
    registry.on_use_block(|e, phase, srv| -> bool {
        if phase != yog_api::EventPhase::Pre { return true; }
        if e.block_id != ALU_ID { return true; }

        let nbt = match srv.get_held_item_nbt(&e.player_name) {
            Some(n) => n,
            None => return true,
        };

        let meta = match ChipMeta::from_nbt(&nbt) {
            Some(m) => m,
            None => return true,
        };

        // Install chip into this ALU block.
        let key = (e.pos.x, e.pos.y, e.pos.z);

        {
            let mut state = ALU_STATE.lock().unwrap();
            state.entry(key).or_default().push((meta.id.clone(), meta.tier));
        }

        // Pre-load the VM from server storage.
        if let Some(circuit) = load_circuit(srv, &meta.id) {
            let mut vm = RedstoneVM::new(meta.tier);
            load_circuit_into_vm(&mut vm, &circuit);
            VM_CACHE.lock().unwrap().insert(meta.id.clone(), vm);
        }

        // Consume the chip from hand.
        let _ = srv.set_held_item_nbt(&e.player_name, "");
        srv.broadcast(&format!(
            "§a{} installed chip '{}' ({} tier) into ALU at ({}, {}, {})",
            e.player_name, meta.name, meta.tier.name(), e.pos.x, e.pos.y, e.pos.z
        ));

        false // cancel normal block interaction
    });

    // TODO: ALU GUI (register_ui) for configuring I/O node modes and chip linking
    // TODO: redstone signal routing (passthrough mode)
    // TODO: chip-to-chip internal linking (node graph)
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
