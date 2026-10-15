//! Rust redstone VM: the simulation behind every installed microchip
//! (DESIGN.md §5). It follows vanilla's rules, so a circuit runs in a chip
//! the way it ran in the editor.
//!
//! A chip's virtual world is a `size`³ grid. Only the cells that hold a
//! block take part in a step: they are kept in [`RedstoneVM::active`] in
//! grid order (y, then z, then x), and grid layers are only allocated once
//! something is placed in them, so an empty Netherite chip costs nothing.
//!
//! ## Power, as vanilla has it
//!
//! - A **component** (lever, torch, repeater, …) gives *weak* power to some
//!   neighbours and *strong* power to some (a torch: weak to every side but
//!   the block it hangs on, strong to the block above it; a repeater: both,
//!   only out of its front).
//! - A **solid block** is *strongly powered* by the strong power going into
//!   it, and *powered* by that or by wire leading into it (wire on top of
//!   it, or pointing at it). Components read *powered* blocks; wire reads
//!   only *strongly powered* ones — which is why wire doesn't run on through
//!   a block.
//! - **Wire** takes the strongest power around it (wire not counting) and
//!   passes it on along the dust, one less per block, also up and down one
//!   block where nothing solid cuts it.
//!
//! `facing` is the block state's own: a repeater or comparator *faces its
//! input* (`repeater[facing=east]` reads from the east, outputs west), an
//! observer faces what it watches and outputs out of its back, a wall torch
//! faces away from its wall.
//!
//! ## A step (one redstone tick)
//!
//! 1. solid blocks take the strong power of the components around them;
//! 2. wire levels settle (strongest first);
//! 3. solid blocks add the power of the wire leading into them;
//! 4. lamps and output ports read their surroundings;
//! 5. components decide their next output from what's around them now —
//!    torches invert, repeaters latch after `delay` steps (stretching short
//!    pulses, held by a locking side), comparators compare or subtract,
//!    observers pulse when what they watch changed. Decided first, applied
//!    after, so the order cells are visited in doesn't matter, and every
//!    component has its one step of delay.
//!
//! ## Moving blocks and items
//!
//! - **Pistons** extend while powered (from any side but their front, or —
//!   quasi-connectivity — through the block above them), pushing up to 12
//!   blocks; slime and honey drag what touches them (but not each other);
//!   redstone parts in the way break; obsidian, containers, extended
//!   pistons and ports don't move. Sticky pistons pull one block back.
//! - **Containers** hold items. Hoppers push one item a time out of their
//!   spout and pull one from the container above, every 4 steps, unless
//!   powered; droppers and dispensers drop one item from a random slot
//!   into the container in front (or out of the world) two steps after
//!   being powered. A comparator reads a container's fill behind it, or
//!   behind the solid block behind it.
//!
//! Self-contained (std + serde only) so `benches/` can build it alone.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

// ── Block types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockType {
    Air,

    /// Any full block: conducts power.
    Solid,
    /// Doesn't conduct (glass, leaves, slabs, …).
    Transparent,

    RedstoneWire,
    /// `attach`: the direction of the block it hangs on (`Down` standing).
    RedstoneTorch { lit: bool, attach: Facing },
    Repeater { delay_ticks: u8, facing: Facing },
    Comparator { mode: ComparatorMode, facing: Facing },
    RedstoneLamp,
    RedstoneBlock,
    /// `attach`: the direction of the block it's mounted on.
    Lever { on: bool, attach: Facing },
    Button { pressed: bool, attach: Facing },
    /// Any pressure plate, at the power it gives.
    PressurePlate { power: u8 },
    Observer { facing: Facing },
    TargetBlock { power: u8 },
    /// Detector rail, trapped chest, …: a constant source.
    Source { power: u8 },

    /// Movable, doesn't conduct, drags what touches it along.
    Slime,
    Honey,
    /// A full block pistons can't move (obsidian, bedrock, …).
    Immovable,
    Piston { facing: Facing, sticky: bool, extended: bool },
    PistonHead { facing: Facing, sticky: bool },
    /// Holds `slots` items (in [`RedstoneVM`]'s inventories); `solid`
    /// conducts power; `partner`: the other half of a double chest.
    Container { slots: u8, solid: bool, partner: Option<Facing> },
    /// `facing`: its spout.
    Hopper { facing: Facing },
    /// A dropper or dispenser; `triggered` while powered.
    Dropper { facing: Facing, triggered: bool },

    /// A VLSI port block.
    Port(PortMode),
    /// An ALU inside the circuit: its chips run inside it ([`NestedAlu`],
    /// set with [`RedstoneVM::set_nested`]); its six faces read and give
    /// power like a component's, a level each.
    Alu,
    /// An analog cable: a run of them carries one level between the ALUs'
    /// faces and the Redstone Ports on it (in `power`), none as redstone.
    Cable,
    /// A Redstone Port: `output`, its cable's level comes out as redstone
    /// (weak, every side); else the redstone it receives goes into it.
    RedstonePort { output: bool },
}

/// What a piston does to a block in its way.
enum Push {
    Move,
    Break,
    Stuck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComparatorMode {
    Compare,
    Subtract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Facing {
    North,
    South,
    East,
    West,
    Up,
    Down,
}

impl Facing {
    pub const ALL: [Facing; 6] = [Facing::North, Facing::South, Facing::East, Facing::West, Facing::Up, Facing::Down];
    pub const HORIZONTAL: [Facing; 4] = [Facing::North, Facing::South, Facing::East, Facing::West];

    /// Its place in [`Facing::ALL`] (an ALU's sides N S E W U D).
    pub fn index(self) -> usize {
        Facing::ALL.iter().position(|&f| f == self).unwrap_or(0)
    }

    pub fn opposite(self) -> Facing {
        match self {
            Facing::North => Facing::South,
            Facing::South => Facing::North,
            Facing::East => Facing::West,
            Facing::West => Facing::East,
            Facing::Up => Facing::Down,
            Facing::Down => Facing::Up,
        }
    }

    /// A quarter turn clockwise seen from above (up and down stay).
    pub fn clockwise(self) -> Facing {
        match self {
            Facing::North => Facing::East,
            Facing::East => Facing::South,
            Facing::South => Facing::West,
            Facing::West => Facing::North,
            other => other,
        }
    }

    /// The two horizontal directions square to this one.
    fn sides(self) -> [Facing; 2] {
        match self {
            Facing::North | Facing::South => [Facing::East, Facing::West],
            _ => [Facing::North, Facing::South],
        }
    }

    /// A block state's `facing` value; anything else is North.
    pub fn from_minecraft(s: &str) -> Facing {
        match s {
            "south" => Facing::South,
            "east" => Facing::East,
            "west" => Facing::West,
            "up" => Facing::Up,
            "down" => Facing::Down,
            _ => Facing::North,
        }
    }
}

/// Direction of a chip I/O port (DESIGN.md §4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PortMode {
    Input,
    Output,
    Bidirectional,
}

// ── Cell ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub block: BlockType,
    /// Wire: its level. Components and ports: what they put out. Lamps: lit
    /// (15) or not.
    pub power: u8,
    /// Solid blocks: strongly powered at this level (what wire reads).
    pub strong: u8,
    /// Solid blocks: powered at this level, wire included (what components
    /// read).
    pub weak: u8,
    /// Repeaters: steps until a scheduled switch (0 = none).
    pub timer: u8,
    /// Value driven into an Input/Bidirectional port from outside the chip.
    pub port_in: u8,
    /// Observers: what they watched looked like last step.
    watched: u32,
    /// Wire: the directions it powers this step (bits of [`Facing::ALL`]).
    points: u8,
}

const AIR: Cell = Cell::new(BlockType::Air);

impl Cell {
    pub const fn new(block: BlockType) -> Self {
        let power = match block {
            BlockType::RedstoneTorch { lit: true, .. }
            | BlockType::Lever { on: true, .. }
            | BlockType::Button { pressed: true, .. }
            | BlockType::RedstoneBlock => 15,
            BlockType::PressurePlate { power } | BlockType::TargetBlock { power } | BlockType::Source { power } => power,
            _ => 0,
        };
        Cell { block, power, strong: 0, weak: 0, timer: 0, port_in: 0, watched: 0, points: 0 }
    }

    fn is_solid(&self) -> bool {
        matches!(
            self.block,
            BlockType::Solid
                | BlockType::RedstoneLamp
                | BlockType::Immovable
                | BlockType::Container { solid: true, .. }
                | BlockType::Dropper { .. }
        )
    }

    fn is_air(&self) -> bool {
        self.block == BlockType::Air
    }

    fn holds_items(&self) -> bool {
        matches!(self.block, BlockType::Container { .. } | BlockType::Hopper { .. } | BlockType::Dropper { .. })
    }

