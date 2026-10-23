//! The blocks a chip's world is built of, and which way they face.

use yog_api::Schema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockType {
    Air,

    /// Any full block: conducts power.
    Solid,
    /// Doesn't conduct (glass, leaves, slabs, …).
    Transparent,

    RedstoneWire,
    /// `attach`: the direction of the block it hangs on (`Down` standing).
    RedstoneTorch {
        lit: bool,
        attach: Facing,
    },
    Repeater {
        delay_ticks: u8,
        facing: Facing,
    },
    Comparator {
        mode: ComparatorMode,
        facing: Facing,
    },
    RedstoneLamp,
    RedstoneBlock,
    /// `attach`: the direction of the block it's mounted on.
    Lever {
        on: bool,
        attach: Facing,
    },
    Button {
        pressed: bool,
        attach: Facing,
    },
    /// Any pressure plate, at the power it gives.
    PressurePlate {
        power: u8,
    },
    Observer {
        facing: Facing,
    },
    TargetBlock {
        power: u8,
    },
    /// Detector rail, trapped chest, …: a constant source.
    Source {
        power: u8,
    },

    /// Movable, doesn't conduct, drags what touches it along.
    Slime,
    Honey,
    /// A full block pistons can't move (obsidian, bedrock, …).
    Immovable,
    Piston {
        facing: Facing,
        sticky: bool,
        extended: bool,
    },
    PistonHead {
        facing: Facing,
        sticky: bool,
    },
    /// Holds `slots` items (in [`RedstoneVM`]'s inventories); `solid`
    /// conducts power; `partner`: the other half of a double chest.
    Container {
        slots: u8,
        solid: bool,
        partner: Option<Facing>,
    },
    /// `facing`: its spout.
    Hopper {
        facing: Facing,
    },
    /// A dropper or dispenser; `triggered` while powered.
    Dropper {
        facing: Facing,
        triggered: bool,
    },

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
    RedstonePort {
        output: bool,
    },
    /// A digital cable: a run of them is a bus, its channels driven and read
    /// by the ALUs on it (their boards' bus pins).
    Bus,
}

impl BlockType {
    /// A number for the block: the same for equal blocks, different for
    /// different ones — what a fingerprint takes (cheaper than hashing it).
    pub(super) fn code(self) -> u64 {
        let f = |d: Facing| d.index() as u64;
        let b = |v: bool| v as u64;
        match self {
            Self::Air => 0,
            Self::Solid => 1,
            Self::Transparent => 2,
            Self::RedstoneWire => 3,
            Self::RedstoneTorch { lit, attach } => 4 | b(lit) << 8 | f(attach) << 9,
            Self::Repeater {
                delay_ticks,
                facing,
            } => 5 | u64::from(delay_ticks) << 8 | f(facing) << 16,
            Self::Comparator { mode, facing } => 6 | (mode as u64) << 8 | f(facing) << 9,
            Self::RedstoneLamp => 7,
            Self::RedstoneBlock => 8,
            Self::Lever { on, attach } => 9 | b(on) << 8 | f(attach) << 9,
            Self::Button { pressed, attach } => 10 | b(pressed) << 8 | f(attach) << 9,
            Self::PressurePlate { power } => 11 | u64::from(power) << 8,
            Self::Observer { facing } => 12 | f(facing) << 8,
            Self::TargetBlock { power } => 13 | u64::from(power) << 8,
            Self::Source { power } => 14 | u64::from(power) << 8,
            Self::Slime => 15,
            Self::Honey => 16,
            Self::Immovable => 17,
            Self::Piston {
                facing,
                sticky,
                extended,
            } => 18 | f(facing) << 8 | b(sticky) << 12 | b(extended) << 13,
            Self::PistonHead { facing, sticky } => 19 | f(facing) << 8 | b(sticky) << 12,
            Self::Container {
                slots,
                solid,
                partner,
            } => 20 | u64::from(slots) << 8 | b(solid) << 16 | partner.map_or(7, f) << 17,
            Self::Hopper { facing } => 21 | f(facing) << 8,
            Self::Dropper { facing, triggered } => 22 | f(facing) << 8 | b(triggered) << 12,
            Self::Port(mode) => 23 | (mode as u64) << 8,
            Self::Alu => 24,
            Self::Cable => 25,
            Self::RedstonePort { output } => 26 | b(output) << 8,
            Self::Bus => 27,
        }
    }
}

/// What a piston does to a block in its way.
pub(super) enum Push {
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
    pub const ALL: [Facing; 6] = [
        Facing::North,
        Facing::South,
        Facing::East,
        Facing::West,
        Facing::Up,
        Facing::Down,
    ];
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
    pub(super) fn sides(self) -> [Facing; 2] {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Schema)]
pub enum PortMode {
    Input,
    Output,
    Bidirectional,
}
