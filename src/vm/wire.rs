//! Wire: its shape, the power it takes and passes on, and the power
//! solid blocks take from it and from components.

use super::{BlockType, Facing, PortMode, RedstoneVM};

impl RedstoneVM {
    /// The wire cells wire `i` passes power to and from: next to it, one up
    /// (nothing solid above `i`) or one down (nothing solid beside it).
    pub(super) fn wire_links(&self, i: u32) -> impl Iterator<Item = u32> + '_ {
        let open_above = !self.next(i, Facing::Up).is_solid();
        Facing::HORIZONTAL.into_iter().filter_map(move |d| {
            let n = self.at(i, d)?;
            let side = self.get(n);
            if side.is_wire() {
                return Some(n);
            }
            if side.is_solid() {
                let up = self.at(n, Facing::Up)?;
                return (open_above && self.get(up).is_wire()).then_some(up);
            }
            let down = self.at(n, Facing::Down)?;
            self.get(down).is_wire().then_some(down)
        })
    }

    /// Wire `i` turns toward direction `d` (to wire, up or down a step, or
    /// a component).
    pub(super) fn wire_turns(&self, i: u32, d: Facing) -> bool {
        let Some(n) = self.at(i, d) else { return false };
        let side = self.get(n);
        if side.is_wire() || side.wire_connects(d) {
            return true;
        }
        if side.is_solid() {
            return !self.next(i, Facing::Up).is_solid() && self.next(n, Facing::Up).is_wire();
        }
        self.next(n, Facing::Down).is_wire()
    }

    /// The directions wire `i` powers (bits of [`Facing::ALL`]): always
    /// the block below; sideways where it points (a lone dot points
    /// everywhere, a dot turned one way runs straight through).
    pub(super) fn wire_shape(&self, i: u32) -> u8 {
        let bit = |d: Facing| 1u8 << Facing::ALL.iter().position(|&f| f == d).unwrap_or(0);
        let mut turns = 0u8;
        let mut count = 0;
        for h in Facing::HORIZONTAL {
            if self.wire_turns(i, h) {
                turns |= bit(h);
                count += 1;
            }
        }
        let sides = match count {
            0 => Facing::HORIZONTAL.iter().fold(0, |m, &h| m | bit(h)),
            1 => Facing::HORIZONTAL
                .iter()
                .filter(|&&h| turns & (bit(h) | bit(h.opposite())) != 0)
                .fold(0, |m, &h| m | bit(h)),
            _ => turns,
        };
        sides | bit(Facing::Down)
    }

    /// 0. Every wire's shape, for this step.
    pub(super) fn shape_wire(&mut self) {
        for k in 0..self.wires.len() {
            let i = self.wires[k];
            if self.get(i).is_wire() {
                let shape = self.wire_shape(i);
                self.get_mut(i).points = shape;
            }
        }
    }

    /// Wire `i` powers the block in direction `d` (this step's shape).
    pub(super) fn wire_points(&self, i: u32, d: Facing) -> bool {
        let bit = Facing::ALL.iter().position(|&f| f == d).unwrap_or(0);
        self.get(i).points & (1 << bit) != 0
    }

    /// The power cell `i`'s neighbour in direction `d` gives it: a
    /// component's weak power, a solid block's strong one (with
    /// `wire`: its whole power), wire's level where it points at `i`.
    pub(super) fn input_from(&self, i: u32, d: Facing, wire: bool) -> u8 {
        let Some(n) = self.at(i, d) else { return 0 };
        let c = self.get(n);
        if c.is_wire() {
            return if wire && self.wire_points(n, d.opposite()) {
                c.power
            } else {
                0
            };
        }
        if c.is_solid() {
            return if wire { c.weak } else { c.strong };
        }
        c.emits(d.opposite()).0
    }

    /// The strongest power around cell `i`.
    pub(super) fn received(&self, i: u32, wire: bool) -> u8 {
        Facing::ALL
            .into_iter()
            .map(|d| self.input_from(i, d, wire))
            .max()
            .unwrap_or(0)
    }

    /// 1. Solid blocks take the strong power going into them.
    pub(super) fn strong_power(&mut self) {
        for k in 0..self.solids.len() {
            let i = self.solids[k];
            if !self.get(i).is_solid() {
                continue;
            }
            let strong = Facing::ALL
                .into_iter()
                .filter_map(|d| {
                    let c = self.next(i, d);
                    (!c.is_wire() && !c.is_solid()).then(|| c.emits(d.opposite()).1)
                })
                .max()
                .unwrap_or(0);
            let cell = self.get_mut(i);
            cell.strong = strong;
            cell.weak = strong;
        }
    }

    /// 2. Wire levels: what each wire gets from outside the dust, spread
    ///    along it strongest first, one less per block.
    pub(super) fn settle_wire(&mut self) {
        let mut buckets: [Vec<u32>; 16] = Default::default();
        for k in 0..self.wires.len() {
            let i = self.wires[k];
            if !self.get(i).is_wire() {
                continue;
            }
            let level = self.received(i, false);
            self.get_mut(i).power = level;
            buckets[level as usize].push(i);
        }
        for level in (2..=15u8).rev() {
            let wires = std::mem::take(&mut buckets[level as usize]);
            for i in wires {
                if self.get(i).power != level {
                    continue;
                }
                let links: Vec<u32> = self.wire_links(i).collect();
                for n in links {
                    if self.get(n).power < level - 1 {
                        self.get_mut(n).power = level - 1;
                        buckets[level as usize - 1].push(n);
                    }
                }
            }
        }
    }

    /// 3. Solid blocks add the wire leading into them.
    pub(super) fn wire_power(&mut self) {
        for k in 0..self.solids.len() {
            let i = self.solids[k];
            if !self.get(i).is_solid() {
                continue;
            }
            let from_wire = Facing::ALL
                .into_iter()
                .filter_map(|d| {
                    let n = self.at(i, d)?;
                    (self.get(n).is_wire() && self.wire_points(n, d.opposite()))
                        .then(|| self.get(n).power)
                })
                .max()
                .unwrap_or(0);
            let cell = self.get_mut(i);
            cell.weak = cell.weak.max(from_wire);
        }
    }

    /// 4. Lamps and output ports read what's around them.
    pub(super) fn read_power(&mut self) {
        self.ensure_buses();
        let n = self.buses.as_ref().map_or(0, |b| b.every_step.len());
        for k in 0..n + self.dirty_readers.len() {
            let i = match self.buses.as_ref().and_then(|b| b.every_step.get(k)) {
                Some(&i) => i,
                None => match self.dirty_readers.pop_first() {
                    Some(i) => i,
                    None => break,
                },
            };
            let cell = *self.get(i);
            let level = match cell.block {
                BlockType::RedstoneLamp => {
                    if self.received(i, true) > 0 {
                        15
                    } else {
                        0
                    }
                }
                BlockType::Port(PortMode::Output | PortMode::Bidirectional) => {
                    self.received(i, true)
                }
                _ => continue,
            };
            self.get_mut(i).power = level;
        }
    }
}
