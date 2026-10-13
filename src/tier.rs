//! The six material tiers shared by microchips and ALUs (DESIGN.md §2.1,
//! §4.1): every per-tier number and id lives here.

use serde::{Deserialize, Serialize};

use crate::content::{Blocks, Items};
use crate::res::Strings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tier {
    Wood,
    Stone,
    Gold,
    Iron,
    Diamond,
    Netherite,
}

/// Server ticks per second; VM steps are scheduled against it.
pub const SERVER_TICKS_PER_SECOND: u32 = 20;

impl Tier {
    pub const ALL: [Tier; 6] = [
        Tier::Wood,
        Tier::Stone,
        Tier::Gold,
        Tier::Iron,
        Tier::Diamond,
        Tier::Netherite,
    ];

    /// VM steps per second (vanilla redstone is 20).
    pub fn tick_rate(self) -> u32 {
        match self {
            Tier::Wood => 5,
            Tier::Stone => 10,
            Tier::Gold => 20,
            Tier::Iron => 25,
            Tier::Diamond => 30,
            Tier::Netherite => 40,
        }
    }

    /// Edge length of the chip's virtual world.
    pub fn world_size(self) -> u32 {
        match self {
            Tier::Wood => 16,
            Tier::Stone => 32,
            Tier::Gold | Tier::Iron => 64,
            Tier::Diamond => 128,
            Tier::Netherite => 256,
        }
    }

    /// Chips an ALU of this tier holds.
    pub fn max_chips(self) -> usize {
        match self {
            Tier::Wood => 2,
            Tier::Stone => 3,
            Tier::Gold => 4,
            Tier::Iron => 5,
            Tier::Diamond => 6,
            Tier::Netherite => 8,
        }
    }

    /// Digital cable channels per ALU side (each × 4 colours).
    pub fn digital_channels(self) -> u32 {
        match self {
            Tier::Wood => 8,
            Tier::Stone => 16,
            Tier::Gold => 24,
            Tier::Iron => 32,
            Tier::Diamond => 48,
            Tier::Netherite => 64,
        }
    }

    /// Lowercase id used in commands and ids (`wood`, …).
    pub fn id(self) -> &'static str {
        match self {
            Tier::Wood => "wood",
            Tier::Stone => "stone",
            Tier::Gold => "gold",
            Tier::Iron => "iron",
            Tier::Diamond => "diamond",
            Tier::Netherite => "netherite",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.id().eq_ignore_ascii_case(s))
    }

    /// Localized display name.
    pub fn name(self) -> String {
        match self {
            Tier::Wood => Strings::tier_wood(),
            Tier::Stone => Strings::tier_stone(),
            Tier::Gold => Strings::tier_gold(),
            Tier::Iron => Strings::tier_iron(),
            Tier::Diamond => Strings::tier_diamond(),
            Tier::Netherite => Strings::tier_netherite(),
        }
    }

    /// The microchip item of this tier.
    pub fn chip_item(self) -> &'static str {
        match self {
            Tier::Wood => Items::CHIP_WOOD,
            Tier::Stone => Items::CHIP_STONE,
            Tier::Gold => Items::CHIP_GOLD,
            Tier::Iron => Items::CHIP_IRON,
            Tier::Diamond => Items::CHIP_DIAMOND,
            Tier::Netherite => Items::CHIP_NETHERITE,
        }
    }

    pub fn from_chip_item(id: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.chip_item() == id)
    }

    /// The ALU block of this tier.
    pub fn alu_block(self) -> &'static str {
        match self {
            Tier::Wood => Blocks::ALU_WOOD,
            Tier::Stone => Blocks::ALU_STONE,
            Tier::Gold => Blocks::ALU_GOLD,
            Tier::Iron => Blocks::ALU_IRON,
            Tier::Diamond => Blocks::ALU_DIAMOND,
            Tier::Netherite => Blocks::ALU_NETHERITE,
        }
    }

    pub fn from_alu_block(id: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.alu_block() == id)
    }

    /// `(corner, centre)` ingredients of this tier's ALU recipe.
    pub fn alu_recipe_materials(self) -> (&'static str, &'static str) {
        match self {
            Tier::Wood => ("minecraft:oak_planks", "minecraft:redstone_block"),
            Tier::Stone => ("minecraft:cobblestone", "minecraft:redstone_block"),
            Tier::Gold => ("minecraft:gold_ingot", "minecraft:diamond"),
            Tier::Iron => ("minecraft:iron_ingot", "minecraft:diamond"),
            Tier::Diamond => ("minecraft:diamond", "minecraft:diamond_block"),
            Tier::Netherite => ("minecraft:netherite_ingot", "minecraft:diamond_block"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for t in Tier::ALL {
            assert_eq!(Tier::parse(t.id()), Some(t));
            assert_eq!(Tier::from_chip_item(t.chip_item()), Some(t));
            assert_eq!(Tier::from_alu_block(t.alu_block()), Some(t));
        }
        assert_eq!(Tier::parse("NETHERITE"), Some(Tier::Netherite));
        assert_eq!(Tier::parse("copper"), None);
    }

    #[test]
    fn tiers_grow() {
        for pair in Tier::ALL.windows(2) {
            assert!(pair[0].tick_rate() < pair[1].tick_rate());
            assert!(pair[0].world_size() <= pair[1].world_size());
            assert!(pair[0].max_chips() < pair[1].max_chips());
        }
    }
}
