//! VLSI Workbench: block for designing and fabricating microchips.
//!
//! Interaction:
//! - Right-click empty → show design library + resource status
//! - Right-click with blank chip → insert into workbench slot
//! - Right-click with Blueprint → import into library
//! - Right-click with resource items → add to resource ammo

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

use yog_api::player::Player;
use yog_api::{BlockDef, ItemDef, Registry, Storage};

use crate::chip::{ChipMeta, CircuitData};
use crate::designs;
use crate::vm::Tier;

pub const WORKBENCH_ID: &str = "yog-vlsi:vlsi_workbench";
pub const BLUEPRINT_ID: &str = "yog-vlsi:blueprint";

/// In-memory resource storage per workbench position: (x, y, z) → (item_id → quantity)
pub static RESOURCES: LazyLock<Mutex<HashMap<(i32, i32, i32), HashMap<String, u64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// In-memory chip slot per workbench position: (x, y, z) → Option<(ChipMeta, CircuitData)>
pub static CHIP_SLOTS: LazyLock<Mutex<HashMap<(i32, i32, i32), Option<(ChipMeta, CircuitData)>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register(registry: &mut Registry) {
    // ── Workbench block ────────────────────────────────────────────────────
    registry.register_block(
        BlockDef::new(WORKBENCH_ID)
            .strength(3.5, 6.0)
            .sound("metal")
            .requires_tool()
    );

    registry.register_item(
        ItemDef::new(WORKBENCH_ID)
            .name("VLSI Workbench")
            .tooltip("§7Design and fabricate redstone microchips.\n§7Right-click to open the design interface.\n§7Insert blank chips, Blueprints, or resources.")
    );

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

    // ── Blueprint item ─────────────────────────────────────────────────────
    registry.register_item(
        ItemDef::new(BLUEPRINT_ID)
            .name("VLSI Blueprint")
            .tooltip("§7Stores a chip design for sharing.\n§7Use in a VLSI Workbench to import.\n§7Craft an empty Blueprint, then Export from the workbench.")
            .max_stack(1)
    );

    // Empty Blueprint recipe
    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:blueprint_craft", BLUEPRINT_ID, 1)
            .row("PPP")
            .row("PIP")
            .row("PPP")
            .key('P', "minecraft:paper")
            .key('I', "minecraft:iron_ingot")
    );


    // ── Cable blocks ───────────────────────────────────────────────────────
    // Analog Cable
    registry.register_block(
        BlockDef::new("yog-vlsi:analog_cable")
            .strength(0.5, 0.5)
            .sound("stone")
            .no_collision()
    );
    registry.register_item(
        ItemDef::new("yog-vlsi:analog_cable")
            .name("Analog Cable")
            .tooltip("§7Carries a single analog redstone signal (0–15).
§7Connects Redstone Port to external redstone.")
    );
    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:analog_cable_craft", "yog-vlsi:analog_cable", 3)
            .row("RRR")
            .key('R', "minecraft:redstone")
    );

    // Digital Cable
    registry.register_block(
        BlockDef::new("yog-vlsi:digital_cable")
            .strength(0.5, 0.5)
            .sound("stone")
            .no_collision()
    );
    registry.register_item(
        ItemDef::new("yog-vlsi:digital_cable")
            .name("Digital Cable")
            .tooltip("§7Carries up to 256 digital bits (64 channels × 4 colors).
§7Connects Digital Ports between ALUs.")
    );
    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:digital_cable_craft", "yog-vlsi:digital_cable", 4)
            .row("GRG")
            .row("RIR")
            .row("GRG")
            .key('G', "minecraft:gold_nugget")
            .key('R', "minecraft:redstone")
            .key('I', "minecraft:iron_ingot")
    );

    // Redstone Port (adapter block)
    registry.register_block(
        BlockDef::new("yog-vlsi:redstone_port")
            .strength(2.0, 6.0)
            .sound("stone")
    );
    registry.register_item(
        ItemDef::new("yog-vlsi:redstone_port")
            .name("Redstone Port")
            .tooltip("§7Adapter between ALU and external redstone.
§7Place on any side of an ALU. Connect Analog Cable on one side, redstone on the other.")
    );
    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:redstone_port_craft", "yog-vlsi:redstone_port", 1)
            .row("SSS")
            .row("SRS")
            .row("SSS")
            .key('S', "minecraft:smooth_stone")
            .key('R', "minecraft:redstone")
    );

    // ── Workbench right-click handler ──────────────────────────────────────
    registry.on_use_block(move |e, phase, srv| -> bool {
        if phase != yog_api::EventPhase::Pre { return true; }
        if e.block_id != WORKBENCH_ID { return true; }

        let key = (e.pos.x, e.pos.y, e.pos.z);
        let game_dir = srv.game_dir();
        let player_name = e.player_name.clone();

        // Check held item
        let held_nbt = srv.get_held_item_nbt(&e.player_name);
        let held_item = {
            let slots = Player::new(srv, &e.player_name).inventory();
            slots.into_iter()
                .find(|(slot, _, _)| *slot == 36) // hotbar slot 0
                .map(|(_, id, _)| id)
        };

        // Try to detect Blueprint in hand
        if let Some(ref item_id) = held_item {
            if item_id == BLUEPRINT_ID {
                if let Some(nbt) = &held_nbt {
                    // Import Blueprint into library
                    if let Some(circuit) = extract_blueprint_circuit(nbt) {
                        let tier = tier_from_size(circuit.width);
                        let design_id = designs::import_design(
                            &game_dir, &player_name,
                            "Imported Design", tier,
                            circuit.ports.clone(), circuit,
                        );
                        srv.broadcast(&format!(
                            "§a{} imported a Blueprint into their design library (ID: {})",
                            e.player_name, &design_id[..8]
                        ));
                        // Consume the Blueprint? No — Blueprint is reusable per design
                        return false;
                    }
                }
                return false;
            }
        }

        // Try to detect blank chip
        if let Some(ref item_id) = held_item {
            if item_id.starts_with("yog-vlsi:chip_") {
                let tier = match item_id.as_str() {
                    "yog-vlsi:chip_wood" => Tier::Wood,
                    "yog-vlsi:chip_stone" => Tier::Stone,
                    "yog-vlsi:chip_gold" => Tier::Gold,
                    "yog-vlsi:chip_iron" => Tier::Iron,
                    "yog-vlsi:chip_diamond" => Tier::Diamond,
                    "yog-vlsi:chip_netherite" => Tier::Netherite,
                    _ => return true,
                };

                // Is this a programmed chip? Check NBT
                if let Some(nbt) = &held_nbt {
                    if let Some(meta) = ChipMeta::from_nbt(nbt) {
                        // Programmed chip — show info
                        srv.broadcast(&format!(
                            "§e{} placed programmed chip '{}' ({} tier, {} ports) into workbench",
                            e.player_name, meta.name, meta.tier.name(), meta.ports.len()
                        ));
                        // TODO: load circuit data and store in workbench slot
                        return false;
                    }
                }

                // Blank chip — insert into workbench slot
                srv.broadcast(&format!(
                    "§a{} inserted blank {} chip into workbench. §7Use /vlsi designs to view your library.",
                    e.player_name, tier.name()
                ));
                return false;
            }
        }

        // Try to add resources from held item
        if let Some(ref item_id) = held_item {
            if !item_id.starts_with("yog-vlsi:") && item_id != "minecraft:air" {
                let mut resources = RESOURCES.lock().unwrap();
                let wb_res = resources.entry(key).or_default();
                let count = wb_res.entry(item_id.clone()).or_default();
                *count += 1;
                // Consume one item from hand (simplified)
                srv.broadcast(&format!(
                    "§a{} added 1× {} to workbench resources. Total: {}",
                    e.player_name, item_id, *count
                ));
                return false;
            }
        }

        // No special item — open workbench GUI
        crate::workbench_ui::set_player(&game_dir, &player_name);
        yog_api::open_ui("yog-vlsi:workbench", true, false);

        false
    });
}

