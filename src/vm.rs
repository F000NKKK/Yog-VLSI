//! Rust redstone VM: accelerated redstone simulation for microchips.
//!
//! Each chip tier maps to a world size. The VM simulates redstone ticks
//! at configurable speed (up to 40 ticks/s for Netherite, 2× vanilla).
//!
//! Phase 1: redstone wire propagation, torches, repeaters, lamps, solid blocks, I/O ports.
//! Phase 2 (planned): comparators, observers, pistons, hoppers, chests.

use std::collections::VecDeque;

// ── Tier ──────────────────────────────────────────────────────────────────────

/// Microchip tier definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Tier {
    Wood,
    Stone,
    Gold,
    Iron,
    Diamond,
    Netherite,
}

impl Tier {
    pub const ALL: &[Tier] = &[
        Tier::Wood, Tier::Stone, Tier::Gold,
        Tier::Iron, Tier::Diamond, Tier::Netherite,
    ];

    pub fn tick_rate(self) -> u32 {
        match self {
            Tier::Wood      => 5,
            Tier::Stone     => 10,
            Tier::Gold      => 20,
            Tier::Iron      => 25,
            Tier::Diamond   => 30,
            Tier::Netherite => 40,
        }
    }

    pub fn world_size(self) -> u32 {
        match self {
            Tier::Wood      => 16,
            Tier::Stone     => 32,
            Tier::Gold      => 64,
            Tier::Iron      => 64,
            Tier::Diamond   => 128,
            Tier::Netherite => 256,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::Wood      => "Wood",
            Tier::Stone     => "Stone",
            Tier::Gold      => "Gold",
            Tier::Iron      => "Iron",
            Tier::Diamond   => "Diamond",
            Tier::Netherite => "Netherite",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Tier::Wood      => "wood",
            Tier::Stone     => "stone",
            Tier::Gold      => "gold",
            Tier::Iron      => "iron",
            Tier::Diamond   => "diamond",
            Tier::Netherite => "netherite",
        }
    }
}

// ── Block types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    Air,
    Solid,
    RedstoneWire,
    RedstoneTorch { lit: bool },
    RedstoneWallTorch { lit: bool, facing: Facing },
    Repeater { delay_ticks: u8, facing: Facing, locked: bool },
    RedstoneLamp,
    Lever { on: bool },
    StoneButton { pressed: bool, facing: Facing },
    Port(PortMode),
    RedstoneBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing {
    North, South, East, West,
}

impl Facing {
    pub fn offset(self) -> (i32, i32) {
        match self {
            Facing::North => ( 0, -1),
            Facing::South => ( 0,  1),
            Facing::East  => ( 1,  0),
            Facing::West  => (-1,  0),
        }
    }

