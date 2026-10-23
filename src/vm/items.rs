//! Containers and their items: hoppers, droppers, what comparators read.

use super::{BlockType, Cell, Facing, RedstoneVM, Slot, max_stack};

impl RedstoneVM {
    /// The cells whose slots make up the container at `i` (a double chest
    /// is two).
    pub(super) fn container(&self, i: u32) -> Vec<u32> {
        let mut cells = vec![i];
        if let BlockType::Container {
            partner: Some(d), ..
        } = self.get(i).block
            && let Some(p) = self.at(i, d)
            && matches!(self.get(p).block, BlockType::Container { partner: Some(back), .. } if back == d.opposite())
        {
            cells.push(p);
        }
        cells
    }

    /// How full the container at `i` is, as a comparator reads it.
    pub(super) fn fill(&self, i: u32) -> u8 {
        let (mut sum, mut slots, mut any) = (0.0f32, 0usize, false);
        for c in self.container(i) {
            for &(id, n) in self.inventories.get(&c).into_iter().flatten() {
                slots += 1;
                if n > 0 {
                    any = true;
                    sum += n as f32 / max_stack(&self.items[id as usize]) as f32;
                }
            }
        }
        if !any || slots == 0 {
            0
        } else {
            (1.0 + sum / slots as f32 * 14.0).floor() as u8
        }
    }

    /// A comparator's back: a container's fill, through one solid block
    /// too; else the power coming in.
    pub(super) fn comparator_back(&self, i: u32, facing: Facing) -> u8 {
        let power = self.input_from(i, facing, true);
        let Some(b) = self.at(i, facing) else {
            return power;
        };
        let back = self.get(b);
        if back.holds_items() {
            return self.fill(b);
        }
        if back.is_solid()
            && power < 15
            && let Some(b2) = self.at(b, facing)
            && self.get(b2).holds_items()
        {
            return power.max(self.fill(b2));
        }
        power
    }

    /// Takes one item out of the first non-empty slot of `from` (or a
    /// random one) that `to` takes; `to = None` throws it away.
    pub(super) fn transfer(&mut self, from: u32, to: Option<u32>, random: bool) -> bool {
        let Some(inv) = self.inventories.get(&from) else {
            return false;
        };
        let mut full: Vec<usize> = inv
            .iter()
            .enumerate()
            .filter(|(_, s)| s.1 > 0)
            .map(|(k, _)| k)
            .collect();
        if random && !full.is_empty() {
            self.rng = self
                .rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let pick = (self.rng >> 33) as usize % full.len();
            full = vec![full[pick]];
        }
        for slot in full {
            let item = self.inventories[&from][slot].0;
            let ok = match to {
                Some(to) => self.insert(to, item),
                None => true,
            };
            if ok {
                let s = &mut self.inventories.get_mut(&from).unwrap()[slot];
                s.1 -= 1;
                return true;
            }
        }
        false
    }

    /// Puts one `item` into the container at `to`.
    pub(super) fn insert(&mut self, to: u32, item: u16) -> bool {
        let max = max_stack(&self.items[item as usize]);
        let cells = self.container(to);
        for pass in 0..2 {
            for &c in &cells {
                let Some(inv) = self.inventories.get_mut(&c) else {
                    continue;
                };
                let found = inv.iter_mut().find(|s: &&mut Slot| {
                    if pass == 0 {
                        s.1 > 0 && s.0 == item && s.1 < max
                    } else {
                        s.1 == 0
                    }
                });
                if let Some(s) = found {
                    *s = (item, s.1 + 1);
                    return true;
                }
            }
        }
        false
    }

    /// Hoppers move an item every 4 steps unless powered; droppers drop
    /// theirs.
    pub(super) fn move_items(&mut self) {
        for k in 0..self.movers.len() {
            let Some(&i) = self.movers.get(k) else { break };
            let cell = *self.get(i);
            match cell.block {
                BlockType::Hopper { facing } => {
                    if cell.timer > 0 {
                        self.get_mut(i).timer -= 1;
                        if cell.timer > 1 {
                            continue;
                        }
                    }
                    if self.received(i, true) > 0 {
                        continue;
                    }
                    let out = self.at(i, facing).filter(|&t| self.get(t).holds_items());
                    let mut moved = out.is_some_and(|t| self.transfer(i, Some(t), false));
                    if let Some(up) = self
                        .at(i, Facing::Up)
                        .filter(|&u| self.get(u).holds_items())
                    {
                        moved |= self.transfer(up, Some(i), false);
                    }
                    if moved {
                        self.get_mut(i).timer = 4;
                    }
                }
                BlockType::Dropper { facing, .. } if cell.timer > 0 => {
                    self.get_mut(i).timer -= 1;
                    if cell.timer == 1 {
                        let to = self.at(i, facing).filter(|&t| self.get(t).holds_items());
                        self.transfer(i, to, true);
                    }
                }
                _ => {}
            }
        }
    }

    /// A repeater takes its input `delay` steps later; once on, it stays on
    /// at least `delay` steps. A powered repeater or comparator pointing
    /// into its side locks it as it is.
    pub(super) fn next_repeater(&self, i: u32, mut cell: Cell, delay: u8, facing: Facing) -> Cell {
        let locked = facing.sides().into_iter().any(|d| {
            let c = self.next(i, d);
            matches!(
                c.block,
                BlockType::Repeater { .. } | BlockType::Comparator { .. }
            ) && c.emits(d.opposite()).0 > 0
        });
        if locked {
            cell.timer = 0;
            return cell;
        }
        let input = self.input_from(i, facing, true) > 0;
        let on = cell.power > 0;
        if cell.timer == 0 {
            if input == on {
                return cell;
            }
            cell.timer = delay;
        }
        cell.timer -= 1;
        if cell.timer == 0 {
            if on && !input {
                cell.power = 0;
            } else if !on {
                cell.power = 15;
                if !input {
                    // A short pulse still comes out `delay` steps long.
                    cell.timer = delay;
                }
            }
        }
        cell
    }
}
