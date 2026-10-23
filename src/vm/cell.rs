//! One cell of a chip's world: its block and that block's state.

use super::block::Push;
use super::{BlockType, Facing, PortMode};

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
    pub(super) watched: u32,
    /// Wire: the directions it powers this step (bits of [`Facing::ALL`]).
    pub(super) points: u8,
}

pub(super) const AIR: Cell = Cell::new(BlockType::Air);

impl Cell {
    pub const fn new(block: BlockType) -> Self {
        let power = match block {
            BlockType::RedstoneTorch { lit: true, .. }
            | BlockType::Lever { on: true, .. }
            | BlockType::Button { pressed: true, .. }
            | BlockType::RedstoneBlock => 15,
            BlockType::PressurePlate { power }
            | BlockType::TargetBlock { power }
            | BlockType::Source { power } => power,
            _ => 0,
        };
        Cell {
            block,
            power,
            strong: 0,
            weak: 0,
            timer: 0,
            port_in: 0,
            watched: 0,
            points: 0,
        }
    }

    pub(super) fn is_solid(&self) -> bool {
        matches!(
            self.block,
            BlockType::Solid
                | BlockType::RedstoneLamp
                | BlockType::Immovable
                | BlockType::Container { solid: true, .. }
                | BlockType::Dropper { .. }
        )
    }

    pub(super) fn is_air(&self) -> bool {
        self.block == BlockType::Air
    }

    pub(super) fn holds_items(&self) -> bool {
        matches!(
            self.block,
            BlockType::Container { .. } | BlockType::Hopper { .. } | BlockType::Dropper { .. }
        )
    }

    pub(super) fn push(&self) -> Push {
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

    pub(super) fn is_sticky(&self) -> bool {
        matches!(self.block, BlockType::Slime | BlockType::Honey)
    }

    pub(super) fn is_wire(&self) -> bool {
        self.block == BlockType::RedstoneWire
    }

    pub(super) fn is_output_port(&self) -> bool {
        matches!(
            self.block,
            BlockType::Port(PortMode::Output | PortMode::Bidirectional)
        )
    }

    /// Wire next to it turns toward it (`toward`: from the wire to it).
    pub(super) fn wire_connects(&self, toward: Facing) -> bool {
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
    pub(super) fn emits(&self, to: Facing) -> (u8, u8) {
        let p = self.power;
        match self.block {
            BlockType::RedstoneBlock
            | BlockType::TargetBlock { .. }
            | BlockType::Source { .. }
            | BlockType::RedstonePort { output: true } => (p, 0),
            BlockType::Lever { attach, .. } | BlockType::Button { attach, .. } => {
                (p, if to == attach { p } else { 0 })
            }
            BlockType::PressurePlate { .. } => (p, if to == Facing::Down { p } else { 0 }),
            BlockType::RedstoneTorch { attach, .. } => (
                if to == attach { 0 } else { p },
                if to == Facing::Up { p } else { 0 },
            ),
            BlockType::Repeater { facing, .. }
            | BlockType::Comparator { facing, .. }
            | BlockType::Observer { facing } => {
                if to == facing.opposite() {
                    (p, p)
                } else {
                    (0, 0)
                }
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
    pub(super) fn looks(&self) -> u32 {
        let shown = match self.block {
            BlockType::Solid | BlockType::Transparent | BlockType::Air | BlockType::Immovable => 0,
            _ => self.power as u32,
        };
        let c = self.block.code();
        ((c ^ c >> 32) as u32).wrapping_mul(0x9E37_79B9) ^ shown << 24
    }
}
