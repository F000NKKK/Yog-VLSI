//! An ALU's board: the chips in its slots and the wires between their pins.
//!
//! The graph (DESIGN.md §4.6) has three kinds of node:
//!
//! - **In** — the ALU's six sides as *sources*: the level arriving from the
//!   cable on that side.
//! - **Out** — the same six sides as *sinks*: a wire into one drives that
//!   level out into the side's cable. A side is one or the other: wiring
//!   into `Out:N` makes N an output, and `In:N` goes dark.
//! - **Chip** — one per occupied chip slot: its input pins (sinks) and
//!   output pins (sources) as the ports were named in the editor; a
//!   bidirectional pin is both.
//! - **Bus** — one per side a digital cable reaches: its channels (`1`…,
//!   as many as the ALU's tier sees), each both a source (what's on the
//!   bus) and a sink (what this ALU drives onto it). The node shows as many
//!   rows as the player pulled it open to.
//!
//! A wire runs from a source to a sink. A source may feed any number of
//! sinks; a sink takes one wire. Chip → chip wires stay inside the ALU;
//! In → chip brings a signal in, chip → Out sends one out, In → Out passes
//! it straight through.
//!
//! The server owns all of it. The screen (`alu.rs`) is sent a [`BoardView`]
//! while it's open and asks for changes with [`Request`]s.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use yog_api::{BlockPos, EventPhase, Registry, Server, Storage};

use crate::cables;
use crate::chip::{ChipMeta, CircuitData};
use crate::content::Tier;
use crate::vm::{NestedPin, PortMode, RedstoneVM};

/// The six sides, in the order the graph lists them.
pub const SIDES: [&str; 6] = ["N", "S", "E", "W", "U", "D"];
/// Chip slots in an ALU's inventory (a tier uses the first `max_chips`).
pub const CHIP_SLOTS: u8 = 8;

/// Server → client: the board of the ALU being looked at.
pub const VIEW_CHANNEL: &str = "yog-vlsi:board";
/// Client → server: a change to it.
pub const REQUEST_CHANNEL: &str = "yog-vlsi:board_request";

/// How often the chip slots are looked at, in server ticks.
const SYNC_EVERY: u64 = 10;
/// How often watchers are sent what changed, in server ticks.
const SEND_EVERY: u64 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AluId {
    pub dimension: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl AluId {
    pub fn pos(&self) -> BlockPos {
        BlockPos { x: self.x, y: self.y, z: self.z }
    }
    fn xyz(&self) -> (i32, i32, i32) {
        (self.x, self.y, self.z)
    }
}

/// A node of the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Node {
    In,
    Out,
    Chip(u8),
    /// The bus on side [`SIDES`]`[i]`.
    Bus(u8),
}

/// A pin: a side of `In`/`Out` (`"N"`, …) or a chip's port label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pin {
    pub node: Node,
    pub name: String,
}

