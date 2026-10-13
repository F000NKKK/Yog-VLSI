//! Rust redstone VM: the simulation behind every installed microchip
//! (DESIGN.md §5).
//!
//! A chip's virtual world is a `size`³ grid. Only the cells that hold a
//! block take part in a step: they are kept in [`RedstoneVM::active`] in
//! grid order (y, then z, then x — the order the original full-grid scans
//! visited them in), and grid layers are only allocated once something is
//! placed in them, so an empty Netherite chip costs nothing and a one-layer
//! circuit costs one layer.
//!
//! A step (one redstone tick):
//! 1. wires lose their power and blocks their weak power;
//! 2. every source (levers, lit torches, repeaters and comparators that are
//!    on, input ports, …) spreads its level through wires, one less per
//!    wire, weakly powering the solid blocks it touches (and the block under
//!    each wire);
//! 3. output ports read the strongest signal next to them;
//! 4. solid blocks next to an active repeater, torch or comparator become
//!    strongly powered;
//! 5. components look at what's around them now and set their output for
//!    the next step: torches invert the block they hang on, repeaters
//!    latch their input after `delay` steps, comparators compare or
//!    subtract, observers end their pulse.
//!
//! Components settling at the end of a step is what gives them their one
//! tick of delay.
//!
//! Self-contained (std + serde only) so `benches/` can build it alone.
//!
//! Supported blocks: redstone wire, torches, repeaters, comparators, levers,
//! buttons, pressure plates, pistons (sticky + normal), observers, note
//! blocks, lamps, redstone blocks, target blocks, hoppers, droppers,
//! dispensers, chests, trapped chests, shulker boxes, slime blocks, honey
//! blocks, solid/conductor blocks, glass, and I/O ports.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

