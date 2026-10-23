//! Pistons: extending and retracting, what they push, drag and break.

use super::block::Push;
use super::cell::AIR;
use super::{BlockType, Cell, Facing, RedstoneVM};
use std::collections::{HashSet, VecDeque};

impl RedstoneVM {
    /// Powered from any side but its front, or through the block above it.
    pub(super) fn piston_powered(&self, i: u32, facing: Facing) -> bool {
        let direct = Facing::ALL
            .into_iter()
            .filter(|&d| d != facing)
            .any(|d| self.input_from(i, d, true) > 0);
        direct
            || self.at(i, Facing::Up).is_some_and(|up| {
                Facing::ALL
                    .into_iter()
                    .filter(|&d| d != Facing::Down)
                    .any(|d| self.input_from(up, d, true) > 0)
            })
    }

    /// What moving the block at `start` in direction `dir` takes along
    /// (`None`: it can't move). `piston`: the piston doing it, which never
    /// moves itself.
    pub(super) fn plan_move(
        &self,
        start: u32,
        dir: Facing,
        piston: u32,
    ) -> Option<(Vec<u32>, Vec<u32>)> {
        let mut moved = Vec::new();
        let mut seen = HashSet::new();
        let mut broken = Vec::new();
        // (cell, forced): forced cells are in the way and must go; the
        // others are only dragged by slime or honey.
        let mut queue = VecDeque::from([(start, true)]);
        while let Some((p, forced)) = queue.pop_front() {
            let c = *self.get(p);
            if c.is_air() || seen.contains(&p) {
                continue;
            }
            if p == piston {
                if forced {
                    return None;
                }
                continue;
            }
            match c.push() {
                Push::Stuck if forced => return None,
                Push::Stuck => continue,
                Push::Break => {
                    if forced && !broken.contains(&p) {
                        broken.push(p);
                    }
                    continue;
                }
                Push::Move => {}
            }
            seen.insert(p);
            moved.push(p);
            if moved.len() > 12 {
                return None;
            }
            queue.push_back((self.at(p, dir)?, true));
            if c.is_sticky() {
                for d in Facing::ALL {
                    let Some(n) = self.at(p, d) else { continue };
                    let nc = self.get(n);
                    let other_glue = nc.is_sticky() && nc.block != c.block;
                    if !nc.is_air() && !other_glue {
                        queue.push_back((n, false));
                    }
                }
            }
        }
        Some((moved, broken))
    }

    /// Moves what `plan_move` found one block in `dir`.
    pub(super) fn do_move(&mut self, moved: &[u32], broken: &[u32], dir: Facing) {
        for &p in broken {
            self.put(p, AIR);
        }
        let cells: Vec<(u32, Cell)> = moved.iter().map(|&p| (p, *self.get(p))).collect();
        for &(p, _) in &cells {
            self.put(p, AIR);
        }
        for (p, cell) in cells {
            if let Some(to) = self.at(p, dir) {
                self.put(to, cell);
            }
        }
    }

    pub(super) fn extend(&mut self, i: u32, facing: Facing, sticky: bool) {
        let Some(front) = self.at(i, facing) else {
            return;
        };
        let Some((moved, broken)) = self.plan_move(front, facing, i) else {
            return;
        };
        self.do_move(&moved, &broken, facing);
        self.put(front, Cell::new(BlockType::PistonHead { facing, sticky }));
        self.get_mut(i).block = BlockType::Piston {
            facing,
            sticky,
            extended: true,
        };
    }

    pub(super) fn retract(&mut self, i: u32, facing: Facing, sticky: bool) {
        self.get_mut(i).block = BlockType::Piston {
            facing,
            sticky,
            extended: false,
        };
        let Some(head) = self.at(i, facing) else {
            return;
        };
        if matches!(self.get(head).block, BlockType::PistonHead { .. }) {
            self.put(head, AIR);
        }
        if !sticky {
            return;
        }
        let Some(pulled) = self.at(head, facing) else {
            return;
        };
        if matches!(self.get(pulled).push(), Push::Move)
            && let Some((moved, broken)) = self.plan_move(pulled, facing.opposite(), i)
        {
            self.do_move(&moved, &broken, facing.opposite());
        }
    }
}