impl Pin {
    pub fn new(node: Node, name: impl Into<String>) -> Self {
        Pin { node, name: name.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wire {
    pub from: Pin,
    pub to: Pin,
}

/// What's saved of a board.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Saved {
    tier: Tier,
    wires: Vec<Wire>,
    /// Where the player put each chip slot's node.
    layout: Vec<(u8, (f32, f32))>,
    /// Where the player put the In and Out panels (and the buses).
    #[serde(default)]
    panels: Vec<(Node, (f32, f32))>,
    /// How many channel rows each bus node shows, by side.
    #[serde(default)]
    bus_rows: Vec<(u8, u16)>,
}

/// Rows a bus node shows until it's pulled open.
const BUS_ROWS: u16 = 4;

/// A chip sitting in a slot.
struct Installed {
    meta: ChipMeta,
    circuit: CircuitData,
    vm: RedstoneVM,
}

struct Board {
    saved: Saved,
    chips: Vec<Option<Installed>>,
    /// The level on every source pin, and on every driven `Out` side.
    levels: HashMap<Pin, u8>,
    /// The name of the Redstone Port on each side's cable.
    side_names: HashMap<String, String>,
    /// The cable on each side (`cables::ANALOG`, `DIGITAL`), looked at when
    /// cables change and every [`SYNC_EVERY`] ticks — not every tick.
    cables: [Option<&'static str>; 6],
    /// `cables` has been looked at.
    cables_known: bool,
    /// What this ALU last drove onto each bus channel `(side, channel)`.
    bus_driven: HashMap<(u8, u8), u8>,
}

impl Board {
    fn new(tier: Tier) -> Self {
        Board {
            saved: Saved { tier, wires: Vec::new(), layout: Vec::new(), panels: Vec::new(), bus_rows: Vec::new() },
            chips: (0..CHIP_SLOTS).map(|_| None).collect(),
            levels: HashMap::new(),
            side_names: HashMap::new(),
            cables: [None; 6],
            cables_known: false,
            bus_driven: HashMap::new(),
        }
    }

    fn side_is_output(&self, side: &str) -> bool {
        self.saved.wires.iter().any(|w| w.to.node == Node::Out && w.to.name == side)
    }

    fn active(&self, slot: u8) -> Option<&Installed> {
        if slot as usize >= self.saved.tier.max_chips() {
            return None;
        }
        self.chips.get(slot as usize)?.as_ref()
    }

    /// The mode of a chip pin (`None`: no such pin on an active chip).
    fn chip_pin(&self, slot: u8, name: &str) -> Option<PortMode> {
        self.active(slot)?.meta.ports.iter().find(|p| p.label == name).map(|p| p.dir)
    }

    /// How many channels of a bus this ALU's tier sees.
    fn bus_channels(&self) -> u16 {
        (self.saved.tier.digital_channels() as u16 * 4).min(cables::BUS_CHANNELS)
    }

    /// The channel (0-based) a pin of bus `side` names, if the bus is there and the tier sees it.
    fn bus_channel(&self, side: u8, name: &str) -> Option<u8> {
        let n: u16 = name.parse().ok()?;
        (self.is_bus(side as usize) && (1..=self.bus_channels()).contains(&n)).then(|| (n - 1) as u8)
    }

    fn bus_rows(&self, side: u8) -> u16 {
        self.saved.bus_rows.iter().find(|(s, _)| *s == side).map_or(BUS_ROWS, |(_, r)| *r).clamp(1, self.bus_channels())
    }

    fn is_source(&self, pin: &Pin) -> bool {
        match pin.node {
            Node::In => SIDES.contains(&pin.name.as_str()) && !self.side_is_output(&pin.name),
            Node::Out => false,
            Node::Chip(slot) => self.chip_pin(slot, &pin.name).is_some_and(PortMode::outputs),
            Node::Bus(side) => self.bus_channel(side, &pin.name).is_some(),
        }
    }

    fn is_sink(&self, pin: &Pin) -> bool {
        match pin.node {
            Node::In => false,
            Node::Out => SIDES.contains(&pin.name.as_str()),
            Node::Chip(slot) => self.chip_pin(slot, &pin.name).is_some_and(PortMode::inputs),
            Node::Bus(side) => self.bus_channel(side, &pin.name).is_some(),
        }
    }

    fn apply(&mut self, request: Request) -> Result<(), String> {
        match request {
            Request::Connect { from, to, .. } => {
                if !self.is_source(&from) || !self.is_sink(&to) {
                    return Err("§cA wire runs from an output pin to an input pin.".into());
                }
                // A sink takes one wire; a side that becomes an output stops
                // being an input.
                self.saved.wires.retain(|w| w.to != to);
                if to.node == Node::Out {
                    self.saved.wires.retain(|w| !(w.from.node == Node::In && w.from.name == to.name));
                }
                self.saved.wires.push(Wire { from, to });
            }
            Request::Disconnect { to, .. } => self.saved.wires.retain(|w| w.to != to),
            Request::Clear { .. } => self.saved.wires.clear(),
            Request::Move { node: Node::Chip(slot), x, y, .. } => {
                self.saved.layout.retain(|(s, _)| *s != slot);
                self.saved.layout.push((slot, (x, y)));
            }
            Request::Move { node, x, y, .. } => {
                self.saved.panels.retain(|(n, _)| *n != node);
                self.saved.panels.push((node, (x, y)));
            }
            Request::BusRows { side, rows, .. } => {
                self.saved.bus_rows.retain(|(s, _)| *s != side);
                self.saved.bus_rows.push((side, rows.max(1)));
            }
        }
        Ok(())
    }

    /// Looks at the chip slots: new chips get a VM, removed ones lose it.
    fn sync_chips(&mut self, srv: &dyn yog_api::Server, alu: &AluId) {
        for slot in 0..CHIP_SLOTS {
            let meta = srv
                .get_inventory_slot(&alu.dimension, alu.pos(), slot as u32)
                .filter(|(id, _, _)| Tier::from_chip(id).is_some())
                .and_then(|(_, _, nbt)| ChipMeta::from_nbt(&nbt));
            let entry = &mut self.chips[slot as usize];
            match meta {
                Some(meta) if entry.as_ref().is_some_and(|c| c.meta.id == meta.id) => {}
                Some(meta) => {
                    let circuit = CircuitData::load_for_chip(&srv.game_dir(), &meta.id)
                        .unwrap_or_else(|| CircuitData::empty(meta.tier));
                    // Its ALUs run inside it, their chips' circuits from the store.
                    let game_dir = srv.game_dir();
                    let vm = circuit.vm_with(&|id| CircuitData::load_for_chip(&game_dir, id), 1);
                    *entry = Some(Installed { meta, circuit, vm });
                }
                None => *entry = None,
            }
        }
    }

    /// Which sides a digital cable reaches now.
    fn sync_cables(&mut self, srv: &dyn yog_api::Server, alu: &AluId) {
        // The first look: cables written into the world while the server was
        // down are joined up.
        if !self.cables_known {
            cables::refresh_around(srv, &alu.dimension, alu.pos());
        }
        for (i, side) in SIDES.iter().enumerate() {
            self.cables[i] = cables::cable_at(srv, &alu.dimension, cables::beside(alu.xyz(), side));
        }
        self.cables_known = true;
    }

    /// A digital cable reaches side `i`: it's a bus.
    fn is_bus(&self, i: usize) -> bool {
        self.cables[i] == Some(cables::DIGITAL)
    }

    /// What every source drives now: the inputs coming into the ALU's sides,
    /// the chips' output ports and the bus channels shown or wired.
    fn sources(&self, alu: &AluId) -> HashMap<Pin, u8> {
        let max = self.saved.tier.max_chips();
        let mut levels = HashMap::new();
        for (i, &side) in SIDES.iter().enumerate() {
            if !self.side_is_output(side) {
                let level = cables::side_input(&alu.dimension, alu.xyz(), side, self.cables[i]).unwrap_or(0);
                levels.insert(Pin::new(Node::In, side), level);
            }
        }
        for (slot, chip) in self.chips.iter().enumerate().take(max) {
            let Some(chip) = chip else { continue };
            for port in chip.circuit.ports.iter().filter(|p| p.dir.outputs()) {
                levels.insert(Pin::new(Node::Chip(slot as u8), &port.label), chip.vm.get_port_output(port.x, port.y, port.z));
            }
        }
        for (i, side) in SIDES.iter().enumerate().filter(|(i, _)| self.is_bus(*i)) {
            let shown = (1..=self.bus_rows(i as u8)).map(|n| n.to_string());
            let wired = self.saved.wires.iter().filter(|w| w.from.node == Node::Bus(i as u8)).map(|w| w.from.name.clone());
            for name in shown.chain(wired) {
                if let Some(ch) = self.bus_channel(i as u8, &name) {
                    levels.insert(Pin::new(Node::Bus(i as u8), name), cables::bus_level(&alu.dimension, alu.xyz(), side, ch));
                }
            }
        }
        levels
    }

    /// One server tick: every wire carries its source's level into the
    /// chips, the chips step with it, and what they put out leaves by the
    /// output sides the same tick — a signal crosses an ALU in the chips'
    /// own steps, no tick lost waiting on either side of them.
    fn tick(&mut self, alu: &AluId) {
        let max = self.saved.tier.max_chips();
        let incoming = |wires: &[Wire], levels: &HashMap<Pin, u8>, to: &Pin| {
            wires.iter().find(|w| &w.to == to).and_then(|w| levels.get(&w.from)).copied().unwrap_or(0)
        };

        // In: the sides' inputs and the other chips' outputs, into each chip.
        let before = self.sources(alu);
        let mut inputs = Vec::new();
        for (slot, chip) in self.chips.iter().enumerate().take(max) {
            let Some(chip) = chip else { continue };
            for port in chip.circuit.ports.iter().filter(|p| p.dir.inputs()) {
                inputs.push((slot, port.x, port.y, port.z, incoming(&self.saved.wires, &before, &Pin::new(Node::Chip(slot as u8), &port.label))));
            }
        }
        for (slot, x, y, z, level) in inputs {
            if let Some(chip) = &mut self.chips[slot] {
                chip.vm.set_port_input(x, y, z, level);
            }
        }

        // The chips settle with them: as many steps as they need, up to
        // their tier's — they answer this tick.
        for chip in self.chips.iter_mut().take(max).flatten() {
            chip.vm.new_tick();
            chip.vm.settle(chip.meta.tier.steps_per_tick());
        }

        // Out: what they put out now, through the output sides.
        let mut levels = self.sources(alu);
        let mut driven = Vec::new();
        for (i, &side) in SIDES.iter().enumerate() {
            if self.side_is_output(side) {
                let level = incoming(&self.saved.wires, &levels, &Pin::new(Node::Out, side));
                cables::side_output(&alu.dimension, alu.xyz(), side, self.cables[i], level);
                driven.push((Pin::new(Node::Out, side), level));
            }
        }
        // And onto the buses: what's wired into their channels; a channel
        // nothing drives any more goes back to 0.
        let mut now: HashMap<(u8, u8), u8> = HashMap::new();
        for w in self.saved.wires.iter() {
            if let Node::Bus(side) = w.to.node
                && let Some(ch) = self.bus_channel(side, &w.to.name)
            {
                now.insert((side, ch), levels.get(&w.from).copied().unwrap_or(0));
            }
        }
        for (&(side, ch), &level) in &now {
            if self.bus_driven.get(&(side, ch)) != Some(&level) {
                cables::bus_drive(&alu.dimension, alu.xyz(), SIDES[side as usize], ch, level);
            }
        }
        for &(side, ch) in self.bus_driven.keys().filter(|k| !now.contains_key(k)) {
            cables::bus_drive(&alu.dimension, alu.xyz(), SIDES[side as usize], ch, 0);
        }
        self.bus_driven = now;
        levels.extend(driven);
        self.levels = levels;
    }

    fn view(&self, alu: &AluId) -> BoardView {
        BoardView {
            alu: alu.clone(),
            tier: self.saved.tier,
            chips: self
                .chips
                .iter()
                .enumerate()
                .filter_map(|(slot, c)| {
                    let c = c.as_ref()?;
                    Some(ChipView {
                        slot: slot as u8,
                        name: c.meta.name.clone(),
                        tier: c.meta.tier,
                        pins: c.meta.ports.iter().map(|p| (p.label.clone(), p.dir)).collect(),
                        active: slot < self.saved.tier.max_chips(),
                    })
                })
                .collect(),
            // A side shows where an analog cable reaches it — or a wire
            // still does (to take it off).
            sides: SIDES
                .iter()
                .enumerate()
                .filter(|&(i, &side)| {
                    self.cables[i] == Some(cables::ANALOG)
                        || self.saved.wires.iter().any(|w| {
                            [&w.from, &w.to].iter().any(|p| matches!(p.node, Node::In | Node::Out) && p.name == side)
                        })
                })
                .map(|(_, &side)| SideView {
                    side: side.to_string(),
                    name: self.side_names.get(side).cloned(),
                    output: self.side_is_output(side),
                })
                .collect(),
            buses: (0..SIDES.len() as u8)
                .filter(|&i| self.is_bus(i as usize))
                .map(|i| BusView { index: i, side: SIDES[i as usize].to_string(), channels: self.bus_channels(), rows: self.bus_rows(i) })
                .collect(),
            wires: self.saved.wires.clone(),
            layout: self.saved.layout.iter().map(|&(s, p)| (Node::Chip(s), p)).chain(self.saved.panels.iter().copied()).collect(),
            levels: self.levels.iter().map(|(p, l)| (p.clone(), *l)).collect(),
        }
    }
}

// ── What the screen gets and sends ──────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChipView {
    pub slot: u8,
    pub name: String,
    pub tier: Tier,
    pub pins: Vec<(String, PortMode)>,
    /// In a slot the ALU's tier powers.
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SideView {
    pub side: String,
    /// The Redstone Port at the other end of its cable.
    pub name: Option<String>,
    pub output: bool,
}

/// A bus node: the side it's on, how many channels the tier sees, how
/// many rows it shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusView {
    pub index: u8,
    pub side: String,
    pub channels: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardView {
    pub alu: AluId,
    pub tier: Tier,
    pub chips: Vec<ChipView>,
    pub sides: Vec<SideView>,
    pub buses: Vec<BusView>,
    pub wires: Vec<Wire>,
    /// Where the player put the nodes (chips, the In and Out panels).
    pub layout: Vec<(Node, (f32, f32))>,
    pub levels: Vec<(Pin, u8)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Connect { alu: AluId, from: Pin, to: Pin },
    Disconnect { alu: AluId, to: Pin },
    Clear { alu: AluId },
    Move { alu: AluId, node: Node, x: f32, y: f32 },
    /// Pulls a bus node open to `rows` channels (or closes it to them).
    BusRows { alu: AluId, side: u8, rows: u16 },
}

impl Request {
    fn alu(&self) -> &AluId {
        match self {
            Request::Connect { alu, .. }
            | Request::Disconnect { alu, .. }
            | Request::Clear { alu }
            | Request::Move { alu, .. }
            | Request::BusRows { alu, .. } => alu,
        }
    }
}

/// Sends `request` to the server (from the screen).
pub fn send(request: &Request) {
    if let (Some(srv), Ok(json)) = (yog_api::server(), serde_json::to_string(request)) {
        srv.send_to_server(REQUEST_CHANNEL, json.as_bytes());
    }
}

// ── The server side ──────────────────────────────────────────────────────────

static BOARDS: LazyLock<Mutex<HashMap<AluId, Board>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn side_index(name: &str) -> Option<u8> {
    SIDES.iter().position(|s| *s == name).map(|i| i as u8)
}

/// A board pin as a nested ALU's (`None` for a bus: no cables inside a chip).
fn nested_pin(pin: &Pin) -> Option<NestedPin> {
    Some(match pin.node {
        Node::In => NestedPin::In(side_index(&pin.name)?),
        Node::Out => NestedPin::Out(side_index(&pin.name)?),
        Node::Chip(slot) => NestedPin::Chip(slot, pin.name.clone()),
        // A bus pin is its channel's number, from 1.
        Node::Bus(side) => NestedPin::Bus(side, pin.name.parse::<u16>().ok()?.checked_sub(1)?),
    })
}

/// The wires of the ALU at `alu`, as an ALU inside a chip keeps them (a
/// design is saved with it in it).
pub fn nested_wires(alu: &AluId) -> Vec<(NestedPin, NestedPin)> {
    BOARDS
        .lock()
        .unwrap()
        .get(alu)
        .map(|b| b.saved.wires.iter().filter_map(|w| Some((nested_pin(&w.from)?, nested_pin(&w.to)?))).collect())
        .unwrap_or_default()
}

/// Gives the ALU at `alu` the board `wires` (a design with an ALU in it
/// opened in the editor).
pub fn restore(alu: AluId, tier: Tier, wires: &[(NestedPin, NestedPin)]) {
    let pin = |p: &NestedPin| match p {
        NestedPin::In(i) => Pin::new(Node::In, SIDES[*i as usize % 6]),
        NestedPin::Out(i) => Pin::new(Node::Out, SIDES[*i as usize % 6]),
        NestedPin::Chip(slot, name) => Pin::new(Node::Chip(*slot), name.as_str()),
        NestedPin::Bus(side, ch) => Pin::new(Node::Bus(*side), (ch + 1).to_string()),
    };
    let mut board = Board::new(tier);
    board.saved.wires = wires.iter().map(|(f, t)| Wire { from: pin(f), to: pin(t) }).collect();
    BOARDS.lock().unwrap().insert(alu, board);
}

/// Forgets the boards of the ALUs inside `lo`..`hi` of `dimension` (an
/// editor field going).
pub fn forget_in(dimension: &str, lo: BlockPos, hi: BlockPos) {
    BOARDS.lock().unwrap().retain(|a, _| {
        !(a.dimension == dimension && (lo.x..=hi.x).contains(&a.x) && (lo.y..=hi.y).contains(&a.y) && (lo.z..=hi.z).contains(&a.z))
    });
}
/// Who has which ALU open, and what they were last sent.
type Watching = (AluId, Option<BoardView>);
static WATCHERS: LazyLock<Mutex<HashMap<String, Watching>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static TICKS: Mutex<u64> = Mutex::new(0);

pub fn tick(srv: &dyn yog_api::Server) {
    let n = {
        let mut t = TICKS.lock().unwrap();
        *t += 1;
        *t
    };
    let names = if n % SYNC_EVERY == 0 { Some(cables::port_names(srv)) } else { None };
    let cables_changed = cables::cables_changed();
    {
        let mut boards = BOARDS.lock().unwrap();
        for (alu, board) in boards.iter_mut() {
            if names.is_some() || cables_changed || !board.cables_known {
                board.sync_cables(srv, alu);
            }
            if let Some(names) = &names {
                board.sync_chips(srv, alu);
                board.side_names = SIDES
                    .iter()
                    .filter_map(|&side| Some((side.to_string(), cables::side_port_name(srv, alu, side, names)?)))
                    .collect();
            }
            board.tick(alu);
        }
    }
    if n % SEND_EVERY == 0 {
        send_views(srv);
    }
}

fn send_views(srv: &dyn yog_api::Server) {
    let boards = BOARDS.lock().unwrap();
    let mut watchers = WATCHERS.lock().unwrap();
    for (player, (alu, last)) in watchers.iter_mut() {
        let Some(board) = boards.get(alu) else { continue };
        let view = board.view(alu);
        if last.as_ref() != Some(&view) {
            if let Ok(json) = serde_json::to_string(&view) {
                srv.send_to_player(player, VIEW_CHANNEL, json.as_bytes());
            }
            *last = Some(view);
        }
    }
}

const STORE_FILE: &str = "yog-vlsi/boards";
const STORE_KEY: &str = "boards";

pub fn load(srv: &dyn yog_api::Server) {
    let store = Storage::open(&srv.game_dir(), STORE_FILE);
    let saved: Vec<(AluId, Saved)> = store
        .get(STORE_KEY)
        .and_then(|v| v.as_str())
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    let mut boards = BOARDS.lock().unwrap();
    boards.clear();
    for (alu, saved) in saved {
        let mut board = Board::new(saved.tier);
        board.saved = saved;
        board.sync_chips(srv, &alu);
        boards.insert(alu, board);
    }
}

pub fn save(srv: &dyn yog_api::Server) {
    let saved: Vec<(AluId, Saved)> = BOARDS.lock().unwrap().iter().map(|(k, b)| (k.clone(), b.saved.clone())).collect();
    if let Ok(json) = serde_json::to_string(&saved) {
        let mut store = Storage::open(&srv.game_dir(), STORE_FILE);
        store.set(STORE_KEY, &*json);
        let _ = store.flush();
    }
}

pub fn setup(registry: &mut Registry) {
    // Opening an ALU: it gets a board (if new) and the player watches it.
    registry.on_use_block(|e, phase, srv| -> bool {
        let Some(tier) = Tier::from_alu(&e.block_id) else { return true };
        if phase != EventPhase::Pre {
            return true;
        }
        let alu = AluId { dimension: e.dimension.clone(), x: e.pos.x, y: e.pos.y, z: e.pos.z };
        {
            let mut boards = BOARDS.lock().unwrap();
            let board = boards.entry(alu.clone()).or_insert_with(|| Board::new(tier));
            board.saved.tier = tier;
            board.sync_chips(srv, &alu);
        }
        WATCHERS.lock().unwrap().insert(e.player_name.clone(), (alu, None));
        true // its inventory opens
    });

    registry.on_block_break(|e, phase, _srv| {
        if phase == EventPhase::Post && Tier::from_alu(&e.block_id).is_some() {
            let alu = AluId { dimension: e.dimension.clone(), x: e.pos.x, y: e.pos.y, z: e.pos.z };
            BOARDS.lock().unwrap().remove(&alu);
        }
        true
    });

    registry.on_packet(REQUEST_CHANNEL, |ev, srv| {
        let Ok(request) = serde_json::from_slice::<Request>(&ev.payload) else { return };
        let alu = request.alu().clone();
        let result = BOARDS.lock().unwrap().get_mut(&alu).map(|b| b.apply(request));
        if let Some(Err(e)) = result {
            srv.send_actionbar(&ev.player, &e);
        }
        if let Some((_, last)) = WATCHERS.lock().unwrap().get_mut(&ev.player) {
            *last = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chip::Port;

    fn chip(slot: u8, ports: &[(&str, PortMode)]) -> (u8, Installed) {
        let ports: Vec<Port> =
            ports.iter().enumerate().map(|(i, (l, d))| Port { label: l.to_string(), x: i as u32, y: 0, z: 0, dir: *d }).collect();
        let circuit = CircuitData { size: 16, blocks: Vec::new(), ports: ports.clone(), alus: Vec::new() };
        let meta = ChipMeta { id: format!("c{slot}"), tier: Tier::ALL[0], name: "Chip".into(), ports };
        (slot, Installed { vm: circuit.vm(), circuit, meta })
    }

    fn board() -> Board {
        let mut b = Board::new(Tier::ALL[0]);
        for (slot, c) in [chip(0, &[("A", PortMode::Input), ("Q", PortMode::Output)]), chip(1, &[("D", PortMode::Bidirectional)])] {
            b.chips[slot as usize] = Some(c);
        }
        b
    }

    fn connect(b: &mut Board, from: Pin, to: Pin) -> Result<(), String> {
        let alu = AluId { dimension: String::new(), x: 0, y: 0, z: 0 };
        b.apply(Request::Connect { alu, from, to })
    }

    #[test]
    fn wires_run_from_sources_to_sinks() {
        let mut b = board();
        assert!(connect(&mut b, Pin::new(Node::Chip(0), "Q"), Pin::new(Node::Chip(1), "D")).is_ok());
        assert!(connect(&mut b, Pin::new(Node::Chip(1), "D"), Pin::new(Node::Chip(0), "A")).is_ok(), "bidi is both");
        assert!(connect(&mut b, Pin::new(Node::Chip(0), "A"), Pin::new(Node::Out, "N")).is_err(), "A is an input");
        assert!(connect(&mut b, Pin::new(Node::In, "N"), Pin::new(Node::Out, "X")).is_err(), "no side X");
        assert_eq!(b.saved.wires.len(), 2);
    }

    #[test]
    fn a_sink_takes_one_wire() {
        let mut b = board();
        connect(&mut b, Pin::new(Node::In, "N"), Pin::new(Node::Chip(0), "A")).unwrap();
        connect(&mut b, Pin::new(Node::In, "S"), Pin::new(Node::Chip(0), "A")).unwrap();
        assert_eq!(b.saved.wires, [Wire { from: Pin::new(Node::In, "S"), to: Pin::new(Node::Chip(0), "A") }]);
    }

    #[test]
    fn a_side_is_an_input_or_an_output() {
        let mut b = board();
        connect(&mut b, Pin::new(Node::In, "N"), Pin::new(Node::Chip(0), "A")).unwrap();
        connect(&mut b, Pin::new(Node::Chip(0), "Q"), Pin::new(Node::Out, "N")).unwrap();
        assert_eq!(b.saved.wires.len(), 1, "In:N went dark");
        assert!(connect(&mut b, Pin::new(Node::In, "N"), Pin::new(Node::Chip(1), "D")).is_err());
    }

    #[test]
    fn chips_past_the_tier_limit_sleep() {
        let mut b = board();
        let max = b.saved.tier.max_chips() as u8;
        let (_, c) = chip(max, &[("Z", PortMode::Output)]);
        b.chips[max as usize] = Some(c);
        assert!(connect(&mut b, Pin::new(Node::Chip(max), "Z"), Pin::new(Node::Chip(0), "A")).is_err());
    }

    #[test]
    fn a_bus_has_as_many_channels_as_the_tier_sees_each_both_ways() {
        let mut b = Board::new(Tier::Wood);
        let ch = |n: &str| Pin::new(Node::Bus(2), n);
        assert!(!b.is_source(&ch("1")), "no digital cable on E yet");
        b.cables[2] = Some(cables::DIGITAL);
        assert_eq!(b.bus_channels(), 32, "wood: 8 × 4 colours");
        assert!(b.is_source(&ch("1")) && b.is_sink(&ch("1")));
        assert!(b.is_sink(&ch("32")) && !b.is_sink(&ch("33")) && !b.is_source(&ch("0")));
        assert_eq!(b.bus_channel(2, "32"), Some(31), "pins count from 1, Pipes' channels from 0");
        assert_eq!(b.bus_rows(2), BUS_ROWS);
        b.apply(Request::BusRows { alu: AluId { dimension: String::new(), x: 0, y: 0, z: 0 }, side: 2, rows: 500 }).unwrap();
        assert_eq!(b.bus_rows(2), 32, "no more rows than channels");
        // An In side straight onto a bus channel: a bus takes the sides' signals.
        let alu = AluId { dimension: String::new(), x: 0, y: 0, z: 0 };
        assert!(b.apply(Request::Connect { alu, from: Pin::new(Node::In, "N"), to: ch("5") }).is_ok());
    }
}
