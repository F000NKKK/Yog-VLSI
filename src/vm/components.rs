//! Components: what each decides its next output from, cables and the
//! buses between nested ALUs.

use super::nested::Buses;
use super::{BlockType, Cell, ComparatorMode, Facing, FastMap, FastSet, NestedPin, RedstoneVM};
use std::collections::VecDeque;

impl RedstoneVM {
    /// The power a comparator's side reads: only wire, redstone blocks and
    /// repeaters/comparators pointing at it count.
    pub(super) fn side_input(&self, i: u32, d: Facing) -> u8 {
        let Some(n) = self.at(i, d) else { return 0 };
        let c = self.get(n);
        match c.block {
            BlockType::RedstoneWire => c.power,
            BlockType::RedstoneBlock => 15,
            BlockType::Repeater { .. } | BlockType::Comparator { .. } => c.emits(d.opposite()).0,
            _ => 0,
        }
    }

    /// 5a. Analog cables: each run carries the strongest level put into it
    /// — by an input Redstone Port (the redstone around it) or an ALU's face
    /// — to the ALUs' faces on it and out of its output ports. Worked out
    /// from its sources each step: a source gone, the level goes.
    pub(super) fn cable_networks(&mut self) {
        let mut seen: FastSet<u32> = FastSet::default();
        let mut changes: Vec<(u32, u8)> = Vec::new();
        for k in 0..self.cables.len() {
            let start = self.cables[k];
            if self.get(start).block != BlockType::Cable || !seen.insert(start) {
                continue;
            }
            let (mut run, mut outs, mut level) = (vec![start], Vec::new(), 0u8);
            let mut queue = VecDeque::from([start]);
            while let Some(i) = queue.pop_front() {
                for d in Facing::ALL {
                    let Some(n) = self.at(i, d) else { continue };
                    match self.get(n).block {
                        BlockType::Cable => {
                            if seen.insert(n) {
                                run.push(n);
                                queue.push_back(n);
                            }
                        }
                        BlockType::Alu => level = level.max(self.get(n).emits(d.opposite()).0),
                        BlockType::RedstonePort { output: false } => {
                            level = level.max(self.received(n, true))
                        }
                        BlockType::RedstonePort { output: true } => outs.push(n),
                        _ => {}
                    }
                }
            }
            changes.extend(run.into_iter().chain(outs).map(|i| (i, level)));
        }
        for (i, level) in changes {
            self.get_mut(i).power = level;
        }
    }

    /// The buses' runs, which ALU faces are on them, and who drives and
    /// reads each channel — worked out when blocks change; then every ALU
    /// runs once.
    pub(super) fn ensure_buses(&mut self) {
        if self.buses.is_some() {
            return;
        }
        let mut b = Buses::default();
        for k in 0..self.active.len() {
            let start = self.active[k];
            if self.get(start).block != BlockType::Bus || b.run.contains_key(&start) {
                continue;
            }
            let run = b.levels.len();
            b.levels.push(Vec::new());
            b.drivers.push(FastMap::default());
            b.readers.push(FastMap::default());
            b.run.insert(start, run);
            let mut queue = VecDeque::from([start]);
            while let Some(i) = queue.pop_front() {
                for d in Facing::ALL {
                    let Some(n) = self.at(i, d) else { continue };
                    match self.get(n).block {
                        BlockType::Bus if !b.run.contains_key(&n) => {
                            b.run.insert(n, run);
                            queue.push_back(n);
                        }
                        // The face of that ALU this cable touches.
                        BlockType::Alu => {
                            b.alu_runs.entry(n).or_insert([None; 6])[d.opposite().index()] =
                                Some(run)
                        }
                        _ => {}
                    }
                }
            }
        }
        for (&i, alu) in &self.nested {
            let Some(runs) = b.alu_runs.get(&i) else {
                continue;
            };
            for &(f, ch) in &alu.reads {
                if let Some(r) = runs[f as usize % 6] {
                    b.readers[r].entry(ch).or_default().push(i);
                }
            }
            for (f, ch) in alu.sinks.iter().filter_map(|p| match *p {
                NestedPin::Bus {
                    side: f,
                    channel: ch,
                } => Some((f, ch)),
                _ => None,
            }) {
                if let Some(r) = runs[f as usize % 6] {
                    b.drivers[r].entry(ch).or_default().push((i, f));
                }
            }
        }
        // What beside a cell changes only when an ALU or a port does.
        let calm = |vm: &RedstoneVM, i: u32| {
            Facing::ALL.into_iter().all(|f| {
                matches!(
                    vm.next(i, f).block,
                    BlockType::Air | BlockType::Alu | BlockType::Bus | BlockType::Port(_)
                )
            })
        };
        b.always = self
            .nested
            .iter()
            .filter(|&(&i, a)| a.reads_faces && !calm(self, i))
            .map(|(&i, _)| i)
            .collect();
        b.always.sort_unstable();
        b.fed = self
            .readers
            .iter()
            .copied()
            .filter(|&i| calm(self, i))
            .collect();
        b.every_step = self
            .readers
            .iter()
            .copied()
            .filter(|i| !b.fed.contains(i))
            .collect();
        self.dirty.extend(self.nested.keys().copied());
        self.dirty_readers.extend(b.fed.iter().copied());
        self.buses = Some(b);
    }