    pub fn opposite(self) -> Facing {
        match self {
            Facing::North => Facing::South,
            Facing::South => Facing::North,
            Facing::East  => Facing::West,
            Facing::West  => Facing::East,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortMode {
    Input,
    Output,
    Bidirectional,
}

// ── Cell ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Cell {
    pub block: BlockType,
    pub power: u8,
    pub strongly_powered: bool,
    pub weakly_powered: bool,
    pub repeater_timer: u8,
    pub torch_burnout: u8,
}

impl Cell {
    pub fn new(block: BlockType) -> Self {
        Cell {
            block,
            power: 0,
            strongly_powered: false,
            weakly_powered: false,
            repeater_timer: 0,
            torch_burnout: 0,
        }
    }

    fn is_solid(&self) -> bool {
        matches!(self.block, BlockType::Solid | BlockType::RedstoneLamp | BlockType::RedstoneBlock)
    }
}

// ── VM ────────────────────────────────────────────────────────────────────────

pub struct RedstoneVM {
    pub tier: Tier,
    pub width: u32,
    pub height: u32,
    grid: Vec<Cell>,
    updates: VecDeque<(u32, u32, u32)>,
    pub tick: u64,
}

impl RedstoneVM {
    pub fn new(tier: Tier) -> Self {
        let size = tier.world_size();
        let cell_count = (size * size * size) as usize;
        RedstoneVM {
            tier,
            width: size,
            height: size,
            grid: vec![Cell::new(BlockType::Air); cell_count],
            updates: VecDeque::new(),
            tick: 0,
        }
    }

    /// Direct index computation (no self borrow).
    #[inline]
    fn idx_static(x: u32, y: u32, z: u32, w: u32) -> usize {
        (x + z * w + y * w * w) as usize
    }

    #[inline]
    fn in_bounds(&self, x: i32, y: i32, z: i32) -> bool {
        x >= 0 && (x as u32) < self.width
            && y >= 0 && (y as u32) < self.height
            && z >= 0 && (z as u32) < self.width
    }

    pub fn cell(&self, x: u32, y: u32, z: u32) -> &Cell {
        &self.grid[Self::idx_static(x, y, z, self.width)]
    }

    pub fn set_block(&mut self, x: u32, y: u32, z: u32, block: BlockType) {
        let i = Self::idx_static(x, y, z, self.width);
        self.grid[i] = Cell::new(block);
        self.schedule_update(x, y, z);
        for (dx, dz) in &[(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let nz = z as i32 + dz;
            if self.in_bounds(nx, y as i32, nz) {
                self.schedule_update(nx as u32, y, nz as u32);
            }
        }
    }

    fn schedule_update(&mut self, x: u32, y: u32, z: u32) {
        if !self.updates.iter().any(|&(ux, uy, uz)| ux == x && uy == y && uz == z) {
            self.updates.push_back((x, y, z));
        }
    }

    pub fn set_port_input(&mut self, x: u32, y: u32, z: u32, power: u8) {
        if !self.in_bounds(x as i32, y as i32, z as i32) { return; }
        let i = Self::idx_static(x, y, z, self.width);
        let cell = &mut self.grid[i];
        if matches!(cell.block, BlockType::Port(PortMode::Input | PortMode::Bidirectional)) {
            cell.power = power.min(15);
            cell.strongly_powered = power > 0;
        }
        self.schedule_update(x, y, z);
    }

    pub fn get_port_output(&self, x: u32, y: u32, z: u32) -> u8 {
        if !self.in_bounds(x as i32, y as i32, z as i32) { return 0; }
        let cell = self.cell(x, y, z);
        if matches!(cell.block, BlockType::Port(PortMode::Output | PortMode::Bidirectional)) {
            cell.power
        } else {
            0
        }
    }

    /// Advance the simulation by one tick.
    pub fn step(&mut self) {
        self.tick += 1;
        self.reset_wire_power();

        // Drain scheduled updates before the main pass.
        let updates: Vec<_> = self.updates.drain(..).collect();
        for &(x, y, z) in &updates {
            self.process_update(x, y, z);
        }

        self.propagate_power();
        self.update_solid_powering();
        self.update_timers();
    }

    fn reset_wire_power(&mut self) {
        for cell in &mut self.grid {
            if matches!(cell.block, BlockType::RedstoneWire) {
                cell.power = 0;
            }
            cell.weakly_powered = false;
        }
    }

    fn process_update(&mut self, x: u32, y: u32, z: u32) {
        let i = Self::idx_static(x, y, z, self.width);
        let block = self.grid[i].block;
        match block {
            BlockType::RedstoneTorch { .. } | BlockType::RedstoneWallTorch { .. } => {
                self.update_torch_at(x, y, z);
            }
            BlockType::Repeater { .. } => {
                self.update_repeater_at(x, y, z);
            }
            _ => {}
        }
    }

    // ── Power propagation (BFS through wires) ──────────────────────────────

    fn propagate_power(&mut self) {
        let w = self.width;
        let h = self.height;
        let mut queue: VecDeque<(u32, u32, u32, u8)> = VecDeque::new();

        // Collect sources.
        for y in 0..h {
            for z in 0..w {
                for x in 0..w {
                    let i = Self::idx_static(x, y, z, w);
                    let src = self.source_power_at(i);
                    if src > 0 {
                        queue.push_back((x, y, z, src));
                    }
                }
            }
        }

        while let Some((x, y, z, power)) = queue.pop_front() {
            if power <= 1 { continue; }
            let next = power - 1;

            for (dx, dz) in &[(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as i32 + dx;
                let nz = z as i32 + dz;
                if nx < 0 || nx as u32 >= w || nz < 0 || nz as u32 >= w { continue; }
                let ni = Self::idx_static(nx as u32, y, nz as u32, w);

                match self.grid[ni].block {
                    BlockType::RedstoneWire => {
                        if self.grid[ni].power < next {
                            self.grid[ni].power = next;
                            queue.push_back((nx as u32, y, nz as u32, next));
                        }
                    }
                    BlockType::Solid | BlockType::RedstoneLamp | BlockType::RedstoneBlock => {
                        self.grid[ni].weakly_powered = true;
                    }
                    _ => {}
                }
            }

            // Power solid block below the wire.
            if y > 0 {
                let bi = Self::idx_static(x, y - 1, z, w);
                if self.grid[bi].is_solid() {
                    self.grid[bi].weakly_powered = true;
                    // Neighbors of the solid block below also get weak power.
                    for (dx, dz) in &[(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                        let nx = x as i32 + dx;
                        let nz = z as i32 + dz;
                        if nx >= 0 && (nx as u32) < w && nz >= 0 && (nz as u32) < w {
                            let bni = Self::idx_static(nx as u32, y - 1, nz as u32, w);
                            if self.grid[bni].is_solid() {
                                self.grid[bni].weakly_powered = true;
                            }
                        }
                    }
                }
            }
        }
    }

    fn source_power_at(&self, i: usize) -> u8 {
        let cell = &self.grid[i];
        match cell.block {
            BlockType::RedstoneTorch { lit: true } => 15,
            BlockType::RedstoneWallTorch { lit: true, .. } => 15,
            BlockType::Repeater { .. } if cell.power > 0 => cell.power,
            BlockType::Lever { on: true } => 15,
            BlockType::StoneButton { pressed: true, .. } => 15,
            BlockType::RedstoneBlock => 15,
            BlockType::Port(PortMode::Input | PortMode::Bidirectional) if cell.strongly_powered => cell.power,
            _ => 0,
        }
    }

    // ── Solid block powering ───────────────────────────────────────────────

    fn update_solid_powering(&mut self) {
        let w = self.width;
        let h = self.height;

        // First pass: collect which solid blocks get strong power from neighbors.
        let mut strong_list: Vec<(usize, bool)> = Vec::new();

        for y in 0..h {
            for z in 0..w {
                for x in 0..w {
                    let i = Self::idx_static(x, y, z, w);
                    if !self.grid[i].is_solid() { continue; }

                    let mut strong = false;
                    for (dx, dz) in &[(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                        let nx = x as i32 + dx;
                        let nz = z as i32 + dz;
                        if nx < 0 || nx as u32 >= w || nz < 0 || nz as u32 >= w { continue; }
                        let ni = Self::idx_static(nx as u32, y, nz as u32, w);
                        strong |= matches!(self.grid[ni].block,
                            BlockType::Repeater { .. }
                            | BlockType::RedstoneTorch { lit: true }
                            | BlockType::RedstoneWallTorch { lit: true, .. }
                        ) && self.grid[ni].power > 0;
                    }
                    strong_list.push((i, strong));
                }
            }
        }

        for (i, strong) in strong_list {
            self.grid[i].strongly_powered = strong;
        }
    }

    // ── Timers (repeater delay, torch burnout) ─────────────────────────────

    fn update_timers(&mut self) {
        let w = self.width;
        let h = self.height;
        let mut reschedule: Vec<(u32, u32, u32)> = Vec::new();

        for y in 0..h {
            for z in 0..w {
                for x in 0..w {
                    let i = Self::idx_static(x, y, z, w);
                    let cell = &mut self.grid[i];

                    if matches!(cell.block, BlockType::Repeater { .. }) && cell.repeater_timer > 0 {
                        cell.repeater_timer -= 1;
                        if cell.repeater_timer == 0 {
                            reschedule.push((x, y, z));
                        }
                    }

                    if cell.torch_burnout > 0 {
                        cell.torch_burnout -= 1;
                        if cell.torch_burnout == 0 {
                            reschedule.push((x, y, z));
                        }
                    }
                }
            }
        }

        for (x, y, z) in reschedule {
            self.process_update(x, y, z);
        }
    }

    // ── Torch logic ────────────────────────────────────────────────────────

    fn update_torch_at(&mut self, x: u32, y: u32, z: u32) {
        let w = self.width;
        let i = Self::idx_static(x, y, z, w);

        if self.grid[i].torch_burnout > 0 { return; }

        let attached_powered = {
            let cell = &self.grid[i];
            match cell.block {
                BlockType::RedstoneTorch { .. } => {
                    if y > 0 {
                        let bi = Self::idx_static(x, y - 1, z, w);
                        let below = &self.grid[bi];
                        below.power > 0 || below.strongly_powered || below.weakly_powered
                    } else {
                        false
                    }
                }
                BlockType::RedstoneWallTorch { facing, .. } => {
                    let (dx, dz) = facing.opposite().offset();
                    let ax = x as i32 + dx;
                    let az = z as i32 + dz;
                    if self.in_bounds(ax, y as i32, az) {
                        let ai = Self::idx_static(ax as u32, y, az as u32, w);
                        let attached = &self.grid[ai];
                        attached.power > 0 || attached.strongly_powered || attached.weakly_powered
                    } else {
                        false
                    }
                }
                _ => return,
            }
        };

        let new_lit = !attached_powered;
        let cell = &mut self.grid[i];

        match &mut cell.block {
            BlockType::RedstoneTorch { lit } => *lit = new_lit,
            BlockType::RedstoneWallTorch { lit, .. } => *lit = new_lit,
            _ => {}
        }

        if new_lit {
            cell.power = 15;
            cell.strongly_powered = true;
        } else {
            cell.power = 0;
            cell.strongly_powered = false;
        }

        // Schedule neighbor updates.
        for (dx, dz) in &[(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let nz = z as i32 + dz;
            if self.in_bounds(nx, y as i32, nz) {
                self.schedule_update(nx as u32, y, nz as u32);
            }
        }
        if y > 0 { self.schedule_update(x, y - 1, z); }
        if (y as u32) + 1 < self.height { self.schedule_update(x, y + 1, z); }
    }

    // ── Repeater logic ─────────────────────────────────────────────────────

    fn update_repeater_at(&mut self, x: u32, y: u32, z: u32) {
        let w = self.width;
        let i = Self::idx_static(x, y, z, w);

        let (facing, delay) = match self.grid[i].block {
            BlockType::Repeater { facing, delay_ticks, .. } => (facing, delay_ticks),
            _ => return,
        };

        let (ix, iz) = facing.opposite().offset();
        let ix = x as i32 + ix;
        let iz = z as i32 + iz;
        let input_powered = if self.in_bounds(ix, y as i32, iz) {
            let ii = Self::idx_static(ix as u32, y, iz as u32, w);
            self.grid[ii].power > 0 || self.grid[ii].strongly_powered
        } else {
            false
        };

        let cell = &mut self.grid[i];

        if input_powered && cell.repeater_timer == 0 {
            cell.repeater_timer = delay;
        }

        if cell.repeater_timer == 0 && input_powered {
            cell.power = 15;
            cell.strongly_powered = true;

            let (ox, oz) = facing.offset();
            let ox = x as i32 + ox;
            let oz = z as i32 + oz;
            if self.in_bounds(ox, y as i32, oz) {
                let oi = Self::idx_static(ox as u32, y, oz as u32, w);
                if self.grid[oi].is_solid() {
                    self.grid[oi].strongly_powered = true;
                }
            }
        } else if !input_powered {
            cell.power = 0;
            cell.strongly_powered = false;
            cell.repeater_timer = 0;
        }
    }

    // ── Port I/O ───────────────────────────────────────────────────────────

    pub fn read_outputs(&self, y: u32) -> Vec<(u32, u32, u8)> {
        let w = self.width;
        let mut result = Vec::new();
        for z in 0..w {
            for x in 0..w {
                let cell = &self.grid[Self::idx_static(x, y, z, w)];
                if matches!(cell.block, BlockType::Port(PortMode::Output | PortMode::Bidirectional)) {
                    result.push((x, z, cell.power));
                }
            }
        }
        result
    }

    pub fn write_inputs(&mut self, y: u32, inputs: &[(u32, u32, u8)]) {
        for &(x, z, power) in inputs {
            self.set_port_input(x, y, z, power);
        }
    }
}
