//! Rust redstone VM: accelerated redstone simulation for microchips.
//!
//! Each chip tier maps to a world size. The VM simulates redstone ticks
//! at configurable speed (up to 40 ticks/s for Netherite, 2× vanilla).
//!
//! Supported blocks: redstone wire, torches, repeaters, comparators, levers,
//! buttons, pressure plates, pistons (sticky + normal), observers, note blocks,
//! lamps, redstone blocks, target blocks, hoppers, droppers, dispensers,
//! chests, trapped chests, shulker boxes, slime blocks, honey blocks,
//! solid/conductor blocks, glass, and I/O ports.

use std::collections::VecDeque;

// ── Tier ──────────────────────────────────────────────────────────────────────

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
    ShulkerBox { color: Option<ShulkerColor> },
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

    // I/O ports (special VLSI blocks)
    Port(PortMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparatorMode {
    Compare,
    Subtract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShulkerColor {
    White, Orange, Magenta, LightBlue, Yellow, Lime,
    Pink, Gray, LightGray, Cyan, Purple, Blue, Brown, Green, Red, Black,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoorHalf {
    Upper,
    Lower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing {
    North, South, East, West, Up, Down,
}

impl Facing {
    pub fn horizontal_offset(self) -> (i32, i32) {
        match self {
            Facing::North => ( 0, -1),
            Facing::South => ( 0,  1),
            Facing::East  => ( 1,  0),
            Facing::West  => (-1,  0),
            _ => (0, 0),
        }
    }

    pub fn opposite(self) -> Facing {
        match self {
            Facing::North => Facing::South,
            Facing::South => Facing::North,
            Facing::East  => Facing::West,
            Facing::West  => Facing::East,
            Facing::Up    => Facing::Down,
            Facing::Down  => Facing::Up,
        }
    }

    pub fn from_minecraft(s: &str) -> Facing {
        match s {
            "north" => Facing::North,
            "south" => Facing::South,
            "east"  => Facing::East,
            "west"  => Facing::West,
            "up"    => Facing::Up,
            "down"  => Facing::Down,
            _       => Facing::North,
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
        matches!(self.block,
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
            | BlockType::ShulkerBox { .. }
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

    /// Blocks that can receive a redstone signal (solid + specific non-solids).
    fn is_redstone_conductor(&self) -> bool {
        self.is_solid() || matches!(self.block,
            BlockType::Hopper { .. }
            | BlockType::Dropper { .. }
            | BlockType::Dispenser { .. }
        )
    }

    /// Whether this block type outputs power by itself.
    fn is_power_source(&self) -> bool {
        matches!(self.block,
            BlockType::RedstoneTorch { lit: true }
            | BlockType::RedstoneWallTorch { lit: true, .. }
            | BlockType::RedstoneBlock
            | BlockType::Lever { on: true }
            | BlockType::Observer { powered: true, .. }
            | BlockType::TargetBlock { .. }
            | BlockType::DetectorRail { powered: true }
        )
    }

    /// Power level a source outputs.
    fn source_power(&self) -> u8 {
        match self.block {
            BlockType::RedstoneTorch { lit: true } => 15,
            BlockType::RedstoneWallTorch { lit: true, .. } => 15,
            BlockType::Repeater { .. } if self.power > 0 => self.power,
            BlockType::Lever { on: true } => 15,
            BlockType::StoneButton { pressed: true, .. } => 15,
            BlockType::WoodButton { pressed: true, .. } => 15,
            BlockType::RedstoneBlock => 15,
            BlockType::TargetBlock { power } => power,
            BlockType::Observer { powered: true, .. } => 15,
            BlockType::LightWeightedPressurePlate { power } => power,
            BlockType::HeavyWeightedPressurePlate { power } => power,
            BlockType::DetectorRail { powered: true } => 15,
            BlockType::TrappedChest => {
                // Trapped chest outputs power based on viewers (simplified: 1 per viewer)
                1
            }
            BlockType::Port(PortMode::Input | PortMode::Bidirectional) if self.strongly_powered => self.power,
            _ => 0,
        }
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
        if y > 0 { self.schedule_update(x, y - 1, z); }
        if (y as u32) + 1 < self.height { self.schedule_update(x, y + 1, z); }
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

    pub fn step(&mut self) {
        self.tick += 1;
        self.reset_wire_power();

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
            BlockType::Observer { .. } => {
                // Observer pulse: 2 tick output then off
                let cell = &mut self.grid[i];
                if let BlockType::Observer { powered, .. } = &mut cell.block {
                    *powered = false;
                    cell.power = 0;
                }
            }
            _ => {}
        }
    }

    fn propagate_power(&mut self) {
        let w = self.width;
        let h = self.height;
        let mut queue: VecDeque<(u32, u32, u32, u8)> = VecDeque::new();

        for y in 0..h {
            for z in 0..w {
                for x in 0..w {
                    let i = Self::idx_static(x, y, z, w);
                    let src = self.grid[i].source_power();
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
                    _ if self.grid[ni].is_solid() => {
                        self.grid[ni].weakly_powered = true;
                        // Power solid neighbors of this solid block
                        for (ddx, ddz) in &[(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                            let nnx = nx + ddx;
                            let nnz = nz + ddz;
                            if nnx >= 0 && (nnx as u32) < w && nnz >= 0 && (nnz as u32) < w {
                                let nni = Self::idx_static(nnx as u32, y, nnz as u32, w);
                                if self.grid[nni].is_solid() {
                                    self.grid[nni].weakly_powered = true;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Power block below wire
            if y > 0 {
                let bi = Self::idx_static(x, y - 1, z, w);
                if self.grid[bi].is_solid() {
                    self.grid[bi].weakly_powered = true;
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

    fn update_solid_powering(&mut self) {
        let w = self.width;
        let h = self.height;
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
                            | BlockType::Comparator { .. }
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
                    } else { false }
                }
                BlockType::RedstoneWallTorch { facing, .. } => {
                    let (dx, dz) = facing.opposite().horizontal_offset();
                    let ax = x as i32 + dx;
                    let az = z as i32 + dz;
                    if self.in_bounds(ax, y as i32, az) {
                        let ai = Self::idx_static(ax as u32, y, az as u32, w);
                        let attached = &self.grid[ai];
                        attached.power > 0 || attached.strongly_powered || attached.weakly_powered
                    } else { false }
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

    fn update_repeater_at(&mut self, x: u32, y: u32, z: u32) {
        let w = self.width;
        let i = Self::idx_static(x, y, z, w);

        let (facing, delay) = match self.grid[i].block {
            BlockType::Repeater { facing, delay_ticks, .. } => (facing, delay_ticks),
            _ => return,
        };

        let (ix, iz) = facing.opposite().horizontal_offset();
        let ix = x as i32 + ix;
        let iz = z as i32 + iz;
        let input_powered = if self.in_bounds(ix, y as i32, iz) {
            let ii = Self::idx_static(ix as u32, y, iz as u32, w);
            self.grid[ii].power > 0 || self.grid[ii].strongly_powered
        } else { false };

        let cell = &mut self.grid[i];

        if input_powered && cell.repeater_timer == 0 {
            cell.repeater_timer = delay;
        }

        if cell.repeater_timer == 0 && input_powered {
            cell.power = 15;
            cell.strongly_powered = true;

            let (ox, oz) = facing.horizontal_offset();
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
