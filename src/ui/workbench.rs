//! VLSI Workbench: designs live on Blueprints, chips are fabricated from
//! them.
//!
//! The block's inventory screen (`workbench.yui`) has a chip slot (where a
//! fabricated chip comes out), six resource slots and a Blueprint slot.
//! Whatever goes into a resource slot is swallowed into the workbench's own
//! store (blocks count ×9, quartz blocks ×4), so it holds any amount. A
//! blank Blueprint gets a name and a tier (Create); a written one opens in
//! the editor or is fabricated into a chip, paid from the store.
//!
//! This file is the screen's code-behind — the view model `WorkbenchVm` —
//! and the server side: the stores, the requests, and the state each
//! player with a workbench open is kept up to date with.

yog_api::ui_include!("ui/workbench");

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use yog_api::player::Player;
use yog_api::{BlockPos, EventPhase, Registry, Server, Storage};
use yog_kit::prelude::*;

use crate::chip::{Blueprint, ChipMeta};
use crate::content::Items;
use crate::content::Tier;
use crate::designs;
use crate::editor;
use crate::res::Strings;
use crate::resources::{Amounts, Resource};

pub const WORKBENCH_ID: &str = crate::content::Blocks::VLSI_WORKBENCH;
pub const BLUEPRINT_ID: &str = Items::BLUEPRINT;

/// The workbench's own slots.
const CHIP_SLOT: u32 = 0;
const BLUEPRINT_SLOT: u32 = 7;
/// Client → server requests.
const REQUEST_CHANNEL: &str = "yog-vlsi:wb_request";
/// Server → client state.
const STATE_CHANNEL: &str = "yog-vlsi:wb_state";
/// Server ticks between looks at the open workbenches.
const TICK_EVERY: u64 = 5;
/// Where a vial is full: a million, on a log scale.
const VIAL_FULL_LOG10: f32 = 6.0;
/// Inner height of a vial: an 18px slot minus its 1px border.
const VIAL_HEIGHT: f32 = 16.0;

/// A workbench in the world.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BenchId {
    pub dimension: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BenchId {
    fn pos(&self) -> BlockPos {
        BlockPos {
            x: self.x,
            y: self.y,
            z: self.z,
        }
    }
}

/// What's on the Blueprint slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BlueprintView {
    None,
    Blank,
    Design {
        name: String,
        tier: Tier,
        ports: usize,
        blocks: usize,
        cost: Amounts,
    },
}

/// What a workbench screen shows — sent to the player who has it open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchState {
    pub bench: BenchId,
    pub stored: Amounts,
    pub blueprint: BlueprintView,
    /// The chip slot is empty (a fabricated chip can come out).
    pub chip_free: bool,
}

#[derive(Debug, Serialize, Deserialize)]
enum Request {
    /// `design`: a ready-made one to start from ([`designs`]).
    Create {
        bench: BenchId,
        name: String,
        tier: Tier,
        design: Option<String>,
    },
    Edit {
        bench: BenchId,
    },
    Fabricate {
        bench: BenchId,
    },
}

// ── The screen's view model ──────────────────────────────────────────────────

/// One line of the fabrication cost.
pub struct CostRow {
    pub text: String,
    pub color: u32,
}

#[derive(Default)]
pub struct WorkbenchVm {
    /// The open workbench, as the server last described it.
    state: Option<BenchState>,
    /// A new design's name, tier and what it starts as (an index into
    /// [`designs::all`], past the empty one).
    name: String,
    tier: Option<Tier>,
    template: usize,
}

impl ViewModel for WorkbenchVm {}

impl WorkbenchVm {
    /// The server's latest word on the open workbench.
    pub fn apply(&mut self, state: BenchState) {
        self.state = Some(state);
    }

    fn stored(&self, r: Resource) -> u64 {
        self.state.as_ref().map_or(0, |s| s.stored[r.index()])
    }
    fn design(&self) -> Option<(&str, Tier, usize, usize, &Amounts)> {
        match &self.state.as_ref()?.blueprint {
            BlueprintView::Design {
                name,
                tier,
                ports,
                blocks,
                cost,
            } => Some((name, *tier, *ports, *blocks, cost)),
            _ => None,
        }
    }

