//! The six materials a workbench stores and fabricates chips from, what its
//! resource slots take (items and their storage blocks), and what a circuit
//! costs in them.

use crate::chip::CircuitBlock;
use crate::content::Cost;
pub use crate::content::Resource;
use yog_api::yog_ids::kind;

/// An amount of each [`Resource`], in [`Resource::ALL`] order.
pub type Amounts = [u64; 6];

impl Resource {
    /// The workbench slot it's fed through (1–6; 0 is the chip, 7 the blueprint).
    pub fn slot(self) -> u32 {
        self.index() as u32 + 1
    }

    /// Units `item` is worth in this resource's slot, if it takes it (an
    /// input is an item or `#` a tag of them).
    pub fn units_of(self, item: &str) -> Option<u64> {
        self.inputs()
            .iter()
            .find(|(input, _)| match input.strip_prefix('#') {
                Some(tag) => yog_api::event::tag::has_item(item, tag),
                None => *input == item,
            })
            .map(|(_, n)| *n)
    }

    /// `units` of it for people, in whole pieces of it (an ingot, not a
    /// nugget): `2`, `2.3`, `1.5k`.
    pub fn shown(self, units: u64) -> String {
        let unit = self.unit().max(1);
        let whole = units / unit;
        let tenth = units % unit * 10 / unit;
        if whole < 10 && tenth > 0 {
            format!("{whole}.{tenth}")
        } else {
            compact(whole)
        }
    }

    /// Whether `id` is this resource itself in a design.
    fn is(self, id: &str) -> bool {
        self.base().iter().any(|p| fits(p, id))
    }
}

/// Whether `id` is `pattern` — `*` in it any run of letters.
fn fits(pattern: &str, id: &str) -> bool {
    match pattern.split_once('*') {
        Some((pre, post)) => {
            id.len() >= pre.len() + post.len() && id.starts_with(pre) && id.ends_with(post)
        }
        None => pattern == id,
    }
}

