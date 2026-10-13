//! VLSI Workbench: block for designing and fabricating microchips.
//!
//! Now backed by the yog-inventory framework — right-click opens a real
//! Container/Menu screen with slots (chip + resources) + player inventory.
//! Custom UI (design library, fabricate button) rendered via yog-ui callbacks.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

use yog_api::{InventoryDef, Registry, Storage};

use crate::content::{Blocks, Items};

use crate::chip::CircuitData;
use crate::vm::Tier;

pub const WORKBENCH_ID: &str = Blocks::VLSI_WORKBENCH;
pub const BLUEPRINT_ID: &str = Items::BLUEPRINT;
/// Custom slot count for the workbench's own inventory (chip + 6 resource
/// vials + 2 spares) — the real vanilla `Slot`s at indices `0..WORKBENCH_SLOT_COUNT`,
/// before the player's own inventory starts at `WORKBENCH_SLOT_COUNT`.
pub const WORKBENCH_SLOT_COUNT: usize = 9;

/// In-memory resource storage per workbench position: (x, y, z) → (item_id → quantity).
/// TODO: migrate to block-entity inventory slots (phase 7 follow-up).
pub static RESOURCES: LazyLock<Mutex<HashMap<(i32, i32, i32), HashMap<String, u64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register(registry: &mut Registry) {
    // ── Workbench inventory (phase 7) ───────────────────────────────────────
    // Real vanilla `Slot` positions are baked in from the exact same widget
    // tree `workbench_inv_ui` draws (see `measure_layout`), so the decorative
    // overlay and the real, draggable slots underneath always agree on where
    // things are — no hand-picked pixel offsets to keep in sync by hand.
    const WORKBENCH_INV: &str = "yog-vlsi:workbench";
    let ((bg_w, bg_h), slot_positions, player_inv_origin) = crate::workbench_inv_ui::measure_layout();
    registry.register_inventory(
        InventoryDef::new(WORKBENCH_INV, WORKBENCH_SLOT_COUNT)
            .title("VLSI Workbench")
            .include_player_inventory(true)
            .layout(slot_positions.into_iter().map(|(x, y)| yog_api::SlotLayout { x, y }).collect())
            .player_inv_offset(player_inv_origin.0, player_inv_origin.1)
            .background_size(bg_w, bg_h)
    );

    // ── Workbench block (content/vlsi_workbench.yblock, registered manually
    // to attach its inventory) ─────────────────────────────────────────────
    registry.register_block(Blocks::vlsi_workbench().inventory(WORKBENCH_INV));

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
    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:analog_cable_craft", Blocks::ANALOG_CABLE, 3)
            .row("RRR")
            .key('R', "minecraft:redstone")
    );

    // Digital Cable
    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:digital_cable_craft", Blocks::DIGITAL_CABLE, 4)
            .row("GRG")
            .row("RIR")
            .row("GRG")
            .key('G', "minecraft:gold_nugget")
            .key('R', "minecraft:redstone")
            .key('I', "minecraft:iron_ingot")
    );

    // Redstone Port (adapter block)
    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:redstone_port_craft", Blocks::REDSTONE_PORT, 1)
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