    pub fn amount(&self, r: Resource) -> String {
        r.shown(self.stored(r))
    }
    /// Fill height of `r`'s vial — log scale, full at a million.
    pub fn vial(&self, r: Resource) -> f32 {
        let n = (self.stored(r) / r.unit().max(1)) as f32;
        VIAL_HEIGHT * ((n + 1.0).log10() / VIAL_FULL_LOG10).min(1.0)
    }
    pub fn resource_tooltip(&self, r: Resource) -> String {
        let inputs: Vec<String> = r
            .inputs()
            .iter()
            .map(|(id, units)| {
                // A tag: its name (Iron Nuggets).
                let name = match id.strip_prefix('#') {
                    Some(tag) => yog_api::tag_name("item", tag),
                    None => crate::resources::item_name(id),
                };
                match units {
                    1 => name,
                    n => format!("{name} ×{n}"),
                }
            })
            .collect();
        Strings::screen_workbench_resource_tooltip(r.name(), self.stored(r), inputs.join(", "))
    }

    pub fn no_blueprint(&self) -> bool {
        self.state
            .as_ref()
            .is_none_or(|s| s.blueprint == BlueprintView::None)
    }
    pub fn blank(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|s| s.blueprint == BlueprintView::Blank)
    }
    pub fn has_design(&self) -> bool {
        self.design().is_some()
    }
    pub fn design_name(&self) -> String {
        self.design().map(|d| d.0.to_string()).unwrap_or_default()
    }
    pub fn design_tier(&self) -> String {
        self.design()
            .map(|(_, tier, ports, blocks, _)| {
                Strings::screen_workbench_design_tier(tier.name(), ports, blocks)
            })
            .unwrap_or_default()
    }
    pub fn cost_rows(&self) -> Vec<CostRow> {
        let Some((.., cost)) = self.design() else {
            return Vec::new();
        };
        let rows: Vec<CostRow> = Resource::ALL
            .into_iter()
            .filter(|r| cost[r.index()] > 0)
            .map(|r| {
                let (need, have) = (cost[r.index()], self.stored(r));
                CostRow {
                    text: Strings::screen_workbench_cost_row(
                        r.name(),
                        r.shown(need),
                        r.shown(have),
                    ),
                    color: if have >= need {
                        0xFF_404040
                    } else {
                        0xFF_B02020
                    },
                }
            })
            .collect();
        if rows.is_empty() {
            return vec![CostRow {
                text: Strings::screen_workbench_free(),
                color: 0xFF_404040,
            }];
        }
        rows
    }

    fn tier(&self) -> Tier {
        self.tier.unwrap_or(Tier::ALL[0])
    }
    pub fn tier_text(&self) -> String {
        Strings::screen_workbench_tier(self.tier().name())
    }
    pub fn next_tier(&mut self) {
        let i = Tier::ALL
            .iter()
            .position(|t| *t == self.tier())
            .unwrap_or(0);
        self.tier = Some(Tier::ALL[(i + 1) % Tier::ALL.len()]);
    }
    /// The ready-made design picked, if any.
    fn template(&self) -> Option<String> {
        self.template
            .checked_sub(1)
            .and_then(|i| designs::all().get(i).map(|d| d.name.clone()))
    }
    pub fn template_text(&self) -> String {
        Strings::screen_workbench_template(
            self.template()
                .unwrap_or_else(Strings::screen_workbench_template_empty),
        )
    }
    pub fn next_template(&mut self) {
        self.template = (self.template + 1) % (designs::all().len() + 1);
    }
    pub fn can_create(&self) -> bool {
        self.blank() && (!self.name.trim().is_empty() || self.template > 0)
    }
    pub fn can_fabricate(&self) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        let Some((.., cost)) = self.design() else {
            return false;
        };
        state.chip_free
            && Resource::ALL
                .iter()
                .all(|r| state.stored[r.index()] >= cost[r.index()])
    }

    pub fn create(&mut self) {
        if let Some(s) = &self.state {
            let design = self.template();
            // Unnamed, it's called what it starts as.
            let name = Some(self.name.trim().to_string())
                .filter(|n| !n.is_empty())
                .or(design.clone())
                .unwrap_or_default();
            send(&Request::Create {
                bench: s.bench.clone(),
                name,
                tier: self.tier(),
                design,
            });
            self.name.clear();
        }
    }
    pub fn edit(&mut self) {
        if let Some(s) = &self.state {
            send(&Request::Edit {
                bench: s.bench.clone(),
            });
        }
    }
    pub fn fabricate(&mut self) {
        if let Some(s) = &self.state {
            send(&Request::Fabricate {
                bench: s.bench.clone(),
            });
        }
    }
}

/// The game's name of item or block `id`.
fn send(request: &Request) {
    if let (Some(srv), Ok(json)) = (yog_api::server(), serde_json::to_string(request)) {
        srv.send_to_server(REQUEST_CHANNEL, json.as_bytes());
    }
}

// ── The server side ──────────────────────────────────────────────────────────

