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

use yog_api::BlockPos;
use yog_api::Schema;

use crate::cables;
use crate::chip::{ChipMeta, CircuitData};
use crate::content::Tier;
use crate::vm::{PortMode, RedstoneVM};

mod protocol;
mod server;
#[cfg(test)]
mod tests;

pub use protocol::{BoardView, BusView, ChipView, Request, SideView, send};
pub use server::{forget_in, load, nested_wires, restore, save, setup, tick};

/// The six sides, in the order the graph lists them.
pub const SIDES: [&str; 6] = ["N", "S", "E", "W", "U", "D"];
/// Chip slots in an ALU's inventory (a tier uses the first `max_chips`).
pub const CHIP_SLOTS: u8 = 8;

/// Server → client: the board of the ALU being looked at.
pub const VIEW_CHANNEL: &str = "yog-vlsi:board";
/// Client → server: a change to it.
pub const REQUEST_CHANNEL: &str = "yog-vlsi:board_request";

/// How often the chip slots are looked at, in server ticks.
pub(super) const SYNC_EVERY: u64 = 10;
/// How often watchers are sent what changed, in server ticks.
pub(super) const SEND_EVERY: u64 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Schema)]
pub struct AluId {
    pub dimension: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl AluId {
    pub fn pos(&self) -> BlockPos {
        BlockPos {
            x: self.x,
            y: self.y,
            z: self.z,
        }
    }
    pub(super) fn xyz(&self) -> (i32, i32, i32) {
        (self.x, self.y, self.z)
    }
}

/// A node of the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Schema)]
pub enum Node {
    In,
    Out,
    Chip(u8),
    /// The bus on side [`SIDES`]`[i]`.
    Bus(u8),
}

/// A pin: a side of `In`/`Out` (`"N"`, …) or a chip's port label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Schema)]
pub struct Pin {
    pub node: Node,
    pub name: String,
}

impl Pin {
    pub fn new(node: Node, name: impl Into<String>) -> Self {
        Pin {
            node,
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Schema)]
pub struct Wire {
    pub from: Pin,
    pub to: Pin,
}

/// What's saved of a board.
#[derive(Debug, Clone, Schema)]
pub(super) struct Saved {
    tier: Tier,
    wires: Vec<Wire>,
    /// Where the player put each chip slot's node.
    layout: Vec<(u8, (f32, f32))>,
    /// Where the player put the In and Out panels (and the buses).
    #[data(default)]
    panels: Vec<(Node, (f32, f32))>,
    /// How many channel rows each bus node shows, by side.
    #[data(default)]
    bus_rows: Vec<(u8, u16)>,
}

/// Rows a bus node shows until it's pulled open.
pub(super) const BUS_ROWS: u16 = 4;

/// A chip sitting in a slot.
pub(super) struct Installed {
    meta: ChipMeta,
    circuit: CircuitData,
    sim: crate::compile::ChipSim,
}

impl Installed {
    pub(super) fn new(meta: ChipMeta, circuit: CircuitData, vm: RedstoneVM) -> Self {
        let ins = circuit
            .ports
            .iter()
            .filter(|p| p.dir.inputs())
            .map(|p| (p.x, p.y, p.z))
            .collect();
        let outs = circuit
            .ports
            .iter()
            .filter(|p| p.dir.outputs())
            .map(|p| (p.x, p.y, p.z))
            .collect();
        let sim = crate::compile::ChipSim::new(vm, ins, outs, Some(&meta.id));
        Installed { meta, circuit, sim }
    }
}

/// The time all ALUs may take in one server tick: past it their chips go
/// on next tick, slower — the server isn't.
pub(super) const TICK_BUDGET: std::time::Duration = std::time::Duration::from_millis(10);

pub(super) struct Board {
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
    pub(super) fn new(tier: Tier) -> Self {
        Board {
            saved: Saved {
                tier,
                wires: Vec::new(),
                layout: Vec::new(),
                panels: Vec::new(),
                bus_rows: Vec::new(),
            },
            chips: (0..CHIP_SLOTS).map(|_| None).collect(),
            levels: HashMap::new(),
            side_names: HashMap::new(),
            cables: [None; 6],
            cables_known: false,
            bus_driven: HashMap::new(),
        }
    }

    pub(super) fn side_is_output(&self, side: &str) -> bool {
        self.saved
            .wires
            .iter()
            .any(|w| w.to.node == Node::Out && w.to.name == side)
    }