/// The game's name for one of the resources' inputs.
pub fn item_name(id: &str) -> String {
    let path = yog_api::yog_ids::game_path(id).unwrap_or(id);
    let block = path.ends_with("_block") || path == "cobblestone";
    let kind = if block { kind::BLOCK } else { kind::ITEM };
    yog_api::tr(&yog_api::yog_ids::lang_key(kind, id))
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

/// The share of a block's crafting cost fabricating it costs — and again
/// for every level an ALU nests it deeper (a chip in an ALU in the design
/// costs half of a half).
pub const PRINT_SHARE: f64 = 1.0 / 2.0;

/// What crafting `blocks` takes, in resources (unrounded).
fn craft(blocks: &[CircuitBlock]) -> [f64; 6] {
    let mut total = [0.0f64; 6];
    for b in blocks {
        add_block(b.id(), 1.0, &mut total);
    }
    total
}

/// What fabricating `blocks` costs: [`PRINT_SHARE`] of crafting them, rounded up.
#[cfg(test)]
fn cost(blocks: &[CircuitBlock]) -> Amounts {
    craft(blocks).map(|units| (units * PRINT_SHARE).ceil() as u64)
}

/// What a whole circuit costs to fabricate: [`PRINT_SHARE`] of crafting
/// its blocks, and of the circuits of the chips in its ALUs another share
/// of that for each level down, to [`MAX_NESTING`](crate::chip::MAX_NESTING)
/// (their circuits from `load`); rounded up once, at the end.
pub fn circuit_cost(
    circuit: &crate::chip::CircuitData,
    load: &dyn Fn(&str) -> Option<crate::chip::CircuitData>,
) -> Amounts {
    fn go(
        c: &crate::chip::CircuitData,
        load: &dyn Fn(&str) -> Option<crate::chip::CircuitData>,
        depth: u32,
        share: f64,
        total: &mut [f64; 6],
    ) {
        for (t, x) in total.iter_mut().zip(craft(&c.blocks)) {
            *t += x * share;
        }
        if depth >= crate::chip::MAX_NESTING {
            return;
        }
        for alu in &c.alus {
            for (_, meta) in &alu.chips {
                if let Some(inner) = load(&meta.id) {
                    go(&inner, load, depth + 1, share * PRINT_SHARE, total);
                }
            }
        }
    }
    let mut total = [0.0; 6];
    go(circuit, load, 1, PRINT_SHARE, &mut total);
    total.map(|units| units.ceil() as u64)
}

/// Adds what `n` of item/block `id` takes in crafting to `total`
/// (`cost.ydef`, down to the resources themselves).
fn add_block(id: &str, n: f64, total: &mut [f64; 6]) {
    if let Some(r) = Resource::ALL.into_iter().find(|r| r.is(id)) {
        total[r.index()] += n * r.unit() as f64;
    } else if let Some(c) = Cost::ALL
        .into_iter()
        .find(|c| c.of().iter().any(|p| fits(p, id)))
    {
        for (what, k) in c.takes() {
            add_block(what, n * k, total);
        }
    } else {
        // Anything else: a block of stone.
        total[Resource::Stone.index()] += n;
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
        let block = |state: &str| CircuitBlock {
            x: 0,
            y: 0,
            z: 0,
            state: state.into(),
            items: Vec::new(),
            redstone_port: None,
        };
        // Half of crafting, rounded up: 8 dust → 4 redstone; 4 repeaters
        // (3 redstone, 3 stone, ¼ log each) → 6, 6, 1.
        let c = cost(&vec![block("minecraft:redstone_wire"); 8]);
        assert_eq!(c, [4, 0, 0, 0, 0, 0]);
        let c = cost(&vec![block("minecraft:repeater[delay=1]"); 4]);
        assert_eq!(c[Resource::Redstone.index()], 6);
        assert_eq!(c[Resource::Stone.index()], 6);
        assert_eq!(c[Resource::Wood.index()], 1);
    }

    #[test]
    fn any_wood_is_wood() {
        let block = |state: &str| CircuitBlock {
            x: 0,
            y: 0,
            z: 0,
            state: state.into(),
            items: Vec::new(),
            redstone_port: None,
        };
        assert_eq!(
            cost(&[
                block("minecraft:spruce_planks"),
                block("minecraft:cherry_planks")
            ]),
            [0, 0, 0, 0, 1, 0]
        );
        assert_eq!(
            cost(&[
                block("minecraft:stripped_birch_log"),
                block("minecraft:crimson_stem")
            ]),
            [0, 0, 0, 0, 1, 0]
        );
    }

    #[test]
    fn a_chip_in_an_alu_in_the_design_costs_another_half() {
        use crate::chip::{ChipMeta, CircuitData, SavedAlu};
        use crate::content::Tier;
        let block = |state: &str| CircuitBlock {
            x: 0,
            y: 0,
            z: 0,
            state: state.into(),
            items: Vec::new(),
            redstone_port: None,
        };
        // The inner chip: 64 dust — 32 redstone printed alone, 16 inside an ALU.
        let inner = CircuitData {
            size: 16,
            blocks: vec![block("minecraft:redstone_wire"); 64],
            ..Default::default()
        };
        let meta = ChipMeta {
            id: "inner".into(),
            tier: Tier::Wood,
            name: "x".into(),
            ports: Vec::new(),
        };
        let outer = CircuitData {
            size: 16,
            blocks: vec![block("minecraft:redstone_wire"); 8],
            ports: Vec::new(),
            alus: vec![SavedAlu {
                x: 0,
                y: 0,
                z: 0,
                tier: Tier::Wood,
                chips: vec![(0, meta)],
                wires: Vec::new(),
            }],
        };
        let load = |id: &str| (id == "inner").then(|| inner.clone());
        assert_eq!(circuit_cost(&inner, &load)[Resource::Redstone.index()], 32);
        // 8 dust → 4, and the inner 64 → 64 / 4 = 16: 20.
        assert_eq!(circuit_cost(&outer, &load)[Resource::Redstone.index()], 20);
    }
}
