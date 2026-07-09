//! VLSI Workbench: block for designing and fabricating microchips.
//!
//! Now backed by the yog-inventory framework — right-click opens a real
//! Container/Menu screen with slots (chip + resources) + player inventory.
//! Custom UI (design library, fabricate button) rendered via yog-ui callbacks.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

use yog_api::{BlockDef, InventoryDef, ItemDef, Registry, Storage};

use crate::chip::CircuitData;
use crate::vm::Tier;

pub const WORKBENCH_ID: &str = "yog-vlsi:vlsi_workbench";
pub const BLUEPRINT_ID: &str = "yog-vlsi:blueprint";

/// In-memory resource storage per workbench position: (x, y, z) → (item_id → quantity).
/// TODO: migrate to block-entity inventory slots (phase 7 follow-up).
pub static RESOURCES: LazyLock<Mutex<HashMap<(i32, i32, i32), HashMap<String, u64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register(registry: &mut Registry) {
    // ── Workbench inventory (phase 7) ───────────────────────────────────────
    const WORKBENCH_INV: &str = "yog-vlsi:workbench";
    registry.register_inventory(
        InventoryDef::new(WORKBENCH_INV, 9)
            .title("VLSI Workbench")
            .include_player_inventory(true)
    );

    // ── Workbench block ────────────────────────────────────────────────────
    registry.register_block(
        BlockDef::new(WORKBENCH_ID)
            .strength(3.5, 6.0)
            .sound("metal")
            .requires_tool()
            .inventory(WORKBENCH_INV)
    );

    registry.register_item(
        ItemDef::new(WORKBENCH_ID)
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
            .shape(5.0, 5.0, 5.0, 11.0, 11.0, 11.0)
            .connects_to_neighbors()
            .connect_groups(&["analog"])
    );
    registry.register_item(
        ItemDef::new("yog-vlsi:analog_cable")
            .tooltip("§7Carries a single analog redstone signal (0–15).\n§7Connects Redstone Port to external redstone.")
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
            .shape(5.0, 5.0, 5.0, 11.0, 11.0, 11.0)
            .connects_to_neighbors()
            .connect_groups(&["digital"])
    );
    registry.register_item(
        ItemDef::new("yog-vlsi:digital_cable")
            .tooltip("§7Carries up to 256 digital bits (64 channels × 4 colors).\n§7Connects Digital Ports between ALUs.")
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
            .connect_groups(&["analog"])
    );
    registry.register_item(
        ItemDef::new("yog-vlsi:redstone_port")
            .tooltip("§7Adapter between ALU and external redstone.\n§7Place on any side of an ALU. Connect Analog Cable on one side, redstone on the other.")
    );
    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:redstone_port_craft", "yog-vlsi:redstone_port", 1)
            .row("SSS")
            .row("SRS")
            .row("SSS")
            .key('S', "minecraft:smooth_stone")
            .key('R', "minecraft:redstone")
    );

    // ── Workbench UI (yog-ui overlay on inventory screen) ──────────────────
    // Capture player context so the UI render callback knows whose designs to show.
    registry.on_use_block(move |e, phase, srv| -> bool {
        if phase != yog_api::EventPhase::Pre { return true; }
        if e.block_id != WORKBENCH_ID { return true; }
        crate::workbench_inv_ui::set_player_context(&srv.game_dir(), &e.player_name);
        true // don't cancel — let the inventory open
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

fn tier_from_size(size: usize) -> Tier {
    match size {
        16 => Tier::Wood,
        32 => Tier::Stone,
        64 => Tier::Gold,
        128 => Tier::Diamond,
        256 => Tier::Netherite,
        _ => Tier::Iron, // default
    }
}

pub fn load_resources(srv: &dyn yog_api::Server) {
    let store = Storage::open(&srv.game_dir(), "yog-vlsi/workbench_resources");
    if let Some(data) = store.get("resources").and_then(|v| v.as_str()) {
        if let Ok(parsed) = serde_json::from_str(data) {
            *RESOURCES.lock().unwrap() = parsed;
        }
    }
}

pub fn save_resources(srv: &dyn yog_api::Server) {
    let resources = RESOURCES.lock().unwrap();
    if let Ok(json) = serde_json::to_string(&*resources) {
        let mut store = Storage::open(&srv.game_dir(), "yog-vlsi/workbench_resources");
        store.set("resources", &*json);
        let _ = store.flush();
    }
}
