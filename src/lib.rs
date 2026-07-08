//! Yog VLSI — Very Large Scale Integration.
//!
//! Design, fabricate, and deploy redstone microchips with a Rust-accelerated
//! simulation VM. Multi-tier workbench, ALU block, and chip-to-chip linking.

mod chip;
mod workbench;
mod alu;
mod vm;

use yog_api::{info, Mod, Registry};

pub struct YogVlsi;

impl Mod for YogVlsi {
    fn register(registry: &mut Registry) {
        info!("[yog-vlsi] initializing VLSI microchip system...");

        workbench::register(registry);
        alu::register(registry);
        chip::register(registry);

        // TODO: chip editing UI via register_ui + on_ui_render
        // TODO: Rust redstone VM startup
        // TODO: server-side chip storage (Storage::open_player)
        // TODO: ALU node graph linking

        info!("[yog-vlsi] ready.");
    }
}

yog_api::export_mod!(YogVlsi);
