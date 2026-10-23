//! Where the chips the player hasn't placed go: in columns by how far down
//! the flow each is (what feeds a chip stands left of it), each column in
//! the order of where what feeds its chips is — wires run left to right
//! and cross little.

use std::collections::HashMap;

use super::layout::chip_height;
use super::{EDGE, GAP, HEAD, ROW};
use crate::board::{BoardView, ChipView, Node, Pin};

/// Passes over the columns: down the flow by what feeds each chip, back up
/// by what it feeds, and down again.
const SWEEPS: usize = 3;
/// A key below which every real height sorts: chips with nothing to go by
/// keep their place among the rest.
const UNKNOWN: f32 = f32::MAX / 2.0;
/// The step between unknown keys, keeping their order.
const UNKNOWN_STEP: f32 = 1e-3;

/// How far down the flow each chip is: 0 fed only by the board's sides
/// and buses, else one more than the farthest chip feeding it (a loop
/// doesn't count twice).
pub(super) fn flow_depths(view: &BoardView) -> HashMap<u8, usize> {
    let mut depth: HashMap<u8, usize> = view.chips.iter().map(|c| (c.slot, 0)).collect();
    for _ in 0..view.chips.len() {
        let mut changed = false;
        for w in &view.wires {
            if let (Node::Chip(a), Node::Chip(b)) = (w.from.node, w.to.node)
                && a != b
            {
                let d = depth.get(&a).copied().unwrap_or(0) + 1;
                if let Some(e) = depth.get_mut(&b)
                    && *e < d
                    && d < view.chips.len()
                {
                    *e = d;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    depth
}

/// Where the chips the player hasn't placed go: a column each depth
/// ([`flow_depths`]) from `x0`, `pitch` apart, each column sorted by the
/// mean height of what feeds its chips (`source_y` for the sides' and
/// buses' pins; a chip's own outputs where it was put).
pub(super) fn flow_layout(
    view: &BoardView,
    placed: &dyn Fn(Node) -> Option<(f32, f32)>,
    x0: f32,
    pitch: f32,
    source_y: &dyn Fn(&Pin) -> Option<f32>,
) -> HashMap<u8, (f32, f32)> {
    let depth = flow_depths(view);
    let max_depth = depth.values().copied().max().unwrap_or(0);
    let mut columns: Vec<Vec<&ChipView>> = (0..=max_depth)
        .map(|d| {
            view.chips
                .iter()
                .filter(|c| depth.get(&c.slot) == Some(&d) && placed(Node::Chip(c.slot)).is_none())
                .collect()
        })
        .collect();
    let mut f = Flow {
        view,
        placed,
        source_y,
        at: HashMap::new(),
    };
    let x_of = |d: usize| x0 + d as f32 * pitch;
    for sweep in 0..SWEEPS {
        if sweep % 2 == 0 {
            for (d, column) in columns.iter_mut().enumerate() {
                // The last times round, both ways: between what feeds it and what it feeds.
                sort_by(column, &|slot| match (f.fed_from(slot), f.feeds(slot)) {
                    (Some(a), Some(b)) if sweep > 0 => Some((a + b) / 2.0),
                    (a, b) => a.or(b),
                });
                f.stack(column, x_of(d));
            }
        } else {
            for (d, column) in columns.iter_mut().enumerate().take(max_depth).rev() {
                sort_by(column, &|slot| f.feeds(slot));
                f.stack(column, x_of(d));
            }
        }
    }
    f.at
}

/// `column` sorted by `key`, those with none keeping their place.
fn sort_by(column: &mut Vec<&ChipView>, key: &dyn Fn(u8) -> Option<f32>) {
    let keys: Vec<f32> = column
        .iter()
        .enumerate()
        .map(|(i, c)| key(c.slot).unwrap_or(i as f32 * UNKNOWN_STEP + UNKNOWN))
        .collect();
    let mut idx: Vec<usize> = (0..column.len()).collect();
    idx.sort_by(|&a, &b| {
        keys[a]
            .total_cmp(&keys[b])
            .then(column[a].slot.cmp(&column[b].slot))
    });
    *column = idx.into_iter().map(|i| column[i]).collect();
}

fn mean(ys: &[f32]) -> Option<f32> {
    (!ys.is_empty()).then(|| ys.iter().sum::<f32>() / ys.len() as f32)
}

/// The columns being laid out: where each chip is so far.
struct Flow<'a> {
    view: &'a BoardView,
    placed: &'a dyn Fn(Node) -> Option<(f32, f32)>,
    source_y: &'a dyn Fn(&Pin) -> Option<f32>,
    at: HashMap<u8, (f32, f32)>,
}

impl Flow<'_> {
    /// Where the chip in `slot` is: placed, else laid out so far.
    fn chip_at(&self, slot: u8) -> Option<(f32, f32)> {
        (self.placed)(Node::Chip(slot)).or_else(|| self.at.get(&slot).copied())
    }

    /// The height of a chip's pin `pin`, among its outputs or its inputs.
    fn chip_pin_y(&self, pin: &Pin, output: bool) -> Option<f32> {
        let Node::Chip(slot) = pin.node else {
            return None;
        };
        let chip = self.view.chips.iter().find(|c| c.slot == slot)?;
        let row = chip
            .pins
            .iter()
            .filter(|(_, m)| if output { m.outputs() } else { m.inputs() })
            .position(|(n, _)| *n == pin.name)?;
        let (_, y) = self.chip_at(slot)?;
        Some(y + HEAD + row as f32 * ROW + ROW / 2.0)
    }

    /// Where a wire into `pin` ends: a chip's input row, or an Out side's.
    fn input_y(&self, pin: &Pin) -> Option<f32> {
        match pin.node {
            Node::Chip(_) => self.chip_pin_y(pin, false),
            Node::Out => {
                let row = self.view.sides.iter().position(|s| s.side == pin.name)?;
                let (_, y) = (self.placed)(Node::Out).unwrap_or((0.0, EDGE));
                Some(y + HEAD + row as f32 * ROW + ROW / 2.0)
            }
            _ => None,
        }
    }

    /// The mean height of what feeds the chip in `slot`.
    fn fed_from(&self, slot: u8) -> Option<f32> {
        let ys: Vec<f32> = self
            .view
            .wires
            .iter()
            .filter(|w| w.to.node == Node::Chip(slot))
            .filter_map(|w| (self.source_y)(&w.from).or_else(|| self.chip_pin_y(&w.from, true)))
            .collect();
        mean(&ys)
    }

    /// The mean height of what the chip in `slot` feeds.
    fn feeds(&self, slot: u8) -> Option<f32> {
        let ys: Vec<f32> = self
            .view
            .wires
            .iter()
            .filter(|w| w.from.node == Node::Chip(slot))
            .filter_map(|w| self.input_y(&w.to))
            .collect();
        mean(&ys)
    }

    /// Stacks `column` at `x`, each chip level with what feeds it where
    /// there's room.
    fn stack(&mut self, column: &[&ChipView], x: f32) {
        let mut y = EDGE;
        for c in column {
            let want = self
                .fed_from(c.slot)
                .map_or(y, |m| (m - HEAD - ROW / 2.0).max(y));
            self.at.insert(c.slot, (x, want));
            y = want + chip_height(c) + GAP;
        }
    }
}
