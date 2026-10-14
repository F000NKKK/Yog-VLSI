//! The six materials a workbench stores and fabricates chips from, what its
//! resource slots take (items and their storage blocks), and what a circuit
//! costs in them.

use serde::{Deserialize, Serialize};

use crate::chip::CircuitBlock;

/// One stored material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resource {
    Redstone,
    Iron,
    Gold,
    Quartz,
    Wood,
    Stone,
}

/// An amount of each [`Resource`], in [`Resource::ALL`] order.
pub type Amounts = [u64; 6];

impl Resource {
    pub const ALL: [Resource; 6] = [
        Resource::Redstone,
        Resource::Iron,
        Resource::Gold,
        Resource::Quartz,
        Resource::Wood,
        Resource::Stone,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The workbench slot it's fed through (1–6; 0 is the chip, 7 the blueprint).
    pub fn slot(self) -> u32 {
        self.index() as u32 + 1
    }

    /// What its slot takes and how many units each item is worth.
    pub fn inputs(self) -> &'static [(&'static str, u64)] {
        match self {
            Resource::Redstone => &[("minecraft:redstone", 1), ("minecraft:redstone_block", 9)],
            Resource::Iron => &[("minecraft:iron_ingot", 1), ("minecraft:iron_block", 9)],
            Resource::Gold => &[("minecraft:gold_ingot", 1), ("minecraft:gold_block", 9)],
            Resource::Quartz => &[("minecraft:quartz", 1), ("minecraft:quartz_block", 4)],
            Resource::Wood => &[("minecraft:oak_log", 1)],
            Resource::Stone => &[("minecraft:cobblestone", 1)],
        }
    }

    /// Units `item` is worth in this resource's slot, if it takes it.
    pub fn units_of(self, item: &str) -> Option<u64> {
        self.inputs().iter().find(|(id, _)| *id == item).map(|(_, n)| *n)
    }

    /// Its slot's `accepts` list, as `.yui` writes it.
    pub fn accepts(self) -> String {
        self.inputs().iter().map(|(id, _)| *id).collect::<Vec<_>>().join(" ")
    }

    /// The game's name for it (the base item's translation).
    pub fn name(self) -> String {
        let key = match self {
            Resource::Redstone => "item.minecraft.redstone",
            Resource::Iron => "item.minecraft.iron_ingot",
            Resource::Gold => "item.minecraft.gold_ingot",
            Resource::Quartz => "item.minecraft.quartz",
            Resource::Wood => "block.minecraft.oak_log",
            Resource::Stone => "block.minecraft.cobblestone",
        };
        yog_api::tr(key)
    }

    /// The vial's colour.
    pub fn color(self) -> u32 {
        use crate::res::Colors;
        match self {
            Resource::Redstone => Colors::VIAL_REDSTONE,
            Resource::Iron => Colors::VIAL_IRON,
            Resource::Gold => Colors::VIAL_GOLD,
            Resource::Quartz => Colors::VIAL_QUARTZ,
            Resource::Wood => Colors::VIAL_WOOD,
            Resource::Stone => Colors::VIAL_COBBLESTONE,
        }
    }
}

/// `n` short: `999`, `1.2k`, `15k`, `1.5m`, `2b`, `3t`.
pub fn compact(n: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1_000_000_000_000, "t"),
        (1_000_000_000, "b"),
        (1_000_000, "m"),
        (1_000, "k"),
    ];
    for (size, unit) in UNITS {
        if n >= size {
            let whole = n / size;
            let tenth = n % size * 10 / size;
            return if whole < 10 && tenth > 0 {
                format!("{whole}.{tenth}{unit}")
            } else {
                format!("{whole}{unit}")
            };
        }
    }
    n.to_string()
}

/// What a circuit costs to fabricate: a quarter of its blocks' crafting
/// cost, rounded up.
pub fn cost(blocks: &[CircuitBlock]) -> Amounts {
    let mut total = [0.0f64; 6];
    for b in blocks {
        add_block(b.id(), 1.0, &mut total);
    }
    total.map(|units| (units / 4.0).ceil() as u64)
}

fn add(r: Resource, n: f64, total: &mut [f64; 6]) {
    total[r.index()] += n;
}