    fn push(&self) -> Push {
        match self.block {
            BlockType::Immovable
            | BlockType::PistonHead { .. }
            | BlockType::Piston { extended: true, .. }
            | BlockType::Container { .. }
            | BlockType::Hopper { .. }
            | BlockType::Dropper { .. }
            | BlockType::Port(_)
            | BlockType::Alu => Push::Stuck,
            BlockType::RedstoneWire
            | BlockType::RedstoneTorch { .. }
            | BlockType::Repeater { .. }
            | BlockType::Comparator { .. }
            | BlockType::Lever { .. }
            | BlockType::Button { .. }
            | BlockType::PressurePlate { .. } => Push::Break,
            _ => Push::Move,
        }
    }

    fn is_sticky(&self) -> bool {
        matches!(self.block, BlockType::Slime | BlockType::Honey)
    }

    fn is_wire(&self) -> bool {
        self.block == BlockType::RedstoneWire
    }

    fn is_output_port(&self) -> bool {
        matches!(self.block, BlockType::Port(PortMode::Output | PortMode::Bidirectional))
    }

    /// Wire next to it turns toward it (`toward`: from the wire to it).
    fn wire_connects(&self, toward: Facing) -> bool {
        match self.block {
            BlockType::Repeater { facing, .. } => facing == toward || facing == toward.opposite(),
            BlockType::Observer { facing } => facing == toward,
            BlockType::RedstoneTorch { .. }
            | BlockType::Comparator { .. }
            | BlockType::RedstoneBlock
            | BlockType::Lever { .. }
            | BlockType::Button { .. }
            | BlockType::PressurePlate { .. }
            | BlockType::TargetBlock { .. }
            | BlockType::Source { .. }
            | BlockType::Port(_)
            | BlockType::RedstonePort { .. }
            | BlockType::Alu => true,
            _ => false,
        }
    }

    /// The (weak, strong) power this component gives the neighbour in
    /// direction `to`. Not for wire or solid blocks.
    fn emits(&self, to: Facing) -> (u8, u8) {
        let p = self.power;
        match self.block {
            BlockType::RedstoneBlock
            | BlockType::TargetBlock { .. }
            | BlockType::Source { .. }
            | BlockType::RedstonePort { output: true } => (p, 0),
            BlockType::Lever { attach, .. } | BlockType::Button { attach, .. } => (p, if to == attach { p } else { 0 }),
            BlockType::PressurePlate { .. } => (p, if to == Facing::Down { p } else { 0 }),
            BlockType::RedstoneTorch { attach, .. } => {
                (if to == attach { 0 } else { p }, if to == Facing::Up { p } else { 0 })
            }
            BlockType::Repeater { facing, .. } | BlockType::Comparator { facing, .. } | BlockType::Observer { facing } => {
                if to == facing.opposite() { (p, p) } else { (0, 0) }
            }
            BlockType::Port(PortMode::Input | PortMode::Bidirectional) => (self.port_in, 0),
            // A nested ALU's faces: a level each, packed in `watched`.
            BlockType::Alu => {
                let level = ((self.watched >> (4 * to.index())) & 0xF) as u8;
                (level, level)
            }
            _ => (0, 0),
        }
    }

    /// What an observer sees of this cell.
    fn looks(&self) -> u32 {
        let mut h = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(&self.block, &mut h);
        let shown = match self.block {
            BlockType::Solid | BlockType::Transparent | BlockType::Air | BlockType::Immovable => 0,
            _ => self.power as u64,
        };
        (std::hash::Hasher::finish(&h) as u32) ^ (shown as u32) << 24
    }
}

// ── VM ────────────────────────────────────────────────────────────────────────

pub struct RedstoneVM {
    /// Edge length of the (cubic) world, a power of two.
    pub size: u32,
    /// `log2(size)`: cell indices are `x | z << shift | y << 2 * shift`.
    shift: u32,
    /// One `size`×`size` layer per y, allocated on first placement.
    layers: Vec<Option<Box<[Cell]>>>,
    /// Indices of every non-air cell, ascending (= y, z, x order).
    active: Vec<u32>,
    pub tick: u64,
    /// The items of every container, by cell: (item, count) per slot.
    inventories: HashMap<u32, Vec<(u16, u8)>>,
    /// The nested ALUs' insides, by cell.
    nested: HashMap<u32, NestedAlu>,
    /// Item ids, indexed by the numbers in `inventories`.
    items: Vec<String>,
    rng: u64,
    /// Settled, and nothing from outside changed since: another step would
    /// change nothing either, so [`settle`](Self::settle) skips it.
    quiet: bool,
}

/// One slot's item: (index into `items`, count); count 0 = empty.
type Slot = (u16, u8);

/// How many of an item stack in one slot.
fn max_stack(item: &str) -> u8 {
    let path = item.strip_prefix("minecraft:").unwrap_or(item);
    const SIXTEEN: [&str; 7] = ["ender_pearl", "snowball", "egg", "_sign", "_banner", "honey_bottle", "armor_stand"];
    const ONE: [&str; 16] = [
        "sword", "pickaxe", "_axe", "shovel", "_hoe", "helmet", "chestplate", "leggings", "boots", "potion",
        "shulker_box", "minecart", "boat", "_bed", "saddle", "totem_of_undying",
    ];
    if path == "bucket" || SIXTEEN.iter().any(|p| path.ends_with(p)) {
        16
    } else if path.ends_with("_bucket") || ONE.iter().any(|p| path.contains(p)) {
        1
    } else {
        64
    }
}

impl RedstoneVM {
    /// A world `size` cells on each side (rounded up to a power of two;
    /// every tier's is one).
    pub fn new(size: u32) -> Self {
        let size = size.max(1).next_power_of_two();
        RedstoneVM {
            size,
            shift: size.trailing_zeros(),
            layers: vec![None; size as usize],
            active: Vec::new(),
            tick: 0,
            inventories: HashMap::new(),
            nested: HashMap::new(),
            items: Vec::new(),
            rng: 0x9E37_79B9_7F4A_7C15,
            quiet: false,
        }
    }

    #[inline]
    fn idx(&self, x: u32, y: u32, z: u32) -> u32 {
        x | z << self.shift | y << (2 * self.shift)
    }

    #[inline]
    fn pos(&self, i: u32) -> (u32, u32, u32) {
        let mask = self.size - 1;
        (i & mask, i >> (2 * self.shift), (i >> self.shift) & mask)
    }

    #[inline]
    fn layer_len(&self) -> u32 {
        1 << (2 * self.shift)
    }

    #[inline]
    fn in_bounds(&self, x: i32, y: i32, z: i32) -> bool {
        let s = self.size as i32;
        (0..s).contains(&x) && (0..s).contains(&y) && (0..s).contains(&z)
    }

    /// The neighbour of cell `i` in direction `d`, if inside the world.
    #[inline]
    fn at(&self, i: u32, d: Facing) -> Option<u32> {
        let (x, y, z) = self.pos(i);
        let (x, y, z) = (x as i32, y as i32, z as i32);
        let (x, y, z) = match d {
            Facing::North => (x, y, z - 1),
            Facing::South => (x, y, z + 1),
            Facing::East => (x + 1, y, z),
            Facing::West => (x - 1, y, z),
            Facing::Up => (x, y + 1, z),
            Facing::Down => (x, y - 1, z),
        };
        self.in_bounds(x, y, z).then(|| self.idx(x as u32, y as u32, z as u32))
    }

    #[inline]
    fn get(&self, i: u32) -> &Cell {
        match &self.layers[(i >> (2 * self.shift)) as usize] {
            Some(cells) => &cells[(i & (self.layer_len() - 1)) as usize],
            None => &AIR,
        }
    }

    /// The cell next to `i` in direction `d` (air outside the world).
    #[inline]
    fn next(&self, i: u32, d: Facing) -> &Cell {
        self.at(i, d).map_or(&AIR, |n| self.get(n))
    }

    /// Mutable access to a cell that holds a block (its layer exists).
    #[inline]
    fn get_mut(&mut self, i: u32) -> &mut Cell {
        let offset = (i & (self.layer_len() - 1)) as usize;
        let cells = self.layers[(i >> (2 * self.shift)) as usize].as_mut().expect("only non-air cells are written");
        &mut cells[offset]
    }

    pub fn cell(&self, x: u32, y: u32, z: u32) -> &Cell {
        self.get(self.idx(x, y, z))
    }

    /// Number of non-air cells.
    #[cfg(test)]
    pub fn block_count(&self) -> usize {
        self.active.len()
    }

    /// Bytes held by the grid (allocated layers + the active index).
    #[cfg(test)]
    pub fn memory_bytes(&self) -> usize {
        let layer = self.layer_len() as usize * std::mem::size_of::<Cell>();
        self.layers.iter().flatten().count() * layer + self.active.capacity() * 4
    }