/// Extract CircuitData from a Blueprint item's NBT.
fn extract_blueprint_circuit(nbt: &str) -> Option<CircuitData> {
    let key = "YogVlsiBlueprint:\"";
    if let Some(start) = nbt.find(key) {
        let start = start + key.len();
        let chars: Vec<char> = nbt[start..].chars().collect();
        let mut i = 0;
        let mut end = start;
        while i < chars.len() {
            if chars[i] == '\\' && i + 1 < chars.len() {
                i += 2;
            } else if chars[i] == '"' {
                end = start + i;
                break;
            } else {
                i += 1;
            }
        }
        let json = &nbt[start..end].replace("\\\"", "\"").replace("\\\\", "\\");
        CircuitData::from_json(json)
    } else {
        None
    }
}

/// Infer chip tier from world size.
fn tier_from_size(size: u32) -> Tier {
    match size {
        16 => Tier::Wood,
        32 => Tier::Stone,
        64 => Tier::Gold, // also Iron — but same size, default to Gold
        128 => Tier::Diamond,
        256 => Tier::Netherite,
        _ => Tier::Wood,
    }
}

/// Save all workbench resources to persistent storage.
pub fn save_resources(srv: &dyn yog_api::Server) {
    let game_dir = srv.game_dir();
    let resources = RESOURCES.lock().unwrap();
    let mut store = Storage::open(&game_dir, "yog-vlsi");
    let json = serde_json::to_string(&*resources).unwrap_or_default();
    store.set("workbench_resources", &*json);
    let _ = store.flush();
}

/// Load all workbench resources from persistent storage.
pub fn load_resources(srv: &dyn yog_api::Server) {
    let game_dir = srv.game_dir();
    let store = Storage::open(&game_dir, "yog-vlsi");
    if let Some(json) = store.get("workbench_resources").and_then(|v| v.as_str()) {
        if let Ok(loaded) = serde_json::from_str::<HashMap<(i32,i32,i32), HashMap<String, u64>>>(json) {
            *RESOURCES.lock().unwrap() = loaded;
        }
    }
}
