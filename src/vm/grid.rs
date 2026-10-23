//! The grid: cells by index, placing blocks, items and nested ALUs, and
//! the ports a chip talks to the world through.

use super::MAX_POWER;

use super::cell::AIR;
use super::{BlockType, Cell, CellMap, Facing, NestedAlu, PortMode, RedstoneVM};
use std::collections::HashMap;

/// The random seed every VM starts from (the golden ratio's fraction):
/// a circuit runs the same every time.
const RNG_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

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
            logic: Vec::new(),
            alu_changes: 0,
            wires: Vec::new(),
            solids: Vec::new(),
            readers: Vec::new(),
            movers: Vec::new(),
            components: Vec::new(),
            cables: Vec::new(),
            tick: 0,
            inventories: HashMap::new(),
            nested: CellMap::default(),
            items: Vec::new(),
            rng: RNG_SEED,
            quiet: false,
            buses: None,
            dirty: std::collections::BTreeSet::new(),
            unstable: std::collections::BTreeSet::new(),
            dirty_readers: std::collections::BTreeSet::new(),
        }
    }

    #[inline]
    pub(super) fn idx(&self, x: u32, y: u32, z: u32) -> u32 {
        x | z << self.shift | y << (2 * self.shift)
    }

    #[inline]
    pub(super) fn pos(&self, i: u32) -> (u32, u32, u32) {
        let mask = self.size - 1;
        (i & mask, i >> (2 * self.shift), (i >> self.shift) & mask)
    }

    #[inline]
    pub(super) fn layer_len(&self) -> u32 {
        1 << (2 * self.shift)
    }

    #[inline]
    pub(super) fn in_bounds(&self, x: i32, y: i32, z: i32) -> bool {
        let s = self.size as i32;
        (0..s).contains(&x) && (0..s).contains(&y) && (0..s).contains(&z)
    }

    /// The neighbour of cell `i` in direction `d`, if inside the world.
    #[inline]
    pub(super) fn at(&self, i: u32, d: Facing) -> Option<u32> {
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
        self.in_bounds(x, y, z)
            .then(|| self.idx(x as u32, y as u32, z as u32))
    }

    #[inline]
    pub(super) fn get(&self, i: u32) -> &Cell {
        match &self.layers[(i >> (2 * self.shift)) as usize] {
            Some(cells) => &cells[(i & (self.layer_len() - 1)) as usize],
            None => &AIR,
        }
    }

    /// The cell next to `i` in direction `d` (air outside the world).
    #[inline]
    pub(super) fn next(&self, i: u32, d: Facing) -> &Cell {
        self.at(i, d).map_or(&AIR, |n| self.get(n))
    }

    /// Mutable access to a cell that holds a block (its layer exists).
    #[inline]
    pub(super) fn get_mut(&mut self, i: u32) -> &mut Cell {
        let len = self.layer_len();
        let offset = (i & (len - 1)) as usize;
        let cells = self.layers[(i >> (2 * self.shift)) as usize]
            .get_or_insert_with(|| vec![AIR; len as usize].into_boxed_slice());
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
    pub(super) fn put(&mut self, i: u32, cell: Cell) {
        self.buses = None;
        let found = self.active.binary_search(&i);
        // Its place in the passes' lists.
        let air = cell.is_air();
        let kinds = [
            !air && cell.is_wire(),
            !air && cell.is_solid(),
            !air && matches!(
                cell.block,
                BlockType::RedstoneLamp
                    | BlockType::Port(PortMode::Output | PortMode::Bidirectional)
            ),
            !air && matches!(
                cell.block,
                BlockType::Hopper { .. } | BlockType::Dropper { .. }
            ),
            !air && matches!(
                cell.block,
                BlockType::RedstoneTorch { .. }
                    | BlockType::Repeater { .. }
                    | BlockType::Comparator { .. }
                    | BlockType::Observer { .. }
                    | BlockType::Piston { .. }
                    | BlockType::Dropper { .. }
            ),
            cell.block == BlockType::Cable,
        ];
        for (list, want) in [
            &mut self.wires,
            &mut self.solids,
            &mut self.readers,
            &mut self.movers,
            &mut self.components,
            &mut self.cables,
        ]
        .into_iter()
        .zip(kinds)
        {
            match (list.binary_search(&i), want) {
                (Ok(at), false) => {
                    list.remove(at);
                }
                (Err(at), true) => list.insert(at, i),
                _ => {}
            }
        }
        let in_logic = self.logic.binary_search(&i);
        let logic = !cell.is_air() && !matches!(cell.block, BlockType::Alu | BlockType::Bus);
        match (in_logic, logic) {
            (Ok(at), false) => {
                self.logic.remove(at);
            }
            (Err(at), true) => self.logic.insert(at, i),
            _ => {}
        }
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
        let ids: Vec<u16> = items
            .iter()
            .map(|(_, item, _)| self.item_index(item))
            .collect();
        let Some(inv) = self.inventories.get_mut(&i) else {
            return;
        };
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
        self.inventories
            .get(&i)
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, (_, n))| *n > 0)
            .map(|(slot, (id, n))| (slot as u8, self.items[*id as usize].clone(), *n))
            .collect()
    }

    pub(super) fn item_index(&mut self, item: &str) -> u16 {
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
            self.buses = None;
        }
    }

    pub fn set_port_input(&mut self, x: u32, y: u32, z: u32, power: u8) {
        if !self.in_bounds(x as i32, y as i32, z as i32) {
            return;
        }
        let i = self.idx(x, y, z);
        if matches!(
            self.get(i).block,
            BlockType::Port(PortMode::Input | PortMode::Bidirectional)
        ) && self.get(i).port_in != power.min(MAX_POWER)
        {
            self.get_mut(i).port_in = power.min(MAX_POWER);
            self.quiet = false;
            self.touch_around(i);
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
}