// ── Block types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    Air,

    // Solid / decorative
    Solid,
    Glass,

    // Redstone components
    RedstoneWire,
    RedstoneTorch { lit: bool },
    RedstoneWallTorch { lit: bool, facing: Facing },
    Repeater { delay_ticks: u8, facing: Facing, locked: bool },
    Comparator { mode: ComparatorMode, facing: Facing },
    RedstoneLamp,
    RedstoneBlock,
    Lever { on: bool },
    StoneButton { pressed: bool, facing: Facing },
    WoodButton { pressed: bool, facing: Facing },
    StonePressurePlate { pressed: bool },
    WoodPressurePlate { pressed: bool },
    LightWeightedPressurePlate { power: u8 },
    HeavyWeightedPressurePlate { power: u8 },
    Observer { facing: Facing, powered: bool },
    NoteBlock,
    TargetBlock { power: u8 },
    Piston { extended: bool, facing: Facing },
    StickyPiston { extended: bool, facing: Facing },

    // Containers (inventory not simulated, but they conduct/block redstone)
    Chest,
    TrappedChest,
    EnderChest,
    ShulkerBox,
    Barrel,
    Hopper { facing: Facing, enabled: bool },
    Dropper { facing: Facing, triggered: bool },
    Dispenser { facing: Facing, triggered: bool },
    Furnace { lit: bool },
    BlastFurnace { lit: bool },
    Smoker { lit: bool },
    BrewingStand,

    // Movement / utility
    SlimeBlock,
    HoneyBlock,
    Tnt { unstable: bool },
    IronDoor { open: bool, facing: Facing, half: DoorHalf },
    WoodDoor { open: bool, facing: Facing, half: DoorHalf },
    IronTrapdoor { open: bool, facing: Facing, half: DoorHalf },
    WoodTrapdoor { open: bool, facing: Facing, half: DoorHalf },
    FenceGate { open: bool, facing: Facing },
    Rail,
    PoweredRail { powered: bool },
    DetectorRail { powered: bool },
    ActivatorRail { powered: bool },

    // I/O ports (the VLSI port blocks)
    Port(PortMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparatorMode {
    Compare,
    Subtract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoorHalf {
    Upper,
    Lower,
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
    pub fn horizontal_offset(self) -> (i32, i32) {
        match self {
            Facing::North => (0, -1),
            Facing::South => (0, 1),
            Facing::East => (1, 0),
            Facing::West => (-1, 0),
            _ => (0, 0),
        }
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

/// The four horizontal neighbours.
const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

// ── Cell ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub block: BlockType,
    pub power: u8,
    pub strongly_powered: bool,
    pub weakly_powered: bool,
    pub repeater_timer: u8,
    /// Value driven into an Input/Bidirectional port from outside the chip.
    pub port_in: u8,
}

const AIR: Cell = Cell::new(BlockType::Air);

impl Cell {
    pub const fn new(block: BlockType) -> Self {
        Cell {
            block,
            power: 0,
            strongly_powered: false,
            weakly_powered: false,
            repeater_timer: 0,
            port_in: 0,
        }
    }

    fn is_solid(&self) -> bool {
        matches!(
            self.block,
            BlockType::Solid
                | BlockType::RedstoneLamp
                | BlockType::RedstoneBlock
                | BlockType::NoteBlock
                | BlockType::TargetBlock { .. }
                | BlockType::Piston { .. }
                | BlockType::StickyPiston { .. }
                | BlockType::Chest
                | BlockType::TrappedChest
                | BlockType::EnderChest
                | BlockType::ShulkerBox
                | BlockType::Barrel
                | BlockType::Hopper { .. }
                | BlockType::Dropper { .. }
                | BlockType::Dispenser { .. }
                | BlockType::Furnace { .. }
                | BlockType::BlastFurnace { .. }
                | BlockType::Smoker { .. }
                | BlockType::BrewingStand
                | BlockType::Observer { .. }
                | BlockType::SlimeBlock
                | BlockType::HoneyBlock
                | BlockType::IronDoor { .. }
                | BlockType::WoodDoor { .. }
                | BlockType::IronTrapdoor { .. }
                | BlockType::WoodTrapdoor { .. }
                | BlockType::FenceGate { .. }
        )
    }

    /// Power level a source outputs.
    fn source_power(&self) -> u8 {
        match self.block {
            BlockType::RedstoneTorch { lit: true }
            | BlockType::RedstoneWallTorch { lit: true, .. }
            | BlockType::Lever { on: true }
            | BlockType::StoneButton { pressed: true, .. }
            | BlockType::WoodButton { pressed: true, .. }
            | BlockType::RedstoneBlock
            | BlockType::Observer { powered: true, .. }
            | BlockType::DetectorRail { powered: true } => 15,
            BlockType::Repeater { .. } | BlockType::Comparator { .. } => self.power,
            BlockType::TargetBlock { power }
            | BlockType::LightWeightedPressurePlate { power }
            | BlockType::HeavyWeightedPressurePlate { power } => power,
            // Simplified: as if one player had it open.
            BlockType::TrappedChest => 1,
            BlockType::Port(PortMode::Input | PortMode::Bidirectional) => self.port_in,
            _ => 0,
        }
    }

    /// The level a neighbour reads from this cell: its power, its output
    /// as a source, or 15 when strongly powered.
    fn signal(&self) -> u8 {
        let strong = if self.strongly_powered { 15 } else { 0 };
        self.power.max(self.source_power()).max(strong)
    }

    /// Powered in any way (for torches and repeaters reading their input).
    fn is_powered(&self) -> bool {
        self.signal() > 0 || self.weakly_powered
    }

    fn is_output_port(&self) -> bool {
        matches!(self.block, BlockType::Port(PortMode::Output | PortMode::Bidirectional))
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
        RedstoneVM {
            size,
            shift: size.trailing_zeros(),
            layers: vec![None; size as usize],
            active: Vec::new(),
            tick: 0,
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

    /// The horizontal neighbour `(dx, dz)` of cell `i`, if inside the world.
    #[inline]
    fn side(&self, i: u32, (dx, dz): (i32, i32)) -> Option<u32> {
        let mask = self.size - 1;
        let x = (i & mask).wrapping_add_signed(dx);
        let z = ((i >> self.shift) & mask).wrapping_add_signed(dz);
        (x <= mask && z <= mask).then(|| (i & !(mask | mask << self.shift)) | x | z << self.shift)
    }

    /// The cell below `i`, if any.
    #[inline]
    fn below(&self, i: u32) -> Option<u32> {
        i.checked_sub(self.layer_len())
    }

    /// The cell above `i`, if any.
    #[inline]
    fn above(&self, i: u32) -> Option<u32> {
        let up = i + self.layer_len();
        (up >> (2 * self.shift) < self.size).then_some(up)
    }

    #[inline]
    fn get(&self, i: u32) -> &Cell {
        match &self.layers[(i >> (2 * self.shift)) as usize] {
            Some(cells) => &cells[(i & (self.layer_len() - 1)) as usize],
            None => &AIR,
        }
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
        if !matches!(self.get(i).block, BlockType::Port(PortMode::Input | PortMode::Bidirectional)) {
            return;
        }
        let cell = self.get_mut(i);
        cell.port_in = power.min(15);
        if cell.block == BlockType::Port(PortMode::Input) {
            cell.power = cell.port_in;
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

    pub fn step(&mut self) {
        self.tick += 1;
        self.reset_power();
        self.propagate_power();
        self.update_output_ports();
        self.update_solid_powering();
        self.settle_components();
    }

    fn reset_power(&mut self) {
        for k in 0..self.active.len() {
            let i = self.active[k];
            let cell = self.get_mut(i);
            if cell.block == BlockType::RedstoneWire {
                cell.power = 0;
            }
            cell.weakly_powered = false;
        }
    }

    /// Weakly powers the solid block at `i` and its solid horizontal
    /// neighbours.
    fn power_solid_cluster(&mut self, i: u32) {
        self.get_mut(i).weakly_powered = true;
        for d in SIDES {
            if let Some(n) = self.side(i, d) {
                if self.get(n).is_solid() {
                    self.get_mut(n).weakly_powered = true;
                }
            }
        }
    }

    fn propagate_power(&mut self) {
        let mut queue: VecDeque<(u32, u8)> = self
            .active
            .iter()
            .filter_map(|&i| {
                let src = self.get(i).source_power();
                (src > 0).then_some((i, src))
            })
            .collect();

        while let Some((i, power)) = queue.pop_front() {
            if self.get(i).block == BlockType::RedstoneWire {
                if let Some(b) = self.below(i) {
                    if self.get(b).is_solid() {
                        self.power_solid_cluster(b);
                    }
                }
            }
            if power <= 1 {
                continue;
            }
            let next = power - 1;
            for d in SIDES {
                let Some(n) = self.side(i, d) else { continue };
                let neighbour = *self.get(n);
                if neighbour.block == BlockType::RedstoneWire {
                    if neighbour.power < next {
                        self.get_mut(n).power = next;
                        queue.push_back((n, next));
                    }
                } else if neighbour.is_solid() {
                    self.power_solid_cluster(n);
                }
            }
        }
    }

    /// Output and bidirectional ports read the strongest signal next to
    /// them; a bidirectional port also keeps what is driven into it.
    fn update_output_ports(&mut self) {
        for k in 0..self.active.len() {
            let i = self.active[k];
            let cell = *self.get(i);
            if !cell.is_output_port() {
                continue;
            }
            let neighbours = SIDES.iter().filter_map(|&d| self.side(i, d)).chain(self.below(i)).chain(self.above(i));
            let level = neighbours
                .map(|n| self.get(n))
                .filter(|c| !c.is_output_port())
                .map(|c| if c.block == BlockType::RedstoneWire { c.power } else { c.source_power() })
                .fold(cell.port_in, u8::max);
            self.get_mut(i).power = level;
        }
    }

    fn update_solid_powering(&mut self) {
        let strong: Vec<(u32, bool)> = self
            .active
            .iter()
            .filter(|&&i| self.get(i).is_solid())
            .map(|&i| {
                let strong = SIDES.iter().filter_map(|&d| self.side(i, d)).any(|n| {
                    let c = self.get(n);
                    matches!(
                        c.block,
                        BlockType::Repeater { .. }
                            | BlockType::RedstoneTorch { lit: true }
                            | BlockType::RedstoneWallTorch { lit: true, .. }
                            | BlockType::Comparator { .. }
                    ) && c.power > 0
                });
                (i, strong)
            })
            .collect();
        for (i, strong) in strong {
            self.get_mut(i).strongly_powered = strong;
        }
    }

    /// Every component decides its output for the next step from what's
    /// around it now. Decided first, applied after, so the order components
    /// are visited in doesn't matter.
    fn settle_components(&mut self) {
        let changes: Vec<(u32, Cell)> = self
            .active
            .iter()
            .filter_map(|&i| {
                let cell = self.get(i);
                let next = match cell.block {
                    BlockType::RedstoneTorch { .. } | BlockType::RedstoneWallTorch { .. } => self.next_torch(i),
                    BlockType::Repeater { .. } => self.next_repeater(i),
                    BlockType::Comparator { .. } => self.next_comparator(i),
                    BlockType::Observer { facing, powered: true } => {
                        Cell { block: BlockType::Observer { facing, powered: false }, ..Cell::new(cell.block) }
                    }
                    _ => return None,
                };
                (next.block != cell.block
                    || next.power != cell.power
                    || next.strongly_powered != cell.strongly_powered
                    || next.repeater_timer != cell.repeater_timer)
                    .then_some((i, next))
            })
            .collect();
        for (i, next) in changes {
            *self.get_mut(i) = next;
        }
        // Comparators also strongly power the solid block they face.
        for k in 0..self.active.len() {
            let i = self.active[k];
            let cell = *self.get(i);
            if let BlockType::Comparator { facing, .. } = cell.block {
                if let Some(f) = self.side(i, facing.horizontal_offset()) {
                    if self.get(f).is_solid() {
                        self.get_mut(f).strongly_powered = cell.power > 0;
                    }
                }
            }
        }
    }

    /// A torch is lit unless the block it stands on (or hangs from) is
    /// powered.
    fn next_torch(&self, i: u32) -> Cell {
        let mut cell = *self.get(i);
        let attached = match cell.block {
            BlockType::RedstoneWallTorch { facing, .. } => self.side(i, facing.opposite().horizontal_offset()),
            _ => self.below(i),
        };
        let new_lit = !attached.is_some_and(|a| self.get(a).is_powered());
        if let BlockType::RedstoneTorch { lit } | BlockType::RedstoneWallTorch { lit, .. } = &mut cell.block {
            *lit = new_lit;
        }
        cell.power = if new_lit { 15 } else { 0 };
        cell.strongly_powered = new_lit;
        cell
    }

    /// A repeater switches to its input `delay` steps after the input
    /// changed (a change during the delay is taken when it ends).
    fn next_repeater(&self, i: u32) -> Cell {
        let mut cell = *self.get(i);
        let BlockType::Repeater { facing, delay_ticks, .. } = cell.block else { return cell };
        let input = self.side(i, facing.opposite().horizontal_offset()).is_some_and(|n| self.get(n).is_powered());
        let on = cell.power > 0;
        let switch_now = if cell.repeater_timer > 0 {
            cell.repeater_timer -= 1;
            cell.repeater_timer == 0
        } else if input != on {
            cell.repeater_timer = delay_ticks.saturating_sub(1);
            cell.repeater_timer == 0
        } else {
            false
        };
        if switch_now {
            cell.power = if input { 15 } else { 0 };
            cell.strongly_powered = input;
        }
        cell
    }

    /// A comparator outputs its back input if no side input is stronger
    /// (compare), or the back minus the strongest side (subtract).
    fn next_comparator(&self, i: u32) -> Cell {
        let mut cell = *self.get(i);
        let BlockType::Comparator { mode, facing } = cell.block else { return cell };
        let signal = |d: Facing| self.side(i, d.horizontal_offset()).map_or(0, |n| self.get(n).signal());
        let back = signal(facing.opposite());
        let (left, right) = match facing {
            Facing::North | Facing::South => (Facing::East, Facing::West),
            _ => (Facing::North, Facing::South),
        };
        let side = signal(left).max(signal(right));
        let output = match mode {
            ComparatorMode::Compare if back >= side => back,
            ComparatorMode::Compare => 0,
            ComparatorMode::Subtract => back.saturating_sub(side),
        };
        cell.power = output;
        cell.strongly_powered = output > 0;
        cell
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(vm: &mut RedstoneVM, z: u32, from: u32, to: u32) {
        for x in from..to {
            vm.set_block(x, 0, z, BlockType::RedstoneWire);
        }
    }

    fn repeater(delay: u8) -> BlockType {
        BlockType::Repeater { delay_ticks: delay, facing: Facing::East, locked: false }
    }

    #[test]
    fn lever_powers_wire_with_decay() {
        let mut vm = RedstoneVM::new(32);
        vm.set_block(0, 0, 0, BlockType::Lever { on: true });
        line(&mut vm, 0, 1, 20);
        vm.step();
        assert_eq!(vm.cell(1, 0, 0).power, 14);
        assert_eq!(vm.cell(14, 0, 0).power, 1);
        assert_eq!(vm.cell(15, 0, 0).power, 0);
    }

    #[test]
    fn wire_powers_the_block_under_it() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(1, 0, 0, BlockType::Solid);
        vm.set_block(0, 1, 0, BlockType::Lever { on: true });
        vm.set_block(1, 1, 0, BlockType::RedstoneWire);
        vm.step();
        assert!(vm.cell(1, 0, 0).weakly_powered);
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
        assert_eq!(vm.get_port_output(5, 0, 0), 11);
        assert_eq!(vm.read_outputs(0), vec![(5, 0, 11)]);

        vm.set_port_input(0, 0, 0, 0);
        vm.step();
        assert_eq!(vm.get_port_output(5, 0, 0), 0);
    }

    #[test]
    fn bidirectional_port_reads_and_drives() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, BlockType::Port(PortMode::Bidirectional));
        line(&mut vm, 0, 1, 3);
        vm.set_port_input(0, 0, 0, 9);
        vm.step();
        assert_eq!(vm.cell(2, 0, 0).power, 7);
        assert_eq!(vm.get_port_output(0, 0, 0), 9);
    }

    #[test]
    fn torch_inverts_the_block_it_stands_on() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(1, 0, 0, BlockType::Solid);
        vm.set_block(1, 1, 0, BlockType::RedstoneTorch { lit: true });
        vm.step();
        vm.step();
        assert!(matches!(vm.cell(1, 1, 0).block, BlockType::RedstoneTorch { lit: true }));
        assert_eq!(vm.cell(1, 1, 0).power, 15);

        vm.set_block(0, 0, 0, BlockType::Lever { on: true });
        vm.step();
        assert!(matches!(vm.cell(1, 1, 0).block, BlockType::RedstoneTorch { lit: false }));
        assert_eq!(vm.cell(1, 1, 0).power, 0);
    }

    #[test]
    fn repeater_waits_its_delay() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, BlockType::Lever { on: true });
        vm.set_block(1, 0, 0, BlockType::RedstoneWire);
        vm.set_block(2, 0, 0, repeater(3));
        line(&mut vm, 0, 3, 6);
        let powered_at: Vec<bool> = (0..5)
            .map(|_| {
                vm.step();
                vm.cell(3, 0, 0).power > 0
            })
            .collect();
        // Input seen in step 1, latched at the end of step 3, on the wire in step 4.
        assert_eq!(powered_at, [false, false, false, true, true]);
        assert_eq!(vm.cell(3, 0, 0).power, 14);
    }

    #[test]
    fn repeater_turns_off_after_its_delay() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, BlockType::Lever { on: true });
        vm.set_block(1, 0, 0, repeater(1));
        vm.set_block(2, 0, 0, BlockType::RedstoneWire);
        vm.step();
        vm.step();
        assert_eq!(vm.cell(2, 0, 0).power, 14);
        vm.set_block(0, 0, 0, BlockType::Lever { on: false });
        vm.step(); // sees the lever off, switches at the end of this step
        vm.step();
        assert_eq!(vm.cell(2, 0, 0).power, 0);
    }

    #[test]
    fn comparator_compares_and_subtracts() {
        let run = |mode| {
            let mut vm = RedstoneVM::new(16);
            vm.set_block(0, 0, 1, BlockType::RedstoneBlock);
            vm.set_block(1, 0, 1, BlockType::Comparator { mode, facing: Facing::East });
            vm.set_block(1, 0, 0, BlockType::Port(PortMode::Input));
            vm.set_port_input(1, 0, 0, 6);
            vm.step();
            vm.cell(1, 0, 1).power
        };
        assert_eq!(run(ComparatorMode::Compare), 15);
        assert_eq!(run(ComparatorMode::Subtract), 9);
    }

    #[test]
    fn comparator_drives_wire() {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 0, BlockType::RedstoneBlock);
        vm.set_block(1, 0, 0, BlockType::Comparator { mode: ComparatorMode::Compare, facing: Facing::East });
        line(&mut vm, 0, 2, 4);
        vm.step();
        vm.step();
        assert_eq!(vm.cell(2, 0, 0).power, 14);
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