    pub fn set_block(&mut self, x: u32, y: u32, z: u32, block: BlockType) {
        self.quiet = false;
        if !self.in_bounds(x as i32, y as i32, z as i32) {
            return;
        }
        let i = self.idx(x, y, z);
        self.put(i, Cell::new(block));
        let slots = match block {
            BlockType::Container { slots, .. } => slots,
            BlockType::Hopper { .. } => 5,
            BlockType::Dropper { .. } => 9,
            _ => 0,
        };
        if slots > 0 {
            self.inventories.insert(i, vec![(0, 0); slots as usize]);
        } else {
            self.inventories.remove(&i);
        }
    }

    /// Writes `cell` at `i` (air removes it).
    fn put(&mut self, i: u32, cell: Cell) {
        let found = self.active.binary_search(&i);
        if cell.is_air() {
            if let Ok(at) = found {
                self.active.remove(at);
                *self.get_mut(i) = AIR;
            }
            return;
        }
        if let Err(at) = found {
            self.active.insert(at, i);
        }
        let w = self.layer_len() as usize;
        let y = (i >> (2 * self.shift)) as usize;
        let cells = self.layers[y].get_or_insert_with(|| vec![AIR; w].into_boxed_slice());
        cells[i as usize % w] = cell;
    }

    /// Fills the container at `(x, y, z)`: (slot, item id, count).
    pub fn set_items(&mut self, x: u32, y: u32, z: u32, items: &[(u8, String, u8)]) {
        self.quiet = false;
        if !self.in_bounds(x as i32, y as i32, z as i32) {
            return;
        }
        let i = self.idx(x, y, z);
        let ids: Vec<u16> = items.iter().map(|(_, item, _)| self.item_index(item)).collect();
        let Some(inv) = self.inventories.get_mut(&i) else { return };
        for ((slot, _, count), id) in items.iter().zip(ids) {
            if let Some(s) = inv.get_mut(*slot as usize) {
                *s = (id, *count);
            }
        }
    }

    /// The items in the container at `(x, y, z)`: (slot, item id, count).
    #[cfg(test)]
    pub fn items_at(&self, x: u32, y: u32, z: u32) -> Vec<(u8, String, u8)> {
        let i = self.idx(x, y, z);
        self.inventories.get(&i).into_iter().flatten().enumerate()
            .filter(|(_, (_, n))| *n > 0)
            .map(|(slot, (id, n))| (slot as u8, self.items[*id as usize].clone(), *n))
            .collect()
    }

    fn item_index(&mut self, item: &str) -> u16 {
        match self.items.iter().position(|i| i == item) {
            Some(p) => p as u16,
            None => {
                self.items.push(item.to_string());
                (self.items.len() - 1) as u16
            }
        }
    }

    /// Drives `power` into the Input/Bidirectional port at `(x, y, z)`.
    /// Puts `alu`'s insides into the ALU block at `(x, y, z)`.
    pub fn set_nested(&mut self, x: u32, y: u32, z: u32, alu: NestedAlu) {
        self.quiet = false;
        if !self.in_bounds(x as i32, y as i32, z as i32) {
            return;
        }
        let i = self.idx(x, y, z);
        if self.get(i).block == BlockType::Alu {
            self.nested.insert(i, alu);
        }
    }

    pub fn set_port_input(&mut self, x: u32, y: u32, z: u32, power: u8) {
        if !self.in_bounds(x as i32, y as i32, z as i32) {
            return;
        }
        let i = self.idx(x, y, z);
        if matches!(self.get(i).block, BlockType::Port(PortMode::Input | PortMode::Bidirectional)) && self.get(i).port_in != power.min(15) {
            self.get_mut(i).port_in = power.min(15);
            self.quiet = false;
        }
    }

    /// What the Output/Bidirectional port at `(x, y, z)` reads from the
    /// circuit around it (0 for anything else).
    pub fn get_port_output(&self, x: u32, y: u32, z: u32) -> u8 {
        if !self.in_bounds(x as i32, y as i32, z as i32) {
            return 0;
        }
        let cell = self.cell(x, y, z);
        if cell.is_output_port() { cell.power } else { 0 }
    }

    /// `(x, z, power)` of every output/bidirectional port on layer `y`.
    #[cfg(test)]
    pub fn read_outputs(&self, y: u32) -> Vec<(u32, u32, u8)> {
        self.active
            .iter()
            .map(|&i| (i, self.pos(i)))
            .filter(|&(i, (_, cy, _))| cy == y && self.get(i).is_output_port())
            .map(|(i, (x, _, z))| (x, z, self.get(i).power))
            .collect()
    }

    /// A new server tick: every nested ALU's chips get their tier's steps
    /// again (all the way down).
    pub fn new_tick(&mut self) {
        for alu in self.nested.values_mut() {
            for c in &mut alu.chips {
                c.left = c.budget;
                c.vm.new_tick();
            }
        }
    }

    /// Steps until the circuit settles — a step changes nothing — or for
    /// `budget` steps; how many it took. A circuit answers its inputs in
    /// the steps it needs, not one a server tick.
    pub fn settle(&mut self, budget: u32) -> u32 {
        // Settled and its inputs as they were: the step would change nothing.
        if self.quiet {
            return 0;
        }
        let mut before = self.state();
        for n in 1..=budget {
            self.step();
            let now = self.state();
            if now == before {
                // Not while a nested ALU's chip waits for its next tick's steps.
                self.quiet = self.nested.values().all(|a| a.chips.iter().all(|c| c.settled_on.is_some()));
                return n;
            }
            before = now;
        }
        budget
    }

