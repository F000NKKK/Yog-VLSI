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

use std::collections::{HashMap, HashSet};

/// A map keyed by cell index, hashed fast.
type CellMap<V> = HashMap<u32, V, std::hash::BuildHasherDefault<Fnv>>;
/// Any other hot map, hashed fast.
type FastMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<Fnv>>;
type FastSet<K> = HashSet<K, std::hash::BuildHasherDefault<Fnv>>;

/// A fast hasher for fingerprints (FNV-1a): not for maps an adversary fills.
#[derive(Default)]
pub(crate) struct Fnv(u64);

impl std::hash::Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            self.0 = 0xcbf2_9ce4_8422_2325;
        }
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

mod block;
mod cell;
mod components;
mod grid;
mod items;
mod nested;
mod pistons;
mod step;
mod wire;

#[cfg(test)]
mod tests;

pub use block::{BlockType, ComparatorMode, Facing, PortMode};
pub use cell::Cell;
use nested::Buses;
pub use nested::{NestedAlu, NestedChip, NestedPin, NestedPort};

// ── VM ────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct RedstoneVM {
    /// Edge length of the (cubic) world, a power of two.
    pub size: u32,
    /// `log2(size)`: cell indices are `x | z << shift | y << 2 * shift`.
    shift: u32,
    /// One `size`×`size` layer per y, allocated on first placement.
    layers: Vec<Option<Box<[Cell]>>>,
    /// Indices of every non-air cell, ascending (= y, z, x order).
    active: Vec<u32>,
    /// The same, but ALUs and buses: what the redstone passes look at
    /// (a big design is mostly those).
    logic: Vec<u32>,
    /// Bumped whenever an ALU's faces change: part of the fingerprint.
    alu_changes: u64,
    /// `logic` by what each pass looks at: wire, solid blocks, what reads
    /// power (lamps, output ports), containers that move items.
    wires: Vec<u32>,
    solids: Vec<u32>,
    readers: Vec<u32>,
    movers: Vec<u32>,
    /// What decides its own next output (torches, repeaters, …) and analog cables.
    components: Vec<u32>,
    cables: Vec<u32>,
    pub tick: u64,
    /// The items of every container, by cell: (item, count) per slot.
    inventories: HashMap<u32, Vec<(u16, u8)>>,
    /// The nested ALUs' insides, by cell.
    nested: CellMap<NestedAlu>,
    /// Item ids, indexed by the numbers in `inventories`.
    items: Vec<String>,
    rng: u64,
    /// Settled, and nothing from outside changed since: another step would
    /// change nothing either, so [`settle`](Self::settle) skips it.
    quiet: bool,
    /// The buses: worked out again when a block changes (`None`).
    buses: Option<Buses>,
    /// Nested ALUs to run next step: a channel they read changed.
    dirty: std::collections::BTreeSet<u32>,
    /// Nested ALUs not settled yet (run every step till they are).
    unstable: std::collections::BTreeSet<u32>,
    /// Fed readers ([`Buses::fed`]) to look at next step.
    dirty_readers: std::collections::BTreeSet<u32>,
}

/// One slot's item: (index into `items`, count); count 0 = empty.
type Slot = (u16, u8);

/// How many of an item stack in one slot.
fn max_stack(item: &str) -> u8 {
    let path = item.strip_prefix("minecraft:").unwrap_or(item);
    const SIXTEEN: [&str; 7] = [
        "ender_pearl",
        "snowball",
        "egg",
        "_sign",
        "_banner",
        "honey_bottle",
        "armor_stand",
    ];
    const ONE: [&str; 16] = [
        "sword",
        "pickaxe",
        "_axe",
        "shovel",
        "_hoe",
        "helmet",
        "chestplate",
        "leggings",
        "boots",
        "potion",
        "shulker_box",
        "minecart",
        "boat",
        "_bed",
        "saddle",
        "totem_of_undying",
    ];
    if path == "bucket" || SIXTEEN.iter().any(|p| path.ends_with(p)) {
        16
    } else if path.ends_with("_bucket") || ONE.iter().any(|p| path.contains(p)) {
        1
    } else {
        64
    }
}