    pub(super) fn active(&self, slot: u8) -> Option<&Installed> {
        if slot as usize >= self.saved.tier.max_chips() {
            return None;
        }
        self.chips.get(slot as usize)?.as_ref()
    }

    /// The mode of a chip pin (`None`: no such pin on an active chip).
    pub(super) fn chip_pin(&self, slot: u8, name: &str) -> Option<PortMode> {
        self.active(slot)?
            .meta
            .ports
            .iter()
            .find(|p| p.label == name)
            .map(|p| p.dir)
    }

    /// How many channels of a bus this ALU's tier sees.
    pub(super) fn bus_channels(&self) -> u16 {
        (self.saved.tier.digital_channels() as u16 * 4).min(cables::BUS_CHANNELS)
    }

    /// The channel (0-based) a pin of bus `side` names, if the bus is there and the tier sees it.
    pub(super) fn bus_channel(&self, side: u8, name: &str) -> Option<u8> {
        let n: u16 = name.parse().ok()?;
        (self.is_bus(side as usize) && (1..=self.bus_channels()).contains(&n))
            .then(|| (n - 1) as u8)
    }

    pub(super) fn bus_rows(&self, side: u8) -> u16 {
        self.saved
            .bus_rows
            .iter()
            .find(|(s, _)| *s == side)
            .map_or(BUS_ROWS, |(_, r)| *r)
            .clamp(1, self.bus_channels())
    }

    pub(super) fn is_source(&self, pin: &Pin) -> bool {
        match pin.node {
            Node::In => SIDES.contains(&pin.name.as_str()) && !self.side_is_output(&pin.name),
            Node::Out => false,
            Node::Chip(slot) => self
                .chip_pin(slot, &pin.name)
                .is_some_and(PortMode::outputs),
            Node::Bus(side) => self.bus_channel(side, &pin.name).is_some(),
        }
    }

    pub(super) fn is_sink(&self, pin: &Pin) -> bool {
        match pin.node {
            Node::In => false,
            Node::Out => SIDES.contains(&pin.name.as_str()),
            Node::Chip(slot) => self.chip_pin(slot, &pin.name).is_some_and(PortMode::inputs),
            Node::Bus(side) => self.bus_channel(side, &pin.name).is_some(),
        }
    }