    /// A fingerprint of every cell's state (block, levels, timers).
    fn state(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        for &i in &self.active {
            let c = self.get(i);
            let mut bh = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&c.block, &mut bh);
            mix(std::hash::Hasher::finish(&bh));
            mix(u64::from(c.power) | u64::from(c.strong) << 8 | u64::from(c.weak) << 16 | u64::from(c.timer) << 24 | u64::from(i) << 32);
            mix(u64::from(c.watched));
        }
        // A nested ALU's chips settle inside a step; still running (a clock), the circuit isn't settled.
        for alu in self.nested.values() {
            for chip in &alu.chips {
                mix(chip.vm.state());
            }
        }
        h
    }

    pub fn step(&mut self) {
        self.tick += 1;
        self.shape_wire();
        self.strong_power();
        self.settle_wire();
        self.wire_power();
        self.read_power();
        self.settle_components();
        self.move_items();
    }

    // ── Wire ──────────────────────────────────────────────────────────────

    /// The wire cells wire `i` passes power to and from: next to it, one up
    /// (nothing solid above `i`) or one down (nothing solid beside it).
    fn wire_links(&self, i: u32) -> impl Iterator<Item = u32> + '_ {
        let open_above = !self.next(i, Facing::Up).is_solid();
        Facing::HORIZONTAL.into_iter().filter_map(move |d| {
            let n = self.at(i, d)?;
            let side = self.get(n);
            if side.is_wire() {
                return Some(n);
            }
            if side.is_solid() {
                let up = self.at(n, Facing::Up)?;
                return (open_above && self.get(up).is_wire()).then_some(up);
            }
            let down = self.at(n, Facing::Down)?;
            self.get(down).is_wire().then_some(down)
        })
    }

    /// Wire `i` turns toward direction `d` (to wire, up or down a step, or
    /// a component).
    fn wire_turns(&self, i: u32, d: Facing) -> bool {
        let Some(n) = self.at(i, d) else { return false };
        let side = self.get(n);
        if side.is_wire() || side.wire_connects(d) {
            return true;
        }
        if side.is_solid() {
            return !self.next(i, Facing::Up).is_solid() && self.next(n, Facing::Up).is_wire();
        }
        self.next(n, Facing::Down).is_wire()
    }

    /// The directions wire `i` powers (bits of [`Facing::ALL`]): always
    /// the block below; sideways where it points (a lone dot points
    /// everywhere, a dot turned one way runs straight through).
    fn wire_shape(&self, i: u32) -> u8 {
        let bit = |d: Facing| 1u8 << Facing::ALL.iter().position(|&f| f == d).unwrap_or(0);
        let mut turns = 0u8;
        let mut count = 0;
        for h in Facing::HORIZONTAL {
            if self.wire_turns(i, h) {
                turns |= bit(h);
                count += 1;
            }
        }
        let sides = match count {
            0 => Facing::HORIZONTAL.iter().fold(0, |m, &h| m | bit(h)),
            1 => Facing::HORIZONTAL
                .iter()
                .filter(|&&h| turns & (bit(h) | bit(h.opposite())) != 0)
                .fold(0, |m, &h| m | bit(h)),
            _ => turns,
        };
        sides | bit(Facing::Down)
    }

    /// 0. Every wire's shape, for this step.
    fn shape_wire(&mut self) {
        for k in 0..self.active.len() {
            let i = self.active[k];
            if self.get(i).is_wire() {
                let shape = self.wire_shape(i);
                self.get_mut(i).points = shape;
            }
        }
    }

    /// Wire `i` powers the block in direction `d` (this step's shape).
    fn wire_points(&self, i: u32, d: Facing) -> bool {
        let bit = Facing::ALL.iter().position(|&f| f == d).unwrap_or(0);
        self.get(i).points & (1 << bit) != 0
    }

    /// The power cell `i`'s neighbour in direction `d` gives it: a
    /// component's weak power, a solid block's strong one (with
    /// `wire`: its whole power), wire's level where it points at `i`.
    fn input_from(&self, i: u32, d: Facing, wire: bool) -> u8 {
        let Some(n) = self.at(i, d) else { return 0 };
        let c = self.get(n);
        if c.is_wire() {
            return if wire && self.wire_points(n, d.opposite()) { c.power } else { 0 };
        }
        if c.is_solid() {
            return if wire { c.weak } else { c.strong };
        }
        c.emits(d.opposite()).0
    }

    /// The strongest power around cell `i`.
    fn received(&self, i: u32, wire: bool) -> u8 {
        Facing::ALL.into_iter().map(|d| self.input_from(i, d, wire)).max().unwrap_or(0)
    }

    /// 1. Solid blocks take the strong power going into them.
    fn strong_power(&mut self) {
        for k in 0..self.active.len() {
            let i = self.active[k];
            if !self.get(i).is_solid() {
                continue;
            }
            let strong = Facing::ALL
                .into_iter()
                .filter_map(|d| {
                    let c = self.next(i, d);
                    (!c.is_wire() && !c.is_solid()).then(|| c.emits(d.opposite()).1)
                })
                .max()
                .unwrap_or(0);
            let cell = self.get_mut(i);
            cell.strong = strong;
            cell.weak = strong;
        }
    }

    /// 2. Wire levels: what each wire gets from outside the dust, spread
    ///    along it strongest first, one less per block.
    fn settle_wire(&mut self) {
        let mut buckets: [Vec<u32>; 16] = Default::default();
        for k in 0..self.active.len() {
            let i = self.active[k];
            if !self.get(i).is_wire() {
                continue;
            }
            let level = self.received(i, false);
            self.get_mut(i).power = level;
            buckets[level as usize].push(i);
        }
        for level in (2..=15u8).rev() {
            let wires = std::mem::take(&mut buckets[level as usize]);
            for i in wires {
                if self.get(i).power != level {
                    continue;
                }
                let links: Vec<u32> = self.wire_links(i).collect();
                for n in links {
                    if self.get(n).power < level - 1 {
                        self.get_mut(n).power = level - 1;
                        buckets[level as usize - 1].push(n);
                    }
                }
            }
        }
    }

    /// 3. Solid blocks add the wire leading into them.
    fn wire_power(&mut self) {
        for k in 0..self.active.len() {
            let i = self.active[k];
            if !self.get(i).is_solid() {
                continue;
            }
            let from_wire = Facing::ALL
                .into_iter()
                .filter_map(|d| {
                    let n = self.at(i, d)?;
                    (self.get(n).is_wire() && self.wire_points(n, d.opposite())).then(|| self.get(n).power)
                })
                .max()
                .unwrap_or(0);
            let cell = self.get_mut(i);
            cell.weak = cell.weak.max(from_wire);
        }
    }

    /// 4. Lamps and output ports read what's around them.
    fn read_power(&mut self) {
        for k in 0..self.active.len() {
            let i = self.active[k];
            let cell = *self.get(i);
            let level = match cell.block {
                BlockType::RedstoneLamp => {
                    if self.received(i, true) > 0 { 15 } else { 0 }
                }
                BlockType::Port(PortMode::Output | PortMode::Bidirectional) => self.received(i, true),
                _ => continue,
            };
            self.get_mut(i).power = level;
        }
    }

    // ── Components ────────────────────────────────────────────────────────

    /// The power a comparator's side reads: only wire, redstone blocks and
    /// repeaters/comparators pointing at it count.
    fn side_input(&self, i: u32, d: Facing) -> u8 {
        let Some(n) = self.at(i, d) else { return 0 };
        let c = self.get(n);
        match c.block {
            BlockType::RedstoneWire => c.power,
            BlockType::RedstoneBlock => 15,
            BlockType::Repeater { .. } | BlockType::Comparator { .. } => c.emits(d.opposite()).0,
            _ => 0,
        }
    }

    /// 5a. Analog cables: each run carries the strongest level put into it
    /// — by an input Redstone Port (the redstone around it) or an ALU's face
    /// — to the ALUs' faces on it and out of its output ports. Worked out
    /// from its sources each step: a source gone, the level goes.
    fn cable_networks(&mut self) {
        let mut seen: HashSet<u32> = HashSet::new();
        let mut changes: Vec<(u32, u8)> = Vec::new();
        for k in 0..self.active.len() {
            let start = self.active[k];
            if self.get(start).block != BlockType::Cable || !seen.insert(start) {
                continue;
            }
            let (mut run, mut outs, mut level) = (vec![start], Vec::new(), 0u8);
            let mut queue = VecDeque::from([start]);
            while let Some(i) = queue.pop_front() {
                for d in Facing::ALL {
                    let Some(n) = self.at(i, d) else { continue };
                    match self.get(n).block {
                        BlockType::Cable => {
                            if seen.insert(n) {
                                run.push(n);
                                queue.push_back(n);
                            }
                        }
                        BlockType::Alu => level = level.max(self.get(n).emits(d.opposite()).0),
                        BlockType::RedstonePort { output: false } => level = level.max(self.received(n, true)),
                        BlockType::RedstonePort { output: true } => outs.push(n),
                        _ => {}
                    }
                }
            }
            changes.extend(run.into_iter().chain(outs).map(|i| (i, level)));
        }
        for (i, level) in changes {
            self.get_mut(i).power = level;
        }
    }

    /// 5. Every component decides its next output.
    fn settle_components(&mut self) {
        self.cable_networks();
        let mut pistons = Vec::new();
        let changes: Vec<(u32, Cell)> = self
            .active
            .iter()
            .filter_map(|&i| {
                let cell = *self.get(i);
                let next = match cell.block {
                    BlockType::RedstoneTorch { attach, .. } => {
                        let lit = self.input_from(i, attach, true) == 0;
                        Cell { block: BlockType::RedstoneTorch { lit, attach }, power: if lit { 15 } else { 0 }, ..cell }
                    }
                    BlockType::Repeater { delay_ticks, facing } => self.next_repeater(i, cell, delay_ticks.max(1), facing),
                    BlockType::Comparator { mode, facing } => {
                        let back = self.comparator_back(i, facing);
                        let side = facing.sides().into_iter().map(|d| self.side_input(i, d)).max().unwrap_or(0);
                        let power = match mode {
                            ComparatorMode::Compare if back >= side => back,
                            ComparatorMode::Compare => 0,
                            ComparatorMode::Subtract => back.saturating_sub(side),
                        };
                        Cell { power, ..cell }
                    }
                    BlockType::Observer { facing } => {
                        let looks = self.next(i, facing).looks();
                        let changed = looks != cell.watched && self.tick > 1;
                        Cell { power: if changed && cell.power == 0 { 15 } else { 0 }, watched: looks, ..cell }
                    }
                    BlockType::Piston { facing, extended, .. } => {
                        if self.piston_powered(i, facing) != extended {
                            pistons.push(i);
                        }
                        return None;
                    }
                    BlockType::Dropper { facing, triggered } => {
                        let powered = self.piston_powered(i, facing) || self.received(i, true) > 0;
                        let mut next = Cell { block: BlockType::Dropper { facing, triggered: powered }, ..cell };
                        if powered && !triggered {
                            next.timer = 2;
                        }
                        next
                    }
                    _ => return None,
                };
                (next.block != cell.block || next.power != cell.power || next.timer != cell.timer || next.watched != cell.watched)
                    .then_some((i, next))
            })
            .collect();
        // Nested ALUs: their faces' power in, their chips run, their faces' levels out.
        let mut changes = changes;
        let alus: Vec<(u32, [u8; 6])> = self
            .nested
            .keys()
            .filter(|&&i| self.get(i).block == BlockType::Alu)
            .map(|&i| {
                // From a cable, its level; else the redstone into that face.
                let face = |f| match self.next(i, f).block {
                    BlockType::Cable => self.next(i, f).power,
                    _ => self.input_from(i, f, true),
                };
                (i, Facing::ALL.map(face))
            })
            .collect();
        for (i, inputs) in alus {
            let Some(alu) = self.nested.get_mut(&i) else { continue };
            let outputs = alu.run(inputs);
            let packed = outputs.iter().enumerate().fold(0u32, |p, (k, &l)| p | u32::from(l.min(15)) << (4 * k));
            let cell = *self.get(i);
            if cell.watched != packed {
                let power = outputs.iter().copied().max().unwrap_or(0);
                changes.push((i, Cell { watched: packed, power, ..cell }));
            }
        }
        for (i, next) in changes {
            *self.get_mut(i) = next;
        }
        for i in pistons {
            if let BlockType::Piston { facing, sticky, extended } = self.get(i).block {
                if extended {
                    self.retract(i, facing, sticky);
                } else {
                    self.extend(i, facing, sticky);
                }
            }
        }
    }

    // ── Pistons ───────────────────────────────────────────────────────────

    /// Powered from any side but its front, or through the block above it.
    fn piston_powered(&self, i: u32, facing: Facing) -> bool {
        let direct = Facing::ALL.into_iter().filter(|&d| d != facing).any(|d| self.input_from(i, d, true) > 0);
        direct
            || self.at(i, Facing::Up).is_some_and(|up| {
                Facing::ALL.into_iter().filter(|&d| d != Facing::Down).any(|d| self.input_from(up, d, true) > 0)
            })
    }

    /// What moving the block at `start` in direction `dir` takes along
    /// (`None`: it can't move). `piston`: the piston doing it, which never
    /// moves itself.
    fn plan_move(&self, start: u32, dir: Facing, piston: u32) -> Option<(Vec<u32>, Vec<u32>)> {
        let mut moved = Vec::new();
        let mut seen = HashSet::new();
        let mut broken = Vec::new();
        // (cell, forced): forced cells are in the way and must go; the
        // others are only dragged by slime or honey.
        let mut queue = VecDeque::from([(start, true)]);
        while let Some((p, forced)) = queue.pop_front() {
            let c = *self.get(p);
            if c.is_air() || seen.contains(&p) {
                continue;
            }
            if p == piston {
                if forced {
                    return None;
                }
                continue;
            }
            match c.push() {
                Push::Stuck if forced => return None,
                Push::Stuck => continue,
                Push::Break => {
                    if forced && !broken.contains(&p) {
                        broken.push(p);
                    }
                    continue;
                }
                Push::Move => {}
            }
            seen.insert(p);
            moved.push(p);
            if moved.len() > 12 {
                return None;
            }
            queue.push_back((self.at(p, dir)?, true));
            if c.is_sticky() {
                for d in Facing::ALL {
                    let Some(n) = self.at(p, d) else { continue };
                    let nc = self.get(n);
                    let other_glue = nc.is_sticky() && nc.block != c.block;
                    if !nc.is_air() && !other_glue {
                        queue.push_back((n, false));
                    }
                }
            }
        }
        Some((moved, broken))
    }

    /// Moves what `plan_move` found one block in `dir`.
    fn do_move(&mut self, moved: &[u32], broken: &[u32], dir: Facing) {
        for &p in broken {
            self.put(p, AIR);
        }
        let cells: Vec<(u32, Cell)> = moved.iter().map(|&p| (p, *self.get(p))).collect();
        for &(p, _) in &cells {
            self.put(p, AIR);
        }
        for (p, cell) in cells {
            if let Some(to) = self.at(p, dir) {
                self.put(to, cell);
            }
        }
    }

    fn extend(&mut self, i: u32, facing: Facing, sticky: bool) {
        let Some(front) = self.at(i, facing) else { return };
        let Some((moved, broken)) = self.plan_move(front, facing, i) else { return };
        self.do_move(&moved, &broken, facing);
        self.put(front, Cell::new(BlockType::PistonHead { facing, sticky }));
        self.get_mut(i).block = BlockType::Piston { facing, sticky, extended: true };
    }

    fn retract(&mut self, i: u32, facing: Facing, sticky: bool) {
        self.get_mut(i).block = BlockType::Piston { facing, sticky, extended: false };
        let Some(head) = self.at(i, facing) else { return };
        if matches!(self.get(head).block, BlockType::PistonHead { .. }) {
            self.put(head, AIR);
        }
        if !sticky {
            return;
        }
        let Some(pulled) = self.at(head, facing) else { return };
        if matches!(self.get(pulled).push(), Push::Move)
            && let Some((moved, broken)) = self.plan_move(pulled, facing.opposite(), i)
        {
            self.do_move(&moved, &broken, facing.opposite());
        }
    }

    // ── Items ─────────────────────────────────────────────────────────────

    /// The cells whose slots make up the container at `i` (a double chest
    /// is two).
    fn container(&self, i: u32) -> Vec<u32> {
        let mut cells = vec![i];
        if let BlockType::Container { partner: Some(d), .. } = self.get(i).block
            && let Some(p) = self.at(i, d)
            && matches!(self.get(p).block, BlockType::Container { partner: Some(back), .. } if back == d.opposite())
        {
            cells.push(p);
        }
        cells
    }

    /// How full the container at `i` is, as a comparator reads it.
    fn fill(&self, i: u32) -> u8 {
        let (mut sum, mut slots, mut any) = (0.0f32, 0usize, false);
        for c in self.container(i) {
            for &(id, n) in self.inventories.get(&c).into_iter().flatten() {
                slots += 1;
                if n > 0 {
                    any = true;
                    sum += n as f32 / max_stack(&self.items[id as usize]) as f32;
                }
            }
        }
        if !any || slots == 0 { 0 } else { (1.0 + sum / slots as f32 * 14.0).floor() as u8 }
    }

    /// A comparator's back: a container's fill, through one solid block
    /// too; else the power coming in.
    fn comparator_back(&self, i: u32, facing: Facing) -> u8 {
        let power = self.input_from(i, facing, true);
        let Some(b) = self.at(i, facing) else { return power };
        let back = self.get(b);
        if back.holds_items() {
            return self.fill(b);
        }
        if back.is_solid()
            && power < 15
            && let Some(b2) = self.at(b, facing)
            && self.get(b2).holds_items()
        {
            return power.max(self.fill(b2));
        }
        power
    }

    /// Takes one item out of the first non-empty slot of `from` (or a
    /// random one) that `to` takes; `to = None` throws it away.
    fn transfer(&mut self, from: u32, to: Option<u32>, random: bool) -> bool {
        let Some(inv) = self.inventories.get(&from) else { return false };
        let mut full: Vec<usize> = inv.iter().enumerate().filter(|(_, s)| s.1 > 0).map(|(k, _)| k).collect();
        if random && !full.is_empty() {
            self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let pick = (self.rng >> 33) as usize % full.len();
            full = vec![full[pick]];
        }
        for slot in full {
            let item = self.inventories[&from][slot].0;
            let ok = match to {
                Some(to) => self.insert(to, item),
                None => true,
            };
            if ok {
                let s = &mut self.inventories.get_mut(&from).unwrap()[slot];
                s.1 -= 1;
                return true;
            }
        }
        false
    }

    /// Puts one `item` into the container at `to`.
    fn insert(&mut self, to: u32, item: u16) -> bool {
        let max = max_stack(&self.items[item as usize]);
        let cells = self.container(to);
        for pass in 0..2 {
            for &c in &cells {
                let Some(inv) = self.inventories.get_mut(&c) else { continue };
                let found = inv.iter_mut().find(|s: &&mut Slot| {
                    if pass == 0 { s.1 > 0 && s.0 == item && s.1 < max } else { s.1 == 0 }
                });
                if let Some(s) = found {
                    *s = (item, s.1 + 1);
                    return true;
                }
            }
        }
        false
    }

    /// Hoppers move an item every 4 steps unless powered; droppers drop
    /// theirs.
    fn move_items(&mut self) {
        for k in 0..self.active.len() {
            let Some(&i) = self.active.get(k) else { break };
            let cell = *self.get(i);
            match cell.block {
                BlockType::Hopper { facing } => {
                    if cell.timer > 0 {
                        self.get_mut(i).timer -= 1;
                        if cell.timer > 1 {
                            continue;
                        }
                    }
                    if self.received(i, true) > 0 {
                        continue;
                    }
                    let out = self.at(i, facing).filter(|&t| self.get(t).holds_items());
                    let mut moved = out.is_some_and(|t| self.transfer(i, Some(t), false));
                    if let Some(up) = self.at(i, Facing::Up).filter(|&u| self.get(u).holds_items()) {
                        moved |= self.transfer(up, Some(i), false);
                    }
                    if moved {
                        self.get_mut(i).timer = 4;
                    }
                }
                BlockType::Dropper { facing, .. } if cell.timer > 0 => {
                    self.get_mut(i).timer -= 1;
                    if cell.timer == 1 {
                        let to = self.at(i, facing).filter(|&t| self.get(t).holds_items());
                        self.transfer(i, to, true);
                    }
                }
                _ => {}
            }
        }
    }

    /// A repeater takes its input `delay` steps later; once on, it stays on
    /// at least `delay` steps. A powered repeater or comparator pointing
    /// into its side locks it as it is.
    fn next_repeater(&self, i: u32, mut cell: Cell, delay: u8, facing: Facing) -> Cell {
        let locked = facing.sides().into_iter().any(|d| {
            let c = self.next(i, d);
            matches!(c.block, BlockType::Repeater { .. } | BlockType::Comparator { .. }) && c.emits(d.opposite()).0 > 0
        });
        if locked {
            cell.timer = 0;
            return cell;
        }
        let input = self.input_from(i, facing, true) > 0;
        let on = cell.power > 0;
        if cell.timer == 0 {
            if input == on {
                return cell;
            }
            cell.timer = delay;
        }
        cell.timer -= 1;
        if cell.timer == 0 {
            if on && !input {
                cell.power = 0;
            } else if !on {
                cell.power = 15;
                if !input {
                    // A short pulse still comes out `delay` steps long.
                    cell.timer = delay;
                }
            }
        }
        cell
    }
}

