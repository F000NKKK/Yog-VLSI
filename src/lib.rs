//! Yog VLSI — Very Large Scale Integration.

mod alu;
mod alu_ui;
mod chip;
mod commands;
mod designs;
mod vm;
mod workbench;
mod workbench_ui;

use yog_api::{info, Mod, Registry};

pub struct YogVlsi;

impl Mod for YogVlsi {
    fn register(registry: &mut Registry) {
        info!("[yog-vlsi] initializing VLSI microchip system...");

        workbench::register(registry);
        alu::register(registry);
        chip::register(registry);
        commands::register(registry);

        registry.on_tick(|srv| { alu::tick_all(srv); });

        // Workbench UI
        let wb_id = "yog-vlsi:workbench";
        registry.register_ui(wb_id, |uid, ev| workbench_ui::handle_click(uid, ev));
        registry.on_ui_render(wb_id, |gfx| workbench_ui::render(gfx));

        // ALU UI
        let alu_id = "yog-vlsi:alu";
        registry.register_ui(alu_id, |uid, ev| alu_ui::handle_click(uid, ev));
        registry.on_ui_render(alu_id, |gfx| alu_ui::render(gfx));

        // Resource persistence
        registry.on_server_started(|srv| { workbench::load_resources(srv); });
        registry.on_server_stopping(|srv| { workbench::save_resources(srv); });

        info!("[yog-vlsi] ready.");
    }
}

yog_api::export_mod!(YogVlsi);
