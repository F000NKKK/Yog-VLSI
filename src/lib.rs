//! Yog VLSI — Very Large Scale Integration.
//!
//! Design, fabricate, and deploy redstone microchips with a Rust-accelerated
//! simulation VM. Multi-tier workbench, ALU block, and chip-to-chip linking.

mod alu;
mod chip;
mod commands;
mod designs;
mod vm;
mod workbench;

use yog_api::{info, Mod, Registry};

pub struct YogVlsi;

impl Mod for YogVlsi {
    fn register(registry: &mut Registry) {
        info!("[yog-vlsi] initializing VLSI microchip system...");

        // Register blocks, items, and recipes.
        workbench::register(registry);
        alu::register(registry);
        chip::register(registry);

        // Register debug/utility commands.
        commands::register(registry);

        // ALU tick handler: step all installed chip VMs.
        registry.on_tick(|srv| {
            alu::tick_all(srv);
        });

        info!("[yog-vlsi] ready.");
    }
}

yog_api::export_mod!(YogVlsi);