// ── Nested ALUs ──────────────────────────────────────────────────────────────

/// A pin of a nested ALU's board: a side as a source (`In`) or a sink
/// (`Out`), by its index in N S E W U D, or a chip's port by its slot and label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NestedPin {
    In(u8),
    Out(u8),
    Chip(u8, String),
}

/// A port of a chip in a nested ALU.
#[derive(Debug, Clone)]
pub struct NestedPort {
    pub label: String,
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub mode: PortMode,
}

/// A chip in a nested ALU: its ports, its circuit, and the steps its tier
/// gives it each server tick. However many steps the circuit around it
/// takes in a tick, it takes no more than its own — a slow chip stays slow
/// inside a fast one, a fast one isn't sped up by it.
pub struct NestedChip {
    pub slot: u8,
    pub ports: Vec<NestedPort>,
    pub budget: u32,
    pub vm: RedstoneVM,
    /// Steps it has left this tick.
    left: u32,
    /// Settled with these inputs: nothing to do until they change.
    settled_on: Option<Vec<u8>>,
}

impl NestedChip {
    pub fn new(slot: u8, ports: Vec<NestedPort>, budget: u32, vm: RedstoneVM) -> Self {
        NestedChip { slot, ports, budget, vm, left: budget, settled_on: None }
    }
}

