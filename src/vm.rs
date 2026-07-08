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

    /// Ticks per second this tier simulates.
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

    /// World size (width × height) for this tier.
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

    /// Display name.
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

    /// Lowercase identifier for item IDs and NBT.
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

/// The block type at a given cell in the virtual world.
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

/// One cell in the virtual redstone grid.
#[derive(Debug, Clone)]
pub struct Cell {
    pub block: BlockType,
    /// Redstone power level (0-15) from wire propagation.
    pub power: u8,
    /// Whether this cell is strongly powered (by repeater/torch output).
    pub strongly_powered: bool,
    /// Whether this cell is weakly powered (adjacent to powered wire).
    pub weakly_powered: bool,
    /// Repeater countdown: ticks remaining until output changes.
    pub repeater_timer: u8,
    /// Torch burnout counter (0 = normal, >0 = counting down).
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

    pub fn is_solid(&self) -> bool {
        matches!(self.block, BlockType::Solid | BlockType::RedstoneLamp | BlockType::RedstoneBlock)
    }

    pub fn is_air(&self) -> bool {
        matches!(self.block, BlockType::Air)
    }
}

// ── VM ────────────────────────────────────────────────────────────────────────

/// The redstone simulation virtual machine.
pub struct RedstoneVM {
    pub tier: Tier,
    pub width: u32,
    pub height: u32,
    grid: Vec<Cell>,
    /// Pending block updates (positions that need re-evaluation).
    updates: VecDeque<(u32, u32, u32)>,
    /// Current tick (increments each sim step).
    pub tick: u64,
}

impl RedstoneVM {
    /// Create a new empty VM for the given tier.
    pub fn new(tier: Tier) -> Self {
        let size = tier.world_size();
        let cell_count = (size * size * size) as usize;
        let grid = vec![Cell::new(BlockType::Air); cell_count];
        RedstoneVM {
            tier,
            width: size,
            height: size,
            grid,
            updates: VecDeque::new(),
            tick: 0,
        }
    }

    /// Linear index for (x, y, z).
    #[inline]
    fn idx(&self, x: u32, y: u32, z: u32) -> usize {
        (x + z * self.width + y * self.width * self.width) as usize
    }

    /// Check bounds.
    #[inline]
    fn in_bounds(&self, x: i32, y: i32, z: i32) -> bool {
        x >= 0 && (x as u32) < self.width
            && y >= 0 && (y as u32) < self.height
            && z >= 0 && (z as u32) < self.width
    }

    /// Get a cell reference.
    pub fn cell(&self, x: u32, y: u32, z: u32) -> &Cell {
        &self.grid[self.idx(x, y, z)]
    }

    /// Get a mutable cell reference.
    pub fn cell_mut(&mut self, x: u32, y: u32, z: u32) -> &mut Cell {
        let i = self.idx(x, y, z);
        &mut self.grid[i]
    }

    /// Place a block in the grid.
    pub fn set_block(&mut self, x: u32, y: u32, z: u32, block: BlockType) {
        let i = self.idx(x, y, z);
        self.grid[i] = Cell::new(block);
        self.schedule_update(x, y, z);
        // Notify neighbors
        for (dx, dz) in &[(1,0),(-1,0),(0,1),(0,-1)] {
            let nx = x as i32 + dx;
            let nz = z as i32 + dz;
            if self.in_bounds(nx, y as i32, nz) {
                self.schedule_update(nx as u32, y, nz as u32);
            }
        }
    }

    /// Schedule a block update.
    fn schedule_update(&mut self, x: u32, y: u32, z: u32) {
        if !self.updates.iter().any(|(ux, uy, uz)| *ux == x && *uy == y && *uz == z) {
            self.updates.push_back((x, y, z));
        }
    }

    /// Set external input signal on a port.
    pub fn set_port_input(&mut self, x: u32, y: u32, z: u32, power: u8) {
        if !self.in_bounds(x as i32, y as i32, z as i32) { return; }
        let cell = self.cell_mut(x, y, z);
        if matches!(cell.block, BlockType::Port(PortMode::Input | PortMode::Bidirectional)) {
            cell.power = power.min(15);
            cell.strongly_powered = power > 0;
        }
        self.schedule_update(x, y, z);
    }

    /// Read output signal from a port.
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

        // Phase 1: Reset wire power, process scheduled updates
        self.reset_wire_power();
        self.process_updates();

        // Phase 2: Propagate power from sources through wires (BFS)
        self.propagate_power();

        // Phase 3: Update weak/strong powering of solid blocks
        self.update_solid_powering();

        // Phase 4: Update output devices (lamps, output ports)
        self.update_outputs();

