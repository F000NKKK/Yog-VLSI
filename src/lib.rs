//! Yog VLSI — Very Large Scale Integration.

mod board;
mod cables;
mod chip;
mod commands;
mod compile;
mod designs;
mod display;
mod editor;
mod gpu;
mod nbt;
mod resources;
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

        // A written Blueprint shows its design: its name, then tier, ports and blocks.
        registry.on_item_text(content::Items::BLUEPRINT, |_, snbt| {
            let bp = chip::Blueprint::from_nbt(snbt)?;
            let info = res::Strings::screen_workbench_design_tier(bp.tier.name(), bp.circuit.ports.len(), bp.circuit.blocks.len());
            Some((Some(bp.name), vec![format!("§7{info}")]))
        });
        // A fabricated chip names its design and shows its tier, speed and ports.
        for tier in content::Tier::ALL {
            registry.on_item_text(tier.chip(), |_, snbt| {
                let meta = chip::ChipMeta::from_nbt(snbt)?;
                let ports: Vec<String> = meta
                    .ports
                    .iter()
                    .map(|p| match (p.dir.inputs(), p.dir.outputs()) {
                        (true, true) => format!("↔{}", p.label),
                        (true, false) => format!("→{}", p.label),
                        _ => format!("{}→", p.label),
                    })
                    .collect();
                let ports = if ports.is_empty() { res::Strings::chip_no_ports() } else { res::Strings::chip_ports(ports.join(", ")) };
                Some((Some(meta.name), vec![res::Strings::chip_info(meta.tier.name(), meta.tier.steps_per_tick()), ports]))
            });
        }
        workbench::setup(registry);
        alu::setup(registry);
        board::setup(registry);
        port::setup(registry);
        ui::display::setup(registry);
        display::register(registry);
        editor::register(registry);
        commands::register(registry);
        cables::register(registry);

        // Redstone in, the ALUs step, redstone out — a signal crosses
        // ports, cables and ALUs in one tick; only the chips' steps take time.
        registry.on_tick(|srv| {
            cables::tick_inputs(srv);
            board::tick(srv);
            display::tick(srv);
            cables::tick_outputs(srv);
            workbench::tick(srv);
        });

        // Persistence
        registry.on_server_started(|srv| {
            workbench::load_resources(srv);
            board::load(srv);
            cables::load(srv);
            editor::load(srv);
            port::load(srv);
            display::load(srv);
        });
        registry.on_server_stopping(|srv| {
            editor::save_sessions(srv);
            workbench::save_resources(srv);
            board::save(srv);
            cables::save(srv);
            port::save(srv);
            display::save(srv);
        });

        info!("[yog-vlsi] ready.");
    }
}

yog_api::export_mod!(YogVlsi);
