//! View model of the workbench's inventory screen (`workbench.yui`): the
//! chip and resource slots, the player's design library, and the actions on
//! the selected design. Layout, slots and list handling come from the markup
//! and yog-kit.

yog_api::ui_include!("workbench");

use yog_api::inventory;
use yog_kit::prelude::*;

use crate::designs::{DesignMeta, list_designs, load_design};
use crate::network;
use crate::res::Strings;

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