/// Each workbench's store.
static STORES: LazyLock<Mutex<HashMap<BenchId, Amounts>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Who has which workbench open, and what they were last sent.
type Watching = (BenchId, Option<BenchState>);
static WATCHERS: LazyLock<Mutex<HashMap<String, Watching>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static TICKS: Mutex<u64> = Mutex::new(0);

/// Moves what's in the resource slots of `bench` into its store.
fn absorb(srv: &dyn yog_api::Server, bench: &BenchId) {
    let mut gained = [0u64; 6];
    for r in Resource::ALL {
        let Some((item, count, _)) =
            srv.get_inventory_slot(&bench.dimension, bench.pos(), r.slot())
        else {
            continue;
        };
        if let Some(units) = r.units_of(srv, &item)
            && srv.set_inventory_slot(&bench.dimension, bench.pos(), r.slot(), &item, 0, "")
        {
            gained[r.index()] += units * count as u64;
        }
    }
    if gained.iter().any(|g| *g > 0) {
        let mut stores = STORES.lock().unwrap();
        let store = stores.entry(bench.clone()).or_default();
        for (s, g) in store.iter_mut().zip(gained) {
            *s = s.saturating_add(g);
        }
    }
}

fn blueprint_in(srv: &dyn yog_api::Server, bench: &BenchId) -> (BlueprintView, Option<Blueprint>) {
    match srv.get_inventory_slot(&bench.dimension, bench.pos(), BLUEPRINT_SLOT) {
        Some((id, _, nbt)) if id == BLUEPRINT_ID => match Blueprint::from_nbt(&nbt) {
            Some(bp) => {
                let view = BlueprintView::Design {
                    name: bp.name.clone(),
                    tier: bp.tier,
                    ports: bp.circuit.ports.len(),
                    blocks: bp.circuit.blocks.len(),
                    cost: crate::resources::circuit_cost(&bp.circuit, &|id| {
                        crate::chip::CircuitData::load_for_chip(&srv.world_dir(), id)
                    }),
                };
                (view, Some(bp))
            }
            None => (BlueprintView::Blank, None),
        },
        _ => (BlueprintView::None, None),
    }
}

fn state_of(srv: &dyn yog_api::Server, bench: &BenchId) -> BenchState {
    BenchState {
        bench: bench.clone(),
        stored: STORES
            .lock()
            .unwrap()
            .get(bench)
            .copied()
            .unwrap_or_default(),
        blueprint: blueprint_in(srv, bench).0,
        chip_free: srv
            .get_inventory_slot(&bench.dimension, bench.pos(), CHIP_SLOT)
            .is_none(),
    }
}

/// Every few ticks: swallow resources, and tell watchers what changed.
pub fn tick(srv: &dyn yog_api::Server) {
    let n = {
        let mut t = TICKS.lock().unwrap();
        *t += 1;
        *t
    };
    if n % TICK_EVERY != 0 {
        return;
    }
    let watchers: Vec<(String, BenchId)> = WATCHERS
        .lock()
        .unwrap()
        .iter()
        .map(|(p, (b, _))| (p.clone(), b.clone()))
        .collect();
    for (player, bench) in watchers {
        absorb(srv, &bench);
        let state = state_of(srv, &bench);
        let mut w = WATCHERS.lock().unwrap();
        let Some((_, last)) = w.get_mut(&player) else {
            continue;
        };
        if last.as_ref() != Some(&state) {
            if let Ok(json) = serde_json::to_string(&state) {
                srv.send_to_player(&player, STATE_CHANNEL, json.as_bytes());
            }
            *last = Some(state);
        }
    }
}

/// Runs a client's request; the reply goes to its action bar.
fn run_request(srv: &dyn yog_api::Server, player: &str, request: Request) -> String {
    match request {
        Request::Create {
            bench,
            name,
            tier,
            design,
        } => {
            if !matches!(blueprint_in(srv, &bench).0, BlueprintView::Blank) {
                return "§cPut a blank Blueprint in the workbench.".into();
            }
            let mut bp = Blueprint::new(name.clone(), tier);
            if let Some(design) = design {
                let Some(d) = designs::named(&design) else {
                    return format!("§cNo design '{design}'.");
                };
                bp.circuit = d.circuit(tier);
            }
            if !srv.set_inventory_slot(
                &bench.dimension,
                bench.pos(),
                BLUEPRINT_SLOT,
                BLUEPRINT_ID,
                1,
                &bp.to_nbt(),
            ) {
                return "§cCouldn't write the Blueprint.".into();
            }
            format!("§aCreated '{name}' ({} tier).", tier.name())
        }
        Request::Edit { bench } => {
            let Some(bp) = blueprint_in(srv, &bench).1 else {
                return "§cPut a written Blueprint in the workbench.".into();
            };
            let back = Player::new(srv, player).position().unwrap_or((
                bench.x as f64 + 0.5,
                bench.y as f64 + 1.0,
                bench.z as f64 + 0.5,
            ));
            let source = editor::Source {
                dimension: bench.dimension.clone(),
                pos: bench.pos(),
                slot: BLUEPRINT_SLOT,
            };
            match editor::enter(srv, player, &bp, source, &bench.dimension, back) {
                Ok(()) => String::new(),
                Err(e) => e,
            }
        }
        Request::Fabricate { bench } => fabricate(srv, &bench),
    }
}