    /// Something beside cell `i` changed: the ALUs and fed readers next to
    /// it look again next step.
    pub(super) fn touch_around(&mut self, i: u32) {
        for f in Facing::ALL {
            let Some(n) = self.at(i, f) else { continue };
            if self.nested.contains_key(&n) {
                self.dirty.insert(n);
            }
            if self.buses.as_ref().is_some_and(|b| b.fed.contains(&n)) {
                self.dirty_readers.insert(n);
            }
        }
    }

    /// Channel `ch` of run `r` again, from its drivers; who reads it, if it changed.
    pub(super) fn update_channel(&mut self, r: usize, ch: u16) {
        let Some(b) = &mut self.buses else { return };
        let level = b.drivers[r]
            .get(&ch)
            .into_iter()
            .flatten()
            .map(|(alu, f)| {
                self.nested
                    .get(alu)
                    .and_then(|a| a.drives.get(&(*f, ch)))
                    .copied()
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0);
        let levels = &mut b.levels[r];
        if levels.len() <= ch as usize {
            levels.resize(ch as usize + 1, 0);
        }
        if levels[ch as usize] != level {
            levels[ch as usize] = level;
            self.dirty
                .extend(b.readers[r].get(&ch).into_iter().flatten().copied());
        }
    }

    /// 5. Every component decides its next output.
    pub(super) fn settle_components(&mut self) {
        self.cable_networks();
        let mut pistons = Vec::new();
        let changes: Vec<(u32, Cell)> = self
            .components
            .iter()
            .filter_map(|&i| {
                let cell = *self.get(i);
                let next = match cell.block {
                    BlockType::RedstoneTorch { attach, .. } => {
                        let lit = self.input_from(i, attach, true) == 0;
                        Cell {
                            block: BlockType::RedstoneTorch { lit, attach },
                            power: if lit { 15 } else { 0 },
                            ..cell
                        }
                    }
                    BlockType::Repeater {
                        delay_ticks,
                        facing,
                    } => self.next_repeater(i, cell, delay_ticks.max(1), facing),
                    BlockType::Comparator { mode, facing } => {
                        let back = self.comparator_back(i, facing);
                        let side = facing
                            .sides()
                            .into_iter()
                            .map(|d| self.side_input(i, d))
                            .max()
                            .unwrap_or(0);
                        let power = match mode {
                            ComparatorMode::Compare if back >= side => back,
                            ComparatorMode::Compare => 0,
                            ComparatorMode::Subtract => back.saturating_sub(side),
                        };
                        Cell { power, ..cell }
                    }
                    BlockType::Observer { facing } => {
                        let looks = self.next(i, facing).looks();
                        let changed = looks != cell.watched && self.tick > 1;
                        Cell {
                            power: if changed && cell.power == 0 { 15 } else { 0 },
                            watched: looks,
                            ..cell
                        }
                    }
                    BlockType::Piston {
                        facing, extended, ..
                    } => {
                        if self.piston_powered(i, facing) != extended {
                            pistons.push(i);
                        }
                        return None;
                    }
                    BlockType::Dropper { facing, triggered } => {
                        let powered = self.piston_powered(i, facing) || self.received(i, true) > 0;
                        let mut next = Cell {
                            block: BlockType::Dropper {
                                facing,
                                triggered: powered,
                            },
                            ..cell
                        };
                        if powered && !triggered {
                            next.timer = 2;
                        }
                        next
                    }
                    _ => return None,
                };
                (next.block != cell.block
                    || next.power != cell.power
                    || next.timer != cell.timer
                    || next.watched != cell.watched)
                    .then_some((i, next))
            })
            .collect();
        // Nested ALUs: their faces' power in, their chips run, their faces' levels out.
        let mut changes = changes;
        let mut touched = Vec::new();
        // Only those a read channel changed for, still settling, or reading their faces.
        self.ensure_buses();
        let mut run: Vec<u32> = std::mem::take(&mut self.dirty).into_iter().collect();
        run.extend(self.unstable.iter().copied());
        run.extend(
            self.buses
                .as_ref()
                .map(|b| b.always.as_slice())
                .unwrap_or_default()
                .iter()
                .copied(),
        );
        run.sort_unstable();
        run.dedup();
        for i in run {
            if self.get(i).block != BlockType::Alu {
                continue;
            }
            let Some(alu) = self.nested.get(&i) else {
                continue;
            };
            let runs = self
                .buses
                .as_ref()
                .and_then(|b| b.alu_runs.get(&i))
                .copied()
                .unwrap_or([None; 6]);
            // From a cable, its level; else the redstone into that face.
            let inputs = if alu.reads_faces {
                Facing::ALL.map(|f| match self.next(i, f).block {
                    BlockType::Cable => self.next(i, f).power,
                    _ => self.input_from(i, f, true),
                })
            } else {
                [0; 6]
            };
            let levels = self.buses.as_ref().map(|b| &b.levels);
            let alu = self.nested.get_mut(&i).expect("there");
            // The channels its board reads, into its own buffer.
            alu.read_now.clear();
            alu.read_now.extend(alu.reads.iter().map(|&(f, ch)| {
                runs[f as usize % 6]
                    .and_then(|r| levels.and_then(|l| l[r].get(ch as usize)).copied())
                    .unwrap_or(0)
            }));
            let outputs = alu.run(inputs, runs.map(|r| r.is_some()));
            let changed = std::mem::take(&mut alu.changed);
            for &(f, ch) in &changed {
                if let Some(r) = runs[f as usize % 6] {
                    self.update_channel(r, ch);
                }
            }
            if let Some(alu) = self.nested.get_mut(&i) {
                alu.changed = changed;
                alu.changed.clear();
            }
            if self.nested.get(&i).is_some_and(|a| a.stable) {
                self.unstable.remove(&i);
            } else {
                self.unstable.insert(i);
            }
            let packed = outputs
                .iter()
                .enumerate()
                .fold(0u32, |p, (k, &l)| p | u32::from(l.min(15)) << (4 * k));
            let cell = *self.get(i);
            if cell.watched != packed {
                let power = outputs.iter().copied().max().unwrap_or(0);
                changes.push((
                    i,
                    Cell {
                        watched: packed,
                        power,
                        ..cell
                    },
                ));
                self.alu_changes += 1;
                touched.push(i);
            }
        }
        for (i, next) in changes {
            *self.get_mut(i) = next;
        }
        for i in touched {
            self.touch_around(i);
        }
        for i in pistons {
            if let BlockType::Piston {
                facing,
                sticky,
                extended,
            } = self.get(i).block
            {
                if extended {
                    self.retract(i, facing, sticky);
                } else {
                    self.extend(i, facing, sticky);
                }
            }
        }
    }
}