    pub(super) fn apply(&mut self, request: Request) -> Result<(), String> {
        match request {
            Request::Connect { from, to, .. } => {
                if !self.is_source(&from) || !self.is_sink(&to) {
                    return Err(crate::res::Strings::board_wire_direction());
                }
                // A sink takes one wire; a side that becomes an output stops
                // being an input.
                self.saved.wires.retain(|w| w.to != to);
                if to.node == Node::Out {
                    self.saved
                        .wires
                        .retain(|w| !(w.from.node == Node::In && w.from.name == to.name));
                }
                self.saved.wires.push(Wire { from, to });
            }
            Request::Disconnect { to, .. } => self.saved.wires.retain(|w| w.to != to),
            Request::Clear { .. } => self.saved.wires.clear(),
            Request::Move {
                node: Node::Chip(slot),
                x,
                y,
                ..
            } => {
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
    pub(super) fn sync_chips(&mut self, alu: &AluId) {
        for slot in 0..CHIP_SLOTS {
            let meta =
                yog_api::event::block::inventory_slot(&alu.dimension, alu.pos(), slot as u32)
                    .filter(|s| Tier::from_chip(s.item.as_str()).is_some())
                    .and_then(|s| ChipMeta::from_nbt(&s.snbt));
            let entry = &mut self.chips[slot as usize];
            match meta {
                Some(meta) if entry.as_ref().is_some_and(|c| c.meta.id == meta.id) => {}
                Some(meta) => {
                    let circuit = CircuitData::load_for_chip(&meta.id)
                        .unwrap_or_else(|| CircuitData::empty(meta.tier));
                    // Its ALUs run inside it, their chips' circuits from the store.
                    let vm = circuit.vm_with(&CircuitData::load_for_chip, 1);
                    *entry = Some(Installed::new(meta, circuit, vm));
                }
                None => *entry = None,
            }
        }
    }

    /// Which sides a digital cable reaches now.
    pub(super) fn sync_cables(&mut self, alu: &AluId) {
        // The first look: cables written into the world while the server was
        // down are joined up.
        if !self.cables_known {
            cables::refresh_around(&alu.dimension, alu.pos());
        }
        for (i, side) in SIDES.iter().enumerate() {
            self.cables[i] = cables::cable_at(&alu.dimension, cables::beside(alu.xyz(), side));
        }
        self.cables_known = true;
    }

    /// A digital cable reaches side `i`: it's a bus.
    pub(super) fn is_bus(&self, i: usize) -> bool {
        self.cables[i] == Some(cables::DIGITAL)
    }

    /// What every source drives now: the inputs coming into the ALU's sides,
    /// the chips' output ports and the bus channels shown or wired.
    pub(super) fn sources(&self, alu: &AluId) -> HashMap<Pin, u8> {
        let max = self.saved.tier.max_chips();
        let mut levels = HashMap::new();
        for (i, &side) in SIDES.iter().enumerate() {
            if !self.side_is_output(side) {
                let level = cables::side_input(&alu.dimension, alu.xyz(), side, self.cables[i])
                    .unwrap_or(0);
                levels.insert(Pin::new(Node::In, side), level);
            }
        }
        for (slot, chip) in self.chips.iter().enumerate().take(max) {
            let Some(chip) = chip else { continue };
            for port in chip.circuit.ports.iter().filter(|p| p.dir.outputs()) {
                levels.insert(
                    Pin::new(Node::Chip(slot as u8), &port.label),
                    chip.sim.output(port.x, port.y, port.z),
                );
            }
        }
        for (i, side) in SIDES.iter().enumerate().filter(|(i, _)| self.is_bus(*i)) {
            let shown = (1..=self.bus_rows(i as u8)).map(|n| n.to_string());
            let wired = self
                .saved
                .wires
                .iter()
                .filter(|w| w.from.node == Node::Bus(i as u8))
                .map(|w| w.from.name.clone());
            for name in shown.chain(wired) {
                if let Some(ch) = self.bus_channel(i as u8, &name) {
                    levels.insert(
                        Pin::new(Node::Bus(i as u8), name),
                        cables::bus_level(&alu.dimension, alu.xyz(), side, ch),
                    );
                }
            }
        }
        levels
    }

    /// One server tick: every wire carries its source's level into the
    /// chips, the chips step with it, and what they put out leaves by the
    /// output sides the same tick — a signal crosses an ALU in the chips'
    /// own steps, no tick lost waiting on either side of them.
    pub(super) fn tick(&mut self, alu: &AluId, deadline: std::time::Instant) {
        let max = self.saved.tier.max_chips();
        let incoming = |wires: &[Wire], levels: &HashMap<Pin, u8>, to: &Pin| {
            wires
                .iter()
                .find(|w| &w.to == to)
                .and_then(|w| levels.get(&w.from))
                .copied()
                .unwrap_or(0)
        };

        // In: the sides' inputs and the other chips' outputs, into each chip.
        let before = self.sources(alu);
        let mut inputs = Vec::new();
        for (slot, chip) in self.chips.iter().enumerate().take(max) {
            let Some(chip) = chip else { continue };
            for port in chip.circuit.ports.iter().filter(|p| p.dir.inputs()) {
                inputs.push((
                    slot,
                    port.x,
                    port.y,
                    port.z,
                    incoming(
                        &self.saved.wires,
                        &before,
                        &Pin::new(Node::Chip(slot as u8), &port.label),
                    ),
                ));
            }
        }
        for (slot, x, y, z, level) in inputs {
            if let Some(chip) = &mut self.chips[slot] {
                chip.sim.set_input(x, y, z, level);
            }
        }

        // The chips settle with them: as many steps as they need, up to
        // their tier's — they answer this tick.
        for chip in self.chips.iter_mut().take(max).flatten() {
            chip.sim.new_tick();
            chip.sim
                .settle_until(chip.meta.tier.steps_per_tick(), deadline);
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

    pub(super) fn view(&self, alu: &AluId) -> BoardView {
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
                        pins: c
                            .meta
                            .ports
                            .iter()
                            .map(|p| (p.label.clone(), p.dir))
                            .collect(),
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
                            [&w.from, &w.to]
                                .iter()
                                .any(|p| matches!(p.node, Node::In | Node::Out) && p.name == side)
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
                .map(|i| BusView {
                    index: i,
                    side: SIDES[i as usize].to_string(),
                    channels: self.bus_channels(),
                    rows: self.bus_rows(i),
                })
                .collect(),
            wires: self.saved.wires.clone(),
            layout: self
                .saved
                .layout
                .iter()
                .map(|&(s, p)| (Node::Chip(s), p))
                .chain(self.saved.panels.iter().copied())
                .collect(),
            levels: self.levels.iter().map(|(p, l)| (p.clone(), *l)).collect(),
        }
    }
}