/// An ALU inside a circuit: its chips and the wires of its board, run each
/// step of the circuit around it — as the ALU it was in the world.
pub struct NestedAlu {
    pub chips: Vec<NestedChip>,
    pub wires: Vec<(NestedPin, NestedPin)>,
}

impl NestedAlu {
    /// One run: the faces' power in, the chips settle, the faces' levels out.
    fn run(&mut self, inputs: [u8; 6]) -> [u8; 6] {
        let mut levels: HashMap<NestedPin, u8> = HashMap::new();
        for (f, &l) in inputs.iter().enumerate() {
            levels.insert(NestedPin::In(f as u8), l);
        }
        let outputs = |chips: &[NestedChip], levels: &mut HashMap<NestedPin, u8>| {
            for c in chips {
                for p in c.ports.iter().filter(|p| p.mode.outputs()) {
                    levels.insert(NestedPin::Chip(c.slot, p.label.clone()), c.vm.get_port_output(p.x, p.y, p.z));
                }
            }
        };
        outputs(&self.chips, &mut levels);
        let into = |wires: &[(NestedPin, NestedPin)], levels: &HashMap<NestedPin, u8>, to: &NestedPin| {
            wires.iter().find(|(_, t)| t == to).and_then(|(f, _)| levels.get(f)).copied().unwrap_or(0)
        };
        for c in &mut self.chips {
            let ins: Vec<u8> = c
                .ports
                .iter()
                .filter(|p| p.mode.inputs())
                .map(|p| into(&self.wires, &levels, &NestedPin::Chip(c.slot, p.label.clone())))
                .collect();
            if c.settled_on.as_ref() == Some(&ins) {
                continue;
            }
            for (p, &level) in c.ports.iter().filter(|p| p.mode.inputs()).zip(&ins) {
                c.vm.set_port_input(p.x, p.y, p.z, level);
            }
            // Out of steps this tick: it waits for the next, as it would in the world.
            if c.left == 0 {
                continue;
            }
            let used = c.vm.settle(c.left);
            let settled = used < c.left;
            c.left -= used;
            c.settled_on = settled.then_some(ins);
        }
        outputs(&self.chips, &mut levels);
        std::array::from_fn(|f| into(&self.wires, &levels, &NestedPin::Out(f as u8)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVER_ON: BlockType = BlockType::Lever { on: true, attach: Facing::Down };
    const TORCH: BlockType = BlockType::RedstoneTorch { lit: true, attach: Facing::Down };

    fn line(vm: &mut RedstoneVM, z: u32, from: u32, to: u32) {
        for x in from..to {
            vm.set_block(x, 0, z, BlockType::RedstoneWire);
        }
    }

    /// A repeater on row z = 0 sending east (it faces its input, west).
    fn repeater(delay: u8) -> BlockType {
        BlockType::Repeater { delay_ticks: delay, facing: Facing::West }
    }

    #[test]
    fn lever_powers_wire_with_decay() {
        let mut vm = RedstoneVM::new(32);
        vm.set_block(0, 0, 0, LEVER_ON);
        line(&mut vm, 0, 1, 20);
        vm.step();
        assert_eq!(vm.cell(1, 0, 0).power, 15);
        assert_eq!(vm.cell(15, 0, 0).power, 1);
        assert_eq!(vm.cell(16, 0, 0).power, 0);
    }

    #[test]
    fn wire_climbs_and_descends_steps() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, LEVER_ON);
        vm.set_block(1, 0, 0, BlockType::RedstoneWire);
        vm.set_block(2, 0, 0, BlockType::Solid);
        vm.set_block(2, 1, 0, BlockType::RedstoneWire);
        vm.set_block(3, 0, 0, BlockType::RedstoneWire);
        vm.step();
        assert_eq!(vm.cell(2, 1, 0).power, 14, "up the step");
        assert_eq!(vm.cell(3, 0, 0).power, 13, "and down again");

        // A block over the lower wire cuts the step.
        vm.set_block(1, 1, 0, BlockType::Solid);
        vm.step();
        assert_eq!(vm.cell(2, 1, 0).power, 0);
    }

    #[test]
    fn wire_powers_the_block_under_it() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(1, 0, 0, BlockType::Solid);
        vm.set_block(0, 1, 0, LEVER_ON);
        vm.set_block(1, 1, 0, BlockType::RedstoneWire);
        vm.step();
        assert_eq!(vm.cell(1, 0, 0).weak, 15);
        assert_eq!(vm.cell(1, 0, 0).strong, 0);
    }

