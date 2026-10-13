//! VLSI Workbench: block for designing and fabricating microchips.
//!
//! Right-click opens its inventory screen (`workbench.yui`): the chip and
//! resource slots, the player's inventory, and the design library.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

use yog_api::{Registry, Storage};

use crate::content::{Blocks, Items};


pub const WORKBENCH_ID: &str = Blocks::VLSI_WORKBENCH;
pub const BLUEPRINT_ID: &str = Items::BLUEPRINT;

/// In-memory resource storage per workbench position: (x, y, z) → (item_id → quantity).
/// TODO: migrate to block-entity inventory slots (phase 7 follow-up).
pub static RESOURCES: LazyLock<Mutex<HashMap<(i32, i32, i32), HashMap<String, u64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register(registry: &mut Registry) {
    // The screen's markup lays out the block's inventory.
    let screen = crate::workbench_ui::register(registry);
    // content/vlsi_workbench.yblock, registered here to attach that inventory.
    registry.register_block(Blocks::vlsi_workbench().inventory(crate::workbench_ui::ID));

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

    // Whose design library the screen shows.
    registry.on_use_block(move |e, phase, srv| -> bool {
        if phase == yog_api::EventPhase::Pre && e.block_id == WORKBENCH_ID {
            let (game_dir, player, dim) = (srv.game_dir(), e.player_name.clone(), e.dimension.clone());
            screen.update(move |vm| vm.opened_by(game_dir, player, dim));
        }
        true // the inventory opens
    });
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
