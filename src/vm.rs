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
//! Self-contained (std + serde only) so `benches/` can build it alone.

use serde::{Deserialize, Serialize};

// ── Block types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// A VLSI port block.
    Port(PortMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparatorMode {
    Compare,
    Subtract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        Cell { block, power, strong: 0, weak: 0, timer: 0, port_in: 0, watched: 0 }
    }

    fn is_solid(&self) -> bool {
        matches!(self.block, BlockType::Solid | BlockType::RedstoneLamp)
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
            | BlockType::Port(_) => true,
            _ => false,
        }
    }

    /// The (weak, strong) power this component gives the neighbour in
    /// direction `to`. Not for wire or solid blocks.
    fn emits(&self, to: Facing) -> (u8, u8) {
        let p = self.power;
        match self.block {
            BlockType::RedstoneBlock | BlockType::TargetBlock { .. } | BlockType::Source { .. } => (p, 0),
            BlockType::Lever { attach, .. } | BlockType::Button { attach, .. } => (p, if to == attach { p } else { 0 }),
            BlockType::PressurePlate { .. } => (p, if to == Facing::Down { p } else { 0 }),
            BlockType::RedstoneTorch { attach, .. } => {
                (if to == attach { 0 } else { p }, if to == Facing::Up { p } else { 0 })
            }
            BlockType::Repeater { facing, .. } | BlockType::Comparator { facing, .. } | BlockType::Observer { facing } => {
                if to == facing.opposite() { (p, p) } else { (0, 0) }
            }
            BlockType::Port(PortMode::Input | PortMode::Bidirectional) => (self.port_in, 0),
            _ => (0, 0),
        }
    }

    /// What an observer sees of this cell.
    fn looks(&self) -> u32 {
        let id = std::mem::discriminant(&self.block);
        let mut h = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(&id, &mut h);
        let shown = match self.block {
            BlockType::Solid | BlockType::Transparent | BlockType::Air => 0,
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
}

impl RedstoneVM {
    /// A world `size` cells on each side (rounded up to a power of two;
    /// every tier's is one).
    pub fn new(size: u32) -> Self {
        let size = size.max(1).next_power_of_two();
        RedstoneVM { size, shift: size.trailing_zeros(), layers: vec![None; size as usize], active: Vec::new(), tick: 0 }
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
    pub fn block_count(&self) -> usize {
        self.active.len()
    }

    /// Bytes held by the grid (allocated layers + the active index).
    pub fn memory_bytes(&self) -> usize {
        let layer = self.layer_len() as usize * std::mem::size_of::<Cell>();
        self.layers.iter().flatten().count() * layer + self.active.capacity() * 4
    }

    pub fn set_block(&mut self, x: u32, y: u32, z: u32, block: BlockType) {
        if !self.in_bounds(x as i32, y as i32, z as i32) {
            return;
        }
        let i = self.idx(x, y, z);
        let found = self.active.binary_search(&i);
        if block == BlockType::Air {
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
        let cells = self.layers[y as usize].get_or_insert_with(|| vec![AIR; w].into_boxed_slice());
        cells[i as usize % w] = Cell::new(block);
    }

    /// Drives `power` into the Input/Bidirectional port at `(x, y, z)`.
    pub fn set_port_input(&mut self, x: u32, y: u32, z: u32, power: u8) {
        if !self.in_bounds(x as i32, y as i32, z as i32) {
            return;
        }
        let i = self.idx(x, y, z);
        if matches!(self.get(i).block, BlockType::Port(PortMode::Input | PortMode::Bidirectional)) {
            self.get_mut(i).port_in = power.min(15);
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
    pub fn read_outputs(&self, y: u32) -> Vec<(u32, u32, u8)> {
        self.active
            .iter()
            .map(|&i| (i, self.pos(i)))
            .filter(|&(i, (_, cy, _))| cy == y && self.get(i).is_output_port())
            .map(|(i, (x, _, z))| (x, z, self.get(i).power))
            .collect()
    }

    pub fn step(&mut self) {
        self.tick += 1;
        self.strong_power();
        self.settle_wire();
        self.wire_power();
        self.read_power();
        self.settle_components();
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

    /// Wire `i` powers the block in direction `d`: always the one below;
    /// sideways where it points (a lone dot points everywhere, a dot turned
    /// one way runs straight through).
    fn wire_points(&self, i: u32, d: Facing) -> bool {
        match d {
            Facing::Down => true,
            Facing::Up => false,
            _ => {
                let turns: Vec<Facing> = Facing::HORIZONTAL.into_iter().filter(|&h| self.wire_turns(i, h)).collect();
                match turns.len() {
                    0 => true,
                    1 => turns[0] == d || turns[0] == d.opposite(),
                    _ => turns.contains(&d),
                }
            }
        }
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
    /// along it strongest first, one less per block.
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

    /// 5. Every component decides its next output.
    fn settle_components(&mut self) {
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
                        let back = self.input_from(i, facing, true);
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
                    _ => return None,
                };
                (next.block != cell.block || next.power != cell.power || next.timer != cell.timer || next.watched != cell.watched)
                    .then_some((i, next))
            })
            .collect();
        for (i, next) in changes {
            *self.get_mut(i) = next;
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
}
