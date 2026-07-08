//! Rust redstone VM: accelerated redstone simulation for microchips.
//!
//! Each chip tier maps to a world size. The VM simulates redstone ticks
//! at configurable speed (up to 40 ticks/s for Netherite, 2× vanilla).
//!
//! Supported: redstone dust, repeaters, comparators, torches, pistons,
//! hoppers, chests, droppers, dispensers, observers, note blocks, lamps.
//! Entities (pearls, etc.) are purged from the virtual world.

/// Microchip tier definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Wood,     //  5 ticks/s, 16×16
    Stone,    // 10 ticks/s, 32×32
    Gold,     // 20 ticks/s, 64×64
    Iron,     // 25 ticks/s, 64×64
    Diamond,  // 30 ticks/s, 128×128
    Netherite,// 40 ticks/s, 256×256
}

impl Tier {
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
}