fn fabricate(srv: &dyn yog_api::Server, bench: &BenchId) -> String {
    let Some(bp) = blueprint_in(srv, bench).1 else {
        return "§cPut a written Blueprint in the workbench.".into();
    };
    if srv
        .get_inventory_slot(&bench.dimension, bench.pos(), CHIP_SLOT)
        .is_some()
    {
        return "§cTake the chip out of the chip slot first.".into();
    }
    let cost = crate::resources::circuit_cost(&bp.circuit, &|id| {
        crate::chip::CircuitData::load_for_chip(&srv.world_dir(), id)
    });
    {
        let stores = STORES.lock().unwrap();
        let stored = stores.get(bench).copied().unwrap_or_default();
        let missing: Vec<String> = Resource::ALL
            .iter()
            .filter(|r| stored[r.index()] < cost[r.index()])
            .map(|r| {
                format!(
                    "{} {}",
                    r.shown(cost[r.index()] - stored[r.index()]),
                    r.name()
                )
            })
            .collect();
        if !missing.is_empty() {
            return format!("§cNot enough: {}.", missing.join(", "));
        }
    }
    let meta = ChipMeta {
        id: crate::chip::new_id(),
        tier: bp.tier,
        name: bp.name.clone(),
        ports: bp.circuit.ports.clone(),
    };
    bp.circuit.save_for_chip(&srv.world_dir(), &meta.id);
    if !srv.set_inventory_slot(
        &bench.dimension,
        bench.pos(),
        CHIP_SLOT,
        bp.tier.chip(),
        1,
        &meta.to_nbt(),
    ) {
        return "§cCouldn't put the chip in the chip slot.".into();
    }
    let mut stores = STORES.lock().unwrap();
    let store = stores.entry(bench.clone()).or_default();
    for (s, c) in store.iter_mut().zip(cost) {
        *s -= c;
    }
    format!(
        "§aFabricated '{}' ({} tier, {} ports).",
        meta.name,
        bp.tier.name(),
        meta.ports.len()
    )
}

/// The block, its screen, and the server side.
pub fn setup(registry: &mut Registry) {
    // The screen's markup lays out the block's inventory
    // (content/vlsi_workbench.yblock opens it).
    let screen = register(registry);

    registry.on_client_packet(STATE_CHANNEL, move |ev, _srv| {
        if let Ok(state) = serde_json::from_slice::<BenchState>(&ev.payload) {
            screen.update(move |vm| vm.apply(state));
        }
    });

    registry.on_packet(REQUEST_CHANNEL, |ev, srv| {
        let Ok(request) = serde_json::from_slice::<Request>(&ev.payload) else {
            return;
        };
        let reply = run_request(srv, &ev.player, request);
        if !reply.is_empty() {
            srv.send_actionbar(&ev.player, &reply);
        }
        // Whatever changed shows on the next look.
        if let Some((_, last)) = WATCHERS.lock().unwrap().get_mut(&ev.player) {
            *last = None;
        }
    });

    // Opening a workbench: watch it from now on.
    registry.on_use_block(|e, phase, _srv| -> bool {
        if phase == EventPhase::Pre && e.block_id == WORKBENCH_ID {
            let bench = BenchId {
                dimension: e.dimension.clone(),
                x: e.pos.x,
                y: e.pos.y,
                z: e.pos.z,
            };
            WATCHERS
                .lock()
                .unwrap()
                .insert(e.player_name.clone(), (bench, None));
        }
        true // the inventory opens
    });
}

const STORE_FILE: &str = "yog-vlsi/workbenches";
const STORE_KEY: &str = "stores";

pub fn load_resources(srv: &dyn yog_api::Server) {
    let store = Storage::open(&srv.world_dir(), STORE_FILE);
    let saved: Vec<(BenchId, Amounts)> = store
        .get(STORE_KEY)
        .and_then(|v| v.as_str())
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    *STORES.lock().unwrap() = saved.into_iter().collect();
}