/// Adds what `n` of item/block `id` takes in crafting to `total`.
fn add_block(id: &str, n: f64, total: &mut [f64; 6]) {
    use Resource::*;
    let mut item = |id: &str, k: f64| add_block(id, n * k, total);
    match id {
        // The six materials themselves.
        "minecraft:redstone" | "minecraft:redstone_wire" => add(Redstone, n, total),
        "minecraft:iron_ingot" => add(Iron, n, total),
        "minecraft:gold_ingot" => add(Gold, n, total),
        "minecraft:quartz" => add(Quartz, n, total),
        "minecraft:oak_log" => add(Wood, n, total),
        "minecraft:cobblestone" => add(Stone, n, total),
        // Plain intermediates.
        "minecraft:oak_planks" => item("minecraft:oak_log", 0.25),
        "minecraft:stick" => item("minecraft:oak_log", 0.125),
        "minecraft:stone" | "minecraft:smooth_stone" | "minecraft:stone_button" => item("minecraft:cobblestone", 1.0),
        "minecraft:stone_pressure_plate" => item("minecraft:cobblestone", 2.0),
        "minecraft:gold_nugget" => item("minecraft:gold_ingot", 1.0 / 9.0),
        "minecraft:copper_ingot" => item("minecraft:iron_ingot", 0.5),
        "minecraft:redstone_block" => item("minecraft:redstone", 9.0),
        // No resource of their own: the nearest one.
        "minecraft:glowstone_dust" | "minecraft:gunpowder" => item("minecraft:redstone", 1.0),
        "minecraft:slime_ball" | "minecraft:honey_bottle" | "minecraft:hay_block" | "minecraft:sand"
        | "minecraft:bow" | "minecraft:blaze_rod" => item("minecraft:oak_log", 1.0),
        "minecraft:obsidian" => item("minecraft:cobblestone", 4.0),
        "minecraft:prismarine_shard" | "minecraft:prismarine_crystals" | "minecraft:ender_eye"
        | "minecraft:diamond" => item("minecraft:quartz", 1.0),
        "minecraft:tripwire_hook" => {
            item("minecraft:iron_ingot", 1.0);
            item("minecraft:stick", 1.0);
        }
        // Components.
        "minecraft:redstone_torch" | "minecraft:redstone_wall_torch" => {
            item("minecraft:redstone", 1.0);
            item("minecraft:stick", 1.0);
        }
        "minecraft:repeater" => {
            item("minecraft:redstone_torch", 2.0);
            item("minecraft:redstone", 1.0);
            item("minecraft:stone", 3.0);
        }
        "minecraft:comparator" => {
            item("minecraft:redstone_torch", 3.0);
            item("minecraft:quartz", 1.0);
            item("minecraft:stone", 3.0);
        }
        "minecraft:redstone_lamp" => {
            item("minecraft:redstone", 4.0);
            item("minecraft:glowstone_dust", 4.0);
        }
        "minecraft:lever" => {
            item("minecraft:stick", 1.0);
            item("minecraft:cobblestone", 1.0);
        }
        "minecraft:observer" => {
            item("minecraft:cobblestone", 6.0);
            item("minecraft:redstone", 2.0);
            item("minecraft:quartz", 1.0);
        }
        "minecraft:note_block" => {
            item("minecraft:oak_planks", 8.0);
            item("minecraft:redstone", 1.0);
        }
        "minecraft:target" => {
            item("minecraft:redstone", 4.0);
            item("minecraft:hay_block", 1.0);
        }
        "minecraft:piston" => {
            item("minecraft:oak_planks", 3.0);
            item("minecraft:cobblestone", 4.0);
            item("minecraft:iron_ingot", 1.0);
            item("minecraft:redstone", 1.0);
        }
        "minecraft:sticky_piston" => {
            item("minecraft:piston", 1.0);
            item("minecraft:slime_ball", 1.0);
        }
        "minecraft:chest" | "minecraft:barrel" => item("minecraft:oak_planks", 8.0),
        "minecraft:trapped_chest" => {
            item("minecraft:chest", 1.0);
            item("minecraft:tripwire_hook", 1.0);
        }
        "minecraft:ender_chest" => {
            item("minecraft:obsidian", 8.0);
            item("minecraft:ender_eye", 1.0);
        }
        "minecraft:hopper" => {
            item("minecraft:iron_ingot", 5.0);
            item("minecraft:chest", 1.0);
        }
        "minecraft:dropper" => {
            item("minecraft:cobblestone", 7.0);
            item("minecraft:redstone", 1.0);
        }
        "minecraft:dispenser" => {
            item("minecraft:dropper", 1.0);
            item("minecraft:bow", 1.0);
        }
        "minecraft:furnace" => item("minecraft:cobblestone", 8.0),
        "minecraft:blast_furnace" => {
            item("minecraft:smooth_stone", 3.0);
            item("minecraft:furnace", 1.0);
            item("minecraft:iron_ingot", 5.0);
        }
        "minecraft:smoker" => {
            item("minecraft:oak_log", 4.0);
            item("minecraft:furnace", 1.0);
        }
        "minecraft:brewing_stand" => {
            item("minecraft:blaze_rod", 1.0);
            item("minecraft:cobblestone", 3.0);
        }
        "minecraft:slime_block" => item("minecraft:slime_ball", 9.0),
        "minecraft:honey_block" => item("minecraft:honey_bottle", 4.0),
        "minecraft:tnt" => {
            item("minecraft:sand", 4.0);
            item("minecraft:gunpowder", 5.0);
        }
        "minecraft:rail" => {
            item("minecraft:iron_ingot", 6.0 / 16.0);
            item("minecraft:stick", 1.0 / 16.0);
        }
        "minecraft:powered_rail" => {
            item("minecraft:gold_ingot", 1.0);
            item("minecraft:stick", 1.0 / 6.0);
            item("minecraft:redstone", 1.0 / 6.0);
        }
        "minecraft:detector_rail" => {
            item("minecraft:iron_ingot", 1.0);
            item("minecraft:stone_pressure_plate", 1.0 / 6.0);
            item("minecraft:redstone", 1.0 / 6.0);
        }
        "minecraft:activator_rail" => {
            item("minecraft:iron_ingot", 1.0);
            item("minecraft:stick", 2.0 / 6.0);
            item("minecraft:redstone_torch", 1.0 / 6.0);
        }
        "minecraft:iron_door" => item("minecraft:iron_ingot", 2.0),
        "minecraft:iron_trapdoor" => item("minecraft:iron_ingot", 4.0),
        "minecraft:glass" => item("minecraft:sand", 1.0),
        "minecraft:glowstone" => item("minecraft:glowstone_dust", 4.0),
        "minecraft:sea_lantern" => {
            item("minecraft:prismarine_shard", 4.0);
            item("minecraft:prismarine_crystals", 5.0);
        }
        // VLSI's own blocks.
        "yog-vlsi:port_input" | "yog-vlsi:port_output" | "yog-vlsi:port_bidi" => {
            item("minecraft:redstone", 2.0);
            item("minecraft:stone", 4.0);
        }
        "yog-vlsi:analog_cable" => item("minecraft:redstone", 1.0 / 3.0),
        "yog-vlsi:digital_cable" => {
            item("minecraft:gold_nugget", 1.0);
            item("minecraft:redstone", 1.0);
            item("minecraft:iron_ingot", 0.25);
        }
        "yog-vlsi:redstone_port" => {
            item("minecraft:smooth_stone", 8.0);
            item("minecraft:redstone", 1.0);
        }
        // Anything else: a block of stone.
        _ => add(Stone, n, total),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_numbers() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_200), "1.2k");
        assert_eq!(compact(15_400), "15k");
        assert_eq!(compact(200_000), "200k");
        assert_eq!(compact(1_500_000), "1.5m");
        assert_eq!(compact(2_000_000_000), "2b");
    }

    #[test]
    fn cost_in_resources() {
        let block = |state: &str| CircuitBlock { x: 0, y: 0, z: 0, state: state.into() };
        // 4 dust → 1 redstone; a repeater → 3 redstone, 3 stone, ¼ log.
        let c = cost(&[block("minecraft:redstone_wire"); 4]);
        assert_eq!(c, [1, 0, 0, 0, 0, 0]);
        let c = cost(&[block("minecraft:repeater[delay=1]"); 4]);
        assert_eq!(c[Resource::Redstone.index()], 3);
        assert_eq!(c[Resource::Stone.index()], 3);
        assert_eq!(c[Resource::Wood.index()], 1);
    }
}