        // Phase 5: Update repeater timers and torch burnout
        self.update_timers();
    }

    /// Reset all wire cells to unpowered.
    fn reset_wire_power(&mut self) {
        for cell in &mut self.grid {
            if matches!(cell.block, BlockType::RedstoneWire) {
                cell.power = 0;
            }
            cell.weakly_powered = false;
        }
    }

    /// Process scheduled block updates.
    fn process_updates(&mut self) {
        while let Some((x, y, z)) = self.updates.pop_front() {
            let cell = self.grid[self.idx(x, y, z)].clone();
            match cell.block {
                BlockType::RedstoneTorch { .. } | BlockType::RedstoneWallTorch { .. } => {
                    self.update_torch(x, y, z);
                }
                BlockType::Repeater { .. } => {
                    self.update_repeater(x, y, z);
                }
                _ => {}
            }
        }
    }

    /// Propagate power from all sources through redstone wire via BFS.
    fn propagate_power(&mut self) {
        let w = self.width;
        let h = self.height;

        // Collect initial power sources
        let mut queue: VecDeque<(u32, u32, u32, u8)> = VecDeque::new();
        for y in 0..h {
            for z in 0..w {
                for x in 0..w {
                    let cell = &self.grid[self.idx(x, y, z)];
                    let src_power = self.source_power(cell);
                    if src_power > 0 {
                        queue.push_back((x, y, z, src_power));
                    }
                }
            }
        }

        // BFS through wires
        while let Some((x, y, z, power)) = queue.pop_front() {
            if power == 0 { continue; }
            let next_power = power.saturating_sub(1);
            if next_power == 0 { continue; }

            for (dx, dz) in &[(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as i32 + dx;
                let nz = z as i32 + dz;
                if !self.in_bounds(nx, y as i32, nz) { continue; }
                let nx = nx as u32;
                let nz = nz as u32;

                let neighbor = &self.grid[self.idx(nx, y, nz)];
                match neighbor.block {
                    BlockType::RedstoneWire => {
                        if self.grid[self.idx(nx, y, nz)].power < next_power {
                            self.grid[self.idx(nx, y, nz)].power = next_power;
                            queue.push_back((nx, y, nz, next_power));
                        }
                    }
                    BlockType::Solid | BlockType::RedstoneLamp | BlockType::RedstoneBlock => {
                        // Wire can weakly power an adjacent solid block
                        self.grid[self.idx(nx, y, nz)].weakly_powered = true;
                    }
                    _ => {}
                }

                // Also check the block below (wire on top of solid)
                if y > 0 {
                    let below = &mut self.grid[self.idx(nx, y - 1, nz)];
                    if matches!(below.block, BlockType::Solid | BlockType::RedstoneLamp | BlockType::RedstoneBlock) {
                        // Wire on top of a solid block can power blocks adjacent to that solid
                    }
                }
            }

            // Power blocks below (wire rests on solid blocks and powers them)
            if y > 0 {
                let below = self.grid[self.idx(x, y - 1, z)].clone();
                if below.is_solid() {
                    // The solid block below is weakly powered
                    // Its neighbors also get weakly powered
                    for (dx, dz) in &[(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let nx = x as i32 + dx;
                        let nz = z as i32 + dz;
                        if !self.in_bounds(nx, (y - 1) as i32, nz) { continue; }
                        let nx = nx as u32;
                        let nz = nz as u32;
                        let neighbor_below = &mut self.grid[self.idx(nx, y - 1, nz)];
                        if neighbor_below.is_solid() {
                            neighbor_below.weakly_powered = true;
                        }
                    }
                }
            }
        }
    }

    /// Determine the output power of a cell acting as a power source.
    fn source_power(&self, cell: &Cell) -> u8 {
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

    /// Update strong/weak powering of solid blocks based on adjacent components.
    fn update_solid_powering(&mut self) {
        // For now, strongly powered is set directly by torches/repeaters.
        // Weakly powered is handled during propagation.
        // This method re-evaluates solid block powering state.
        let w = self.width;
        let h = self.height;

        for y in 0..h {
            for z in 0..w {
                for x in 0..w {
                    let cell = &self.grid[self.idx(x, y, z)];
                    if !cell.is_solid() { continue; }

                    // A solid block is strongly powered if a repeater or torch faces into it
                    let mut strong = false;
                    for (dx, dz) in &[(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let nx = x as i32 + dx;
                        let nz = z as i32 + dz;
                        if !self.in_bounds(nx, y as i32, nz) { continue; }
                        let neighbor = &self.grid[self.idx(nx as u32, y, nz as u32)];
                        strong |= matches!(neighbor.block,
                            BlockType::Repeater { .. } | BlockType::RedstoneTorch { lit: true }
                            if neighbor.power > 0
                        );
                    }
                    self.grid[self.idx(x, y, z)].strongly_powered = strong;
                }
            }
        }
    }

    /// Update output devices: lamps glow when powered.
    fn update_outputs(&mut self) {
        for cell in &mut self.grid {
            if matches!(cell.block, BlockType::RedstoneLamp) {
                // Lamp is on if powered or adjacent to powered block
                // (already computed in power fields)
            }
            if matches!(cell.block, BlockType::Port(PortMode::Output | PortMode::Bidirectional)) {
                // Output port gets its power from the wire grid
            }
        }
    }

    /// Process repeater delays and torch burnout timers.
    fn update_timers(&mut self) {
        let w = self.width;
        let h = self.height;

        for y in 0..h {
            for z in 0..w {
                for x in 0..w {
                    let idx = self.idx(x, y, z);
                    let cell = &mut self.grid[idx];

                    // Repeater delay countdown
                    if matches!(cell.block, BlockType::Repeater { .. }) && cell.repeater_timer > 0 {
                        cell.repeater_timer -= 1;
                        if cell.repeater_timer == 0 {
                            // Output is now active
                            self.schedule_update(x, y, z);
                        }
                    }

                    // Torch burnout countdown
                    if cell.torch_burnout > 0 {
                        cell.torch_burnout -= 1;
                        if cell.torch_burnout == 0 {
                            // Torch can be relit
                            self.schedule_update(x, y, z);
                        }
                    }
                }
            }
        }
    }

    /// Update a redstone torch at the given position.
    fn update_torch(&mut self, x: u32, y: u32, z: u32) {
        let idx = self.idx(x, y, z);
        let cell = &self.grid[idx];
        if cell.torch_burnout > 0 { return; }

        // A torch is powered if the block it's attached to is powered
        let attached_powered = match cell.block {
            BlockType::RedstoneTorch { .. } => {
                // Torch on floor — check block below
                if y > 0 {
                    let below = &self.grid[self.idx(x, y - 1, z)];
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
                    let attached = &self.grid[self.idx(ax as u32, y, az as u32)];
                    attached.power > 0 || attached.strongly_powered || attached.weakly_powered
                } else {
                    false
                }
            }
            _ => return,
        };

        let lit = !attached_powered;
        let cell = &mut self.grid[idx];
        match &mut cell.block {
            BlockType::RedstoneTorch { ref mut lit: ref mut l } => *l = lit,
            BlockType::RedstoneWallTorch { ref mut lit: ref mut l, .. } => *l = lit,
            _ => {}
        }

        if lit {
            // Power adjacent blocks
            cell.power = 15;
            cell.strongly_powered = true;
        } else {
            cell.power = 0;
            cell.strongly_powered = false;
        }

        // Notify neighbors
        for (dx, dz) in &[(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let nz = z as i32 + dz;
            if self.in_bounds(nx, y as i32, nz) {
                self.schedule_update(nx as u32, y, nz as u32);
            }
        }
        if y > 0 { self.schedule_update(x, y - 1, z); }
        if (y as u32) < self.height - 1 { self.schedule_update(x, y + 1, z); }
    }

    /// Update a redstone repeater at the given position.
    fn update_repeater(&mut self, x: u32, y: u32, z: u32) {
        let idx = self.idx(x, y, z);
        let cell = &self.grid[idx];

        let facing = match cell.block {
            BlockType::Repeater { facing, .. } => facing,
            _ => return,
        };

        // Check input side (opposite of facing)
        let (ix, iz) = facing.opposite().offset();
        let ix = x as i32 + ix;
        let iz = z as i32 + iz;
        let input_powered = if self.in_bounds(ix, y as i32, iz) {
            let input = &self.grid[self.idx(ix as u32, y, iz as u32)];
            input.power > 0 || input.strongly_powered
        } else {
            false
        };

        let cell = &mut self.grid[idx];
        let delay = match cell.block {
            BlockType::Repeater { delay_ticks, .. } => delay_ticks,
            _ => return,
        };

        if input_powered && cell.repeater_timer == 0 {
            cell.repeater_timer = delay;
        }

        if cell.repeater_timer == 0 && input_powered {
            // Output is active — power the block in front
            cell.power = 15;
            cell.strongly_powered = true;
            let (ox, oz) = facing.offset();
            let ox = x as i32 + ox;
            let oz = z as i32 + oz;
            if self.in_bounds(ox, y as i32, oz) {
                let out = &mut self.grid[self.idx(ox as u32, y, oz as u32)];
                if out.is_solid() {
                    out.strongly_powered = true;
                }
            }
        } else if !input_powered {
            cell.power = 0;
            cell.strongly_powered = false;
            cell.repeater_timer = 0;
        }
    }

    /// Get all output port values (for ALU external interfacing).
    pub fn read_outputs(&self, y: u32) -> Vec<(u32, u32, u8)> {
        let w = self.width;
        let mut result = Vec::new();
        for z in 0..w {
            for x in 0..w {
                let cell = &self.grid[self.idx(x, y, z)];
                if matches!(cell.block, BlockType::Port(PortMode::Output | PortMode::Bidirectional)) {
                    result.push((x, z, cell.power));
                }
            }
        }
        result
    }

    /// Write input port values (from ALU external signals).
    pub fn write_inputs(&mut self, y: u32, inputs: &[(u32, u32, u8)]) {
        for &(x, z, power) in inputs {
            self.set_port_input(x, y, z, power);
        }
    }
}