    #[test]
    fn wire_does_not_run_on_through_a_block() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, LEVER_ON);
        vm.set_block(1, 0, 0, BlockType::RedstoneWire);
        vm.set_block(2, 0, 0, BlockType::Solid);
        vm.set_block(3, 0, 0, BlockType::RedstoneWire);
        vm.step();
        assert_eq!(vm.cell(2, 0, 0).weak, 15, "the wire points into it");
        assert_eq!(vm.cell(3, 0, 0).power, 0);
    }

    #[test]
    fn wire_only_powers_where_it_points() {
        let mut vm = RedstoneVM::new(16);
        // Wire running east–west past a lamp to its north.
        vm.set_block(0, 0, 1, LEVER_ON);
        line(&mut vm, 1, 1, 4);
        vm.set_block(2, 0, 0, BlockType::RedstoneLamp);
        vm.step();
        assert_eq!(vm.cell(2, 0, 0).power, 0, "the line passes the lamp by");
        // The line's end points on into it.
        vm.set_block(4, 0, 1, BlockType::RedstoneLamp);
        vm.step();
        assert_eq!(vm.cell(4, 0, 1).power, 15);
    }

    #[test]
    fn input_port_drives_output_port() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
        line(&mut vm, 0, 1, 5);
        vm.set_block(5, 0, 0, BlockType::Port(PortMode::Output));
        vm.step();
        assert_eq!(vm.get_port_output(5, 0, 0), 0);

        vm.set_port_input(0, 0, 0, 15);
        vm.step();
        assert_eq!(vm.get_port_output(5, 0, 0), 12);
        assert_eq!(vm.read_outputs(0), vec![(5, 0, 12)]);

        vm.set_port_input(0, 0, 0, 0);
        vm.step();
        assert_eq!(vm.get_port_output(5, 0, 0), 0);
    }

    #[test]
    fn torch_inverts_the_block_it_stands_on() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(1, 0, 0, BlockType::Solid);
        vm.set_block(1, 1, 0, TORCH);
        vm.step();
        assert_eq!(vm.cell(1, 1, 0).power, 15);

        // A lever on the block's side (a lever beside it on the floor wouldn't).
        vm.set_block(0, 0, 0, BlockType::Lever { on: true, attach: Facing::East });
        vm.step();
        assert_eq!(vm.cell(1, 1, 0).power, 0);
        assert!(matches!(vm.cell(1, 1, 0).block, BlockType::RedstoneTorch { lit: false, .. }));
    }

    #[test]
    fn torch_strongly_powers_the_block_above() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(1, 1, 0, TORCH);
        vm.set_block(1, 2, 0, BlockType::Solid);
        vm.set_block(2, 2, 0, BlockType::RedstoneWire);
        vm.step();
        assert_eq!(vm.cell(1, 2, 0).strong, 15);
        assert_eq!(vm.cell(2, 2, 0).power, 15, "a strongly powered block feeds wire");
    }

    #[test]
    fn wall_torch_hangs_on_its_wall() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, BlockType::Lever { on: true, attach: Facing::East });
        vm.set_block(1, 0, 0, BlockType::Solid);
        // facing=east: the wall is to its west.
        vm.set_block(2, 0, 0, BlockType::RedstoneTorch { lit: true, attach: Facing::West });
        vm.step();
        assert_eq!(vm.cell(2, 0, 0).power, 0);
    }

    #[test]
    fn repeater_faces_its_input() {
        // facing=east: reads from the east, sends west.
        let mut vm = RedstoneVM::new(16);
        vm.set_block(3, 0, 0, LEVER_ON);
        vm.set_block(2, 0, 0, BlockType::Repeater { delay_ticks: 1, facing: Facing::East });
        vm.set_block(1, 0, 0, BlockType::RedstoneWire);
        vm.set_block(4, 0, 0, BlockType::RedstoneWire);
        vm.step();
        vm.step();
        assert_eq!(vm.cell(1, 0, 0).power, 15);

        // Turned the other way it sends nothing west.
        vm.set_block(2, 0, 0, BlockType::Repeater { delay_ticks: 1, facing: Facing::West });
        vm.step();
        vm.step();
        assert_eq!(vm.cell(1, 0, 0).power, 0);
    }

    #[test]
    fn repeater_waits_its_delay() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, LEVER_ON);
        vm.set_block(1, 0, 0, BlockType::RedstoneWire);
        vm.set_block(2, 0, 0, repeater(3));
        line(&mut vm, 0, 3, 6);
        let powered_at: Vec<bool> = (0..5)
            .map(|_| {
                vm.step();
                vm.cell(3, 0, 0).power > 0
            })
            .collect();
        // Input seen in step 1, switched at the end of step 3, on the wire in step 4.
        assert_eq!(powered_at, [false, false, false, true, true]);
        assert_eq!(vm.cell(3, 0, 0).power, 15);
    }

    #[test]
    fn repeater_stretches_a_short_pulse() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, LEVER_ON);
        vm.set_block(1, 0, 0, repeater(4));
        vm.step();
        vm.set_block(0, 0, 0, BlockType::Lever { on: false, attach: Facing::Down });
        let on: Vec<bool> = (0..10)
            .map(|_| {
                vm.step();
                vm.cell(1, 0, 0).power > 0
            })
            .collect();
        assert_eq!(on.iter().filter(|o| **o).count(), 4, "{on:?}");
    }

    #[test]
    fn repeater_locked_by_its_side() {
        let mut vm = RedstoneVM::new(16);
        // The locking repeater at (1, 0, 1) sends south into the side of
        // the one at (1, 0, 2), which sends east.
        vm.set_block(1, 0, 0, LEVER_ON);
        vm.set_block(1, 0, 1, BlockType::Repeater { delay_ticks: 1, facing: Facing::North });
        vm.set_block(1, 0, 2, repeater(1));
        vm.step();
        vm.step();
        vm.set_block(0, 0, 2, LEVER_ON);
        vm.step();
        vm.step();
        assert_eq!(vm.cell(1, 0, 2).power, 0, "locked off");
        vm.set_block(1, 0, 0, BlockType::Lever { on: false, attach: Facing::Down });
        vm.step();
        vm.step();
        vm.step();
        assert_eq!(vm.cell(1, 0, 2).power, 15, "unlocked, it takes its input");
    }

    #[test]
    fn comparator_compares_and_subtracts() {
        let run = |mode| {
            let mut vm = RedstoneVM::new(16);
            vm.set_block(0, 0, 1, BlockType::RedstoneBlock);
            vm.set_block(1, 0, 1, BlockType::Comparator { mode, facing: Facing::West });
            vm.set_block(1, 0, 0, BlockType::RedstoneWire);
            vm.set_block(1, 0, 0, BlockType::RedstoneWire);
            vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
            vm.set_port_input(0, 0, 0, 7);
            vm.step();
            vm.cell(1, 0, 1).power
        };
        assert_eq!(run(ComparatorMode::Compare), 15);
        assert_eq!(run(ComparatorMode::Subtract), 8);
    }

    #[test]
    fn observer_pulses_once_per_change() {
        let mut vm = RedstoneVM::new(16);
        // Watches east, outputs west.
        vm.set_block(1, 0, 0, BlockType::Observer { facing: Facing::East });
        vm.set_block(0, 0, 0, BlockType::RedstoneWire);
        vm.step();
        vm.step();
        vm.set_block(2, 0, 0, BlockType::Solid);
        let out: Vec<u8> = (0..4)
            .map(|_| {
                vm.step();
                vm.cell(0, 0, 0).power
            })
            .collect();
        assert_eq!(out, [0, 15, 0, 0]);
    }

    fn piston(facing: Facing, sticky: bool) -> BlockType {
        BlockType::Piston { facing, sticky, extended: false }
    }

    #[test]
    fn piston_pushes_and_breaks() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(1, 0, 0, piston(Facing::East, false));
        vm.set_block(2, 0, 0, BlockType::Solid);
        vm.set_block(3, 0, 0, BlockType::RedstoneBlock);
        vm.set_block(4, 0, 0, BlockType::RedstoneWire);
        vm.set_block(0, 0, 0, LEVER_ON);
        vm.step();
        assert!(matches!(vm.cell(1, 0, 0).block, BlockType::Piston { extended: true, .. }));
        assert!(matches!(vm.cell(2, 0, 0).block, BlockType::PistonHead { .. }));
        assert_eq!(vm.cell(3, 0, 0).block, BlockType::Solid);
        assert_eq!(vm.cell(4, 0, 0).block, BlockType::RedstoneBlock, "the wire broke");

        vm.set_block(0, 0, 0, BlockType::Lever { on: false, attach: Facing::Down });
        vm.step();
        assert!(matches!(vm.cell(1, 0, 0).block, BlockType::Piston { extended: false, .. }));
        assert_eq!(vm.cell(2, 0, 0).block, BlockType::Air, "a plain piston leaves it");
    }

    #[test]
    fn piston_stops_at_obsidian_and_twelve_blocks() {
        let mut vm = RedstoneVM::new(32);
        vm.set_block(1, 0, 0, piston(Facing::East, false));
        vm.set_block(2, 0, 0, BlockType::Solid);
        vm.set_block(3, 0, 0, BlockType::Immovable);
        vm.set_block(0, 0, 0, LEVER_ON);
        vm.step();
        assert!(matches!(vm.cell(1, 0, 0).block, BlockType::Piston { extended: false, .. }));

        let mut vm = RedstoneVM::new(32);
        vm.set_block(1, 0, 0, piston(Facing::East, false));
        for x in 2..15 {
            vm.set_block(x, 0, 0, BlockType::Solid);
        }
        vm.set_block(0, 0, 0, LEVER_ON);
        vm.step();
        assert!(matches!(vm.cell(1, 0, 0).block, BlockType::Piston { extended: false, .. }), "13 is too many");
    }

    #[test]
    fn sticky_piston_pulls_and_slime_drags() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(1, 0, 1, piston(Facing::East, true));
        vm.set_block(2, 0, 1, BlockType::Slime);
        vm.set_block(2, 0, 2, BlockType::Solid);
        vm.set_block(0, 0, 1, LEVER_ON);
        vm.step();
        assert_eq!(vm.cell(3, 0, 1).block, BlockType::Slime);
        assert_eq!(vm.cell(3, 0, 2).block, BlockType::Solid, "dragged by the slime");
        vm.set_block(0, 0, 1, BlockType::Lever { on: false, attach: Facing::Down });
        vm.step();
        assert_eq!(vm.cell(2, 0, 1).block, BlockType::Slime, "pulled back");
        assert_eq!(vm.cell(2, 0, 2).block, BlockType::Solid);
    }

    #[test]
    fn quasi_connectivity() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(1, 0, 0, piston(Facing::East, false));
        // Powering the space above it.
        vm.set_block(0, 1, 0, LEVER_ON);
        vm.step();
        assert!(matches!(vm.cell(1, 0, 0).block, BlockType::Piston { extended: true, .. }));
    }

    #[test]
    fn hopper_moves_items_and_comparator_reads_them() {
        let mut vm = RedstoneVM::new(16);
        let chest = BlockType::Container { slots: 27, solid: false, partner: None };
        vm.set_block(1, 1, 0, chest);
        vm.set_items(1, 1, 0, &[(0, "minecraft:stone".into(), 64)]);
        vm.set_block(1, 0, 0, BlockType::Hopper { facing: Facing::East });
        vm.set_block(2, 0, 0, BlockType::Hopper { facing: Facing::Down });
        // A comparator reading the second hopper, facing it (west).
        vm.set_block(3, 0, 0, BlockType::Comparator { mode: ComparatorMode::Compare, facing: Facing::West });
        // Pulled in step 1, passed on 4 steps later.
        for _ in 0..5 {
            vm.step();
        }
        assert_eq!(vm.items_at(1, 1, 0), [(0, "minecraft:stone".into(), 62)]);
        assert_eq!(vm.items_at(2, 0, 0), [(0, "minecraft:stone".into(), 1)], "{:?}", vm.items_at(1, 0, 0));
        vm.step();
        assert_eq!(vm.cell(3, 0, 0).power, 1);

        // Powered, a hopper stops.
        vm.set_block(1, 0, 1, BlockType::RedstoneBlock);
        let before = vm.items_at(1, 1, 0);
        for _ in 0..10 {
            vm.step();
        }
        assert_eq!(vm.items_at(1, 1, 0), before);
    }

    #[test]
    fn dropper_drops_on_a_rising_edge() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(1, 0, 0, BlockType::Dropper { facing: Facing::East, triggered: false });
        vm.set_items(1, 0, 0, &[(0, "minecraft:stone".into(), 2)]);
        vm.set_block(2, 0, 0, BlockType::Container { slots: 27, solid: false, partner: None });
        vm.set_block(0, 0, 0, LEVER_ON);
        for _ in 0..6 {
            vm.step();
        }
        assert_eq!(vm.items_at(2, 0, 0), [(0, "minecraft:stone".into(), 1)], "once while held on");
    }

    #[test]
    fn air_costs_no_layers() {
        let mut vm = RedstoneVM::new(256);
        assert_eq!(vm.memory_bytes(), 0);
        vm.set_block(3, 7, 3, BlockType::RedstoneLamp);
        assert_eq!(vm.layers.iter().filter(|l| l.is_some()).count(), 1);
        assert_eq!(vm.block_count(), 1);
        vm.set_block(3, 7, 3, BlockType::Air);
        assert_eq!(vm.block_count(), 0);
        vm.step();
        vm.set_block(300, 0, 0, BlockType::Solid);
        assert_eq!(vm.block_count(), 0);
    }

    #[test]
    fn active_cells_stay_in_grid_order() {
        let mut vm = RedstoneVM::new(16);
        for (x, y, z) in [(5, 2, 1), (0, 0, 0), (3, 2, 0), (15, 15, 15), (1, 0, 9)] {
            vm.set_block(x, y, z, BlockType::Solid);
        }
        assert!(vm.active.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(vm.pos(vm.active[4]), (15, 15, 15));
    }

    #[test]
    fn a_not_gate_answers_in_one_settle_and_a_quiet_circuit_stops_early() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
        vm.set_block(1, 0, 0, BlockType::RedstoneWire);
        vm.set_block(2, 0, 0, BlockType::Solid);
        vm.set_block(3, 0, 0, BlockType::RedstoneTorch { lit: true, attach: Facing::West });
        vm.set_block(4, 0, 0, BlockType::Port(PortMode::Output));
        vm.settle(8);
        assert_eq!(vm.get_port_output(4, 0, 0), 15);
        vm.set_port_input(0, 0, 0, 15);
        let steps = vm.settle(8);
        assert_eq!(vm.get_port_output(4, 0, 0), 0, "answered the same tick");
        assert!(steps < 8, "stopped once nothing changed ({steps})");
        assert_eq!(vm.settle(8), 0, "settled, inputs as they were: not a step");
        vm.set_port_input(0, 0, 0, 15);
        assert_eq!(vm.settle(8), 0, "the same input isn't a change");
        vm.set_port_input(0, 0, 0, 0);
        assert!(vm.settle(8) > 0, "a new one is");
        assert_eq!(vm.get_port_output(4, 0, 0), 15);
    }

    /// A NOT gate: port IN at x 0 → wire → block → torch → port OUT at x 4.
    fn not_chip(budget: u32) -> NestedChip {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
        vm.set_block(1, 0, 0, BlockType::RedstoneWire);
        vm.set_block(2, 0, 0, BlockType::Solid);
        vm.set_block(3, 0, 0, BlockType::RedstoneTorch { lit: true, attach: Facing::West });
        vm.set_block(4, 0, 0, BlockType::Port(PortMode::Output));
        vm.settle(8);
        let port = |label: &str, x, mode| NestedPort { label: label.into(), x, y: 0, z: 0, mode };
        NestedChip::new(0, vec![port("IN", 0, PortMode::Input), port("OUT", 4, PortMode::Output)], budget, vm)
    }

    /// An outer circuit: input port, the ALU (its west face in, its east face out), output port.
    fn outer(budget: u32) -> RedstoneVM {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
        vm.set_block(1, 0, 0, BlockType::Alu);
        vm.set_block(2, 0, 0, BlockType::Port(PortMode::Output));
        let wires = vec![
            (NestedPin::In(Facing::West.index() as u8), NestedPin::Chip(0, "IN".into())),
            (NestedPin::Chip(0, "OUT".into()), NestedPin::Out(Facing::East.index() as u8)),
        ];
        vm.set_nested(1, 0, 0, NestedAlu { chips: vec![not_chip(budget)], wires });
        vm.new_tick();
        vm.settle(8);
        vm
    }

    #[test]
    fn cables_and_redstone_ports_carry_an_alus_faces_inside_a_chip() {
        // IN → wire → port (input) → cable → ALU (a NOT) → cable → port (output) → wire → OUT.
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
        vm.set_block(1, 0, 0, BlockType::RedstoneWire);
        vm.set_block(2, 0, 0, BlockType::RedstonePort { output: false });
        vm.set_block(3, 0, 0, BlockType::Cable);
        vm.set_block(4, 0, 0, BlockType::Alu);
        vm.set_block(5, 0, 0, BlockType::Cable);
        vm.set_block(6, 0, 0, BlockType::Cable);
        vm.set_block(7, 0, 0, BlockType::RedstonePort { output: true });
        vm.set_block(8, 0, 0, BlockType::RedstoneWire);
        vm.set_block(9, 0, 0, BlockType::Port(PortMode::Output));
        let wires = vec![
            (NestedPin::In(Facing::West.index() as u8), NestedPin::Chip(0, "IN".into())),
            (NestedPin::Chip(0, "OUT".into()), NestedPin::Out(Facing::East.index() as u8)),
        ];
        vm.set_nested(4, 0, 0, NestedAlu { chips: vec![not_chip(8)], wires });
        vm.new_tick();
        vm.settle(128);
        assert_eq!(vm.get_port_output(9, 0, 0), 15, "NOT of nothing, out through the cable");
        vm.set_port_input(0, 0, 0, 15);
        for _ in 0..3 {
            vm.new_tick();
            vm.settle(128);
        }
        assert_eq!(vm.get_port_output(9, 0, 0), 0, "in through the cable too");
        vm.set_port_input(0, 0, 0, 0);
        for _ in 0..3 {
            vm.new_tick();
            vm.settle(128);
        }
        assert_eq!(vm.get_port_output(9, 0, 0), 15, "and back: a cable holds no level of its own");
    }

    #[test]
    fn an_alu_in_a_circuit_runs_its_chips_the_same_tick() {
        let mut vm = outer(8);
        assert_eq!(vm.get_port_output(2, 0, 0), 15, "NOT of nothing");
        vm.set_port_input(0, 0, 0, 15);
        vm.new_tick();
        vm.settle(128);
        assert_eq!(vm.get_port_output(2, 0, 0), 0, "answered this tick");
    }

    #[test]
    fn a_nested_chip_keeps_its_own_speed_however_fast_the_circuit_around_it() {
        // One step a tick for the inner chip; a NOT gate needs two.
        let mut vm = outer(1);
        vm.new_tick();
        vm.settle(128);
        vm.new_tick();
        vm.settle(128);
        let before = vm.get_port_output(2, 0, 0);
        vm.set_port_input(0, 0, 0, if before == 15 { 15 } else { 0 });
        vm.new_tick();
        vm.settle(128);
        assert_eq!(vm.get_port_output(2, 0, 0), before, "128 steps around it, still one of its own: not yet");
        vm.new_tick();
        vm.settle(128);
        vm.new_tick();
        vm.settle(128);
        assert_ne!(vm.get_port_output(2, 0, 0), before, "its own steps, a tick at a time");
    }
}
