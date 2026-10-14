//! Yog VLSI — Very Large Scale Integration.

mod board;
mod cables;
mod chip;
mod commands;
mod editor;
mod nbt;
mod resources;
pub mod tier;
pub mod vm;
mod ui;

use ui::{alu, port, workbench};

// `crate::res` — typed accessors for resources/*.yres (Strings::...).
yog_api::resources!();
// `crate::content` — ids and definitions of content/ blocks and items.
yog_api::content!();

use yog_api::{info, Mod, Registry, YogDimensionDef, YogDimensionTypeDef};

/// The mod id — its `Storage` namespace.
pub const MOD_ID: &str = "yog-vlsi";

pub struct YogVlsi;

impl Mod for YogVlsi {
    fn register(registry: &mut Registry) {
        info!("[yog-vlsi] initializing VLSI microchip system...");

        // Dedicated pocket dimension for the circuit editor (see `editor.rs`) —
        // replaces the old "reserved lane far out in the Overworld" hack now that
        // the loader can declare real custom dimensions. Void terrain: the editor
        // lays its own floor and barrier box per session, so the generator
        // itself has nothing to do.
        registry.register_dimension(
            &YogDimensionDef::new(editor::DIMENSION_ID).dimension_type(
                YogDimensionTypeDef::default()
                    .ambient_light(1.0)
                    .has_sky_light(false)
                    .has_ceiling(false)
                    .natural(false)
                    .piglin_safe(false)
                    .effects("minecraft:the_end")
                    .min_y(0)
                    .height(384)
                    .logical_height(384),
            )
            .generator_type(editor::GENERATOR_ID)
            .structures(false)
            .features(false)
            .mob_spawning(false),
        );
        registry.register_chunk_generator(editor::GENERATOR_ID, |_writer| {
            // Void — the editor builds its own floor/walls per session.
        });

        workbench::setup(registry);
        alu::setup(registry);
        board::setup(registry);
        port::setup(registry);
        editor::register(registry);
        commands::register(registry);
        cables::register(registry);

        registry.on_tick(|srv| {
            board::tick(srv);
            cables::tick(srv);
            workbench::tick(srv);
        });

        // Persistence
        registry.on_server_started(|srv| {
            workbench::load_resources(srv);
            board::load(srv);
            cables::load(srv);
            port::load(srv);
        });
        registry.on_server_stopping(|srv| {
            editor::finish_all(srv);
            workbench::save_resources(srv);
            board::save(srv);
            cables::save(srv);
            port::save(srv);
        });

        info!("[yog-vlsi] ready.");
    }
}

yog_api::export_mod!(YogVlsi);
