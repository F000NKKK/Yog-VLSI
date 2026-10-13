//! VLSI Workbench: block for designing and fabricating microchips.
//!
//! Right-click opens its inventory screen (`workbench.yui`): the chip and
//! resource slots, the player's inventory, and the design library. This
//! file is its code-behind — the view model `WorkbenchVm` — plus the
//! block, its recipes and its resource storage.

yog_api::ui_include!("workbench");

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

use yog_api::{Registry, Storage, inventory};
use yog_kit::prelude::*;

use crate::content::{Blocks, Items};
use crate::designs::{DesignMeta, list_designs, load_design};
use crate::network;
use crate::res::Strings;

pub const WORKBENCH_ID: &str = Blocks::VLSI_WORKBENCH;
pub const BLUEPRINT_ID: &str = Items::BLUEPRINT;

/// In-memory resource storage per workbench position: (x, y, z) → (item_id → quantity).
/// TODO: migrate to block-entity inventory slots (phase 7 follow-up).
pub static RESOURCES: LazyLock<Mutex<HashMap<(i32, i32, i32), HashMap<String, u64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Items that fill a vial (one stack).
const VIAL_CAPACITY: u32 = 64;
/// Inner height of a vial: an 18px slot minus its 1px border.
const VIAL_HEIGHT: f32 = 16.0;

#[derive(Default)]
pub struct WorkbenchVm {
    /// Who opened the workbench and in which dimension — set by its use
    /// handler (the design library is per player).
    game_dir: String,
    player: String,
    dimension: String,
    designs: Vec<DesignMeta>,
    /// Chosen design's id.
    selected: Option<String>,
    /// The selected design's summary, read once per selection.
    info: String,
}

impl ViewModel for WorkbenchVm {
    fn on_open(&mut self, _cx: &mut Cx<'_, Self>) {
        self.designs = list_designs(&self.game_dir, &self.player);
        if !self.designs.iter().any(|d| Some(&d.id) == self.selected.as_ref()) {
            self.selected = None;
        }
        self.refresh_info();
    }
}

impl WorkbenchVm {
    /// The player `player` opened a workbench in `dimension`.
    pub fn opened_by(&mut self, game_dir: String, player: String, dimension: String) {
        self.game_dir = game_dir;
        self.player = player;
        self.dimension = dimension;
    }

    /// Items in container slot `slot` right now.
    pub fn count(&self, slot: usize) -> u32 {
        inventory::current_slot(slot).map_or(0, |s| s.count)
    }
    /// Fill height of the vial beside slot `slot`.
    pub fn vial(&self, slot: usize) -> f32 {
        VIAL_HEIGHT * (self.count(slot) as f32 / VIAL_CAPACITY as f32).min(1.0)
    }

    pub fn selected(&self) -> Option<&DesignMeta> {
        let id = self.selected.as_ref()?;
        self.designs.iter().find(|d| &d.id == id)
    }
    pub fn selected_id(&self) -> Option<&str> {
        self.selected().map(|d| d.id.as_str())
    }
    pub fn has_selection(&self) -> bool {
        self.selected().is_some()
    }
    pub fn design_info(&self) -> &str {
        &self.info
    }

    pub fn select(&mut self, id: String) {
        self.selected = Some(id);
        self.refresh_info();
    }

    pub fn edit(&mut self) {
        if let Some(d) = self.selected() {
            network::send_workbench_action(&["edit", &d.name, &self.dimension]);
        }
    }
    pub fn fabricate(&mut self) {
        if let Some(d) = self.selected() {
            network::send_workbench_action(&["fabricate", &d.name, d.tier.id()]);
        }
    }
    pub fn export_blueprint(&mut self) {
        if let Some(d) = self.selected() {
            network::send_workbench_action(&["export_bp", &d.name]);
        }
    }

    fn refresh_info(&mut self) {
        self.info = self
            .selected()
            .and_then(|d| load_design(&self.game_dir, &self.player, &d.id))
            .map(|e| Strings::screen_workbench_design_info(e.circuit.ports.len(), e.circuit.blocks.len()))
            .unwrap_or_default();
    }
}

/// The block, its screen and the recipes.
pub fn setup(registry: &mut Registry) {
    // The screen's markup lays out the block's inventory.
    let screen = register(registry);
    // content/vlsi_workbench.yblock, registered here to attach that inventory.
    registry.register_block(Blocks::vlsi_workbench().inventory(ID));

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_fit_the_screen_apart() {
        let def = yog_kit::inventory_of::<View>().unwrap();
        assert_eq!(def.slot_count, 7);
        assert!(def.include_player_inventory);
        let (px, py) = def.player_inv_offset;
        // The player's inventory (9×18 by 3×18 + 4 + 18) stays on screen.
        assert!(px + 9.0 * 18.0 <= WIDTH && py + 76.0 <= HEIGHT, "{px},{py}");
        for (i, a) in def.layout.iter().enumerate() {
            assert!(a.x >= 0.0 && a.y >= 0.0 && a.x + 16.0 <= WIDTH && a.y + 16.0 < py, "slot {i} at {a:?}");
            for b in &def.layout[i + 1..] {
                assert!((a.x - b.x).abs() >= 18.0 || (a.y - b.y).abs() >= 18.0, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn empty_library() {
        let mut h = yog_kit::testing::TestHost::<View>::new(WorkbenchVm::default());
        h.open();
        assert!(!h.vm().has_selection());
        assert!(!h.widget(ids::FABRICATE).unwrap().enabled);
    }
}