pub fn save_resources(srv: &dyn yog_api::Server) {
    let stores: Vec<(BenchId, Amounts)> = STORES
        .lock()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    if let Ok(json) = serde_json::to_string(&stores) {
        let mut store = Storage::open(&srv.world_dir(), STORE_FILE);
        store.set(STORE_KEY, &*json);
        let _ = store.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(stored: Amounts, blueprint: BlueprintView) -> BenchState {
        BenchState {
            bench: BenchId {
                dimension: "minecraft:overworld".into(),
                x: 0,
                y: 0,
                z: 0,
            },
            stored,
            blueprint,
            chip_free: true,
        }
    }

    #[test]
    fn slots_fit_the_screen_apart() {
        let def = yog_kit::inventory_of::<View>().unwrap();
        assert_eq!(def.slot_count, 8);
        assert!(def.include_player_inventory);
        let (px, py) = def.player_inv_offset;
        // The player's inventory (9×18 by 3×18 + 4 + 18) stays on screen.
        assert!(px + 9.0 * 18.0 <= WIDTH && py + 76.0 <= HEIGHT, "{px},{py}");
        for (i, a) in def.layout.iter().enumerate() {
            assert!(
                a.x >= 0.0 && a.y >= 0.0 && a.x + 16.0 <= WIDTH && a.y + 16.0 < py,
                "slot {i} at {a:?}"
            );
            for b in &def.layout[i + 1..] {
                assert!(
                    (a.x - b.x).abs() >= 18.0 || (a.y - b.y).abs() >= 18.0,
                    "{a:?} overlaps {b:?}"
                );
            }
        }
    }

    #[test]
    fn blank_blueprint_then_design() {
        let mut h = yog_kit::testing::TestHost::<View>::new(WorkbenchVm::default());
        h.open();
        assert!(!h.widget(ids::FABRICATE).unwrap().enabled);

        let blank = state([0; 6], BlueprintView::Blank);
        h.handle().update(move |vm| vm.apply(blank));
        h.frame();
        h.type_into(ids::NAME, "Adder");
        assert!(h.vm().can_create());

        let cost = [3, 0, 0, 0, 0, 1];
        h.vm_mut().apply(state(
            [5, 0, 0, 0, 0, 0],
            BlueprintView::Design {
                name: "Adder".into(),
                tier: Tier::ALL[0],
                ports: 2,
                blocks: 4,
                cost,
            },
        ));
        assert!(!h.vm().can_fabricate(), "no stone stored");
        h.vm_mut().apply(state(
            [5, 0, 0, 0, 0, 1],
            BlueprintView::Design {
                name: "Adder".into(),
                tier: Tier::ALL[0],
                ports: 2,
                blocks: 4,
                cost,
            },
        ));
        assert!(h.vm().can_fabricate());
        assert_eq!(h.vm().cost_rows().len(), 2);
    }

    /// The screen's resource slots take what the resources' table says they're worth.
    #[test]
    fn resource_slots_take_the_resources_inputs() {
        let def = yog_kit::inventory_of::<View>().unwrap();
        for r in Resource::ALL {
            let mut accepts = def.layout[r.slot() as usize].accepts.clone();
            let mut inputs: Vec<String> = r.inputs().iter().map(|(i, _)| i.to_string()).collect();
            accepts.sort();
            inputs.sort();
            assert_eq!(accepts, inputs, "{}", r.name());
        }
    }

    #[test]
    fn vials_fill_on_a_log_scale() {
        let mut vm = WorkbenchVm::default();
        // Iron and gold count nuggets: 9 ingots, 999 999 ingots.
        vm.apply(state(
            [0, 81, 8_999_991, 0, 0, 5_000_000],
            BlueprintView::None,
        ));
        assert_eq!(vm.vial(Resource::Redstone), 0.0);
        assert!(vm.vial(Resource::Iron) > 0.0 && vm.vial(Resource::Iron) < VIAL_HEIGHT / 2.0);
        assert!((vm.vial(Resource::Gold) - VIAL_HEIGHT).abs() < 0.01);
        assert_eq!(vm.vial(Resource::Stone), VIAL_HEIGHT);
        assert_eq!(vm.amount(Resource::Stone), "5m");
        assert_eq!(vm.amount(Resource::Iron), "9");
        assert_eq!(
            (
                Resource::Iron.shown(21),
                Resource::Iron.shown(5),
                Resource::Redstone.shown(21)
            ),
            ("2.3".into(), "0.5".into(), "21".into()),
            "an ingot is 9 nuggets"
        );
    }
}
