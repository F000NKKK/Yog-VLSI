//! Nested ALUs: a board of chips inside a cell, run as part of the
//! circuit around it.

use super::{CellMap, FastMap, FastSet, PortMode, RedstoneVM};
use std::collections::HashMap;
use yog_api::Schema;

/// A pin of a nested ALU's board: a side as a source (`In`) or a sink
/// (`Out`), by its index in N S E W U D, a chip's port by its slot and
/// label, or a channel (0-based) of the bus on a side — read as a source,
/// driven as a sink.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Schema)]
pub enum NestedPin {
    In(u8),
    Out(u8),
    Chip { slot: u8, label: String },
    Bus { side: u8, channel: u16 },
}

/// A port of a chip in a nested ALU.
#[derive(Debug, Clone)]
pub struct NestedPort {
    pub label: String,
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub mode: PortMode,
}

/// A chip in a nested ALU: its ports, its circuit, and the steps its tier
/// gives it each server tick. However many steps the circuit around it
/// takes in a tick, it takes no more than its own — a slow chip stays slow
/// inside a fast one, a fast one isn't sped up by it.
#[derive(Clone)]
pub struct NestedChip {
    pub slot: u8,
    pub ports: Vec<NestedPort>,
    pub budget: u32,
    /// Its circuit — run step by step, or from its table once it has one.
    pub sim: crate::compile::ChipSim,
    /// Steps it has left this tick.
    pub(super) left: u32,
    /// Settled with these inputs: nothing to do until they change.
    pub(super) settled_on: Option<Vec<u8>>,
}

impl NestedChip {
    /// `key`: what its circuit is known by (its chip id): a circuit is
    /// analysed once, however many chips run it.
    pub fn new(slot: u8, ports: Vec<NestedPort>, budget: u32, vm: RedstoneVM, key: &str) -> Self {
        let ins = ports
            .iter()
            .filter(|p| p.mode.inputs())
            .map(|p| (p.x, p.y, p.z))
            .collect();
        let outs = ports
            .iter()
            .filter(|p| p.mode.outputs())
            .map(|p| (p.x, p.y, p.z))
            .collect();
        let sim = crate::compile::ChipSim::new(vm, ins, outs, Some(key));
        NestedChip {
            slot,
            ports,
            budget,
            sim,
            left: budget,
            settled_on: None,
        }
    }
}

/// A circuit's buses (digital cable runs).
#[derive(Default, Clone)]
pub(super) struct Buses {
    /// Each bus cell's run.
    pub(super) run: CellMap<usize>,
    /// Each ALU's faces' runs.
    pub(super) alu_runs: CellMap<[Option<usize>; 6]>,
    /// Each run's channels' levels.
    pub(super) levels: Vec<Vec<u8>>,
    /// Each run's channels' drivers (ALU, its face) and readers.
    pub(super) drivers: Vec<FastMap<u16, Vec<(u32, u8)>>>,
    pub(super) readers: Vec<FastMap<u16, Vec<u32>>>,
    /// ALUs reading their faces with redstone beside them: run every step.
    /// (Those with only ports and ALUs beside them run when those change.)
    pub(super) always: Vec<u32>,
    /// Lamps and output ports with only ALUs, ports and air beside them:
    /// looked at when those change, not every step.
    pub(super) fed: FastSet<u32>,
    /// The other readers, looked at every step.
    pub(super) every_step: Vec<u32>,
}

/// An ALU inside a circuit: its chips and the wires of its board, run each
/// step of the circuit around it — as the ALU it was in the world.
#[derive(Clone)]
pub struct NestedAlu {
    pub chips: Vec<NestedChip>,
    pub wires: Vec<(NestedPin, NestedPin)>,
    /// What it drives onto the buses on its faces: (face, channel) → level.
    pub(crate) drives: FastMap<(u8, u16), u8>,
    /// The bus channels its board reads, (face, channel).
    pub(super) reads: Vec<(u8, u16)>,
    /// Every pin its board drives.
    pub(super) sinks: Vec<NestedPin>,
    /// Its board reads its faces (`In`): they're looked at every step.
    pub(super) reads_faces: bool,
    /// Its board, numbered: every source a slot of `levels` (the six faces,
    /// the bus channels it reads, its chips' outputs), every sink the slot
    /// it reads from.
    pub(super) board: Board,
    pub(super) levels: Vec<u8>,
    pub(super) scratch: Vec<u8>,
    /// The chips to look at next run: what they read changed, or they
    /// haven't settled. (`levels` holds every chip's outputs from its first run.)
    pub(super) pending: Vec<bool>,
    pub(super) primed: bool,
    /// The bus channels it reads now (filled by the VM before a run).
    pub(super) read_now: Vec<u8>,
    /// The bus channels whose level it drives changed in its last run.
    pub(super) changed: Vec<(u8, u16)>,
    /// What it last ran on, and what came out: run again on the same, its
    /// chips settled, nothing would change.
    pub(super) last: Option<([u8; 6], [bool; 6])>,
    pub(super) last_reads: Vec<u8>,
    pub(super) out: [u8; 6],
    /// Its chips settled on what they get now.
    pub(super) stable: bool,
}

/// A nested ALU's board as indices into its `levels`.
#[derive(Clone, Default)]
pub(super) struct Board {
    /// Where each chip's outputs go, in its ports' order.
    pub(super) chip_outs: Vec<Vec<usize>>,
    /// Where each chip's inputs come from (`None`: nothing wired).
    pub(super) chip_ins: Vec<Vec<Option<usize>>>,
    pub(super) faces_out: [Option<usize>; 6],
    /// Each bus channel it drives and where from.
    pub(super) drives: Vec<((u8, u16), Option<usize>)>,
    pub(super) size: usize,
    /// The chips reading each slot.
    pub(super) readers: Vec<Vec<usize>>,
}

impl NestedAlu {
    pub fn new(chips: Vec<NestedChip>, wires: Vec<(NestedPin, NestedPin)>) -> Self {
        let mut reads: Vec<(u8, u16)> = wires
            .iter()
            .filter_map(|(f, _)| match *f {
                NestedPin::Bus {
                    side: face,
                    channel: ch,
                } => Some((face, ch)),
                _ => None,
            })
            .collect();
        reads.sort_unstable();
        reads.dedup();
        let sinks = wires.iter().map(|(_, t)| t.clone()).collect();
        let reads_faces = wires.iter().any(|(f, _)| matches!(f, NestedPin::In(_)));

        // Number the sources: faces, read channels, chip outputs.
        let mut at: HashMap<NestedPin, usize> = HashMap::new();
        for f in 0..6u8 {
            at.insert(NestedPin::In(f), f as usize);
        }
        for (k, &(f, ch)) in reads.iter().enumerate() {
            at.insert(
                NestedPin::Bus {
                    side: f,
                    channel: ch,
                },
                6 + k,
            );
        }
        let mut board = Board {
            size: 6 + reads.len(),
            ..Default::default()
        };
        for c in &chips {
            let outs = c
                .ports
                .iter()
                .filter(|p| p.mode.outputs())
                .map(|p| {
                    at.insert(
                        NestedPin::Chip {
                            slot: c.slot,
                            label: p.label.clone(),
                        },
                        board.size,
                    );
                    board.size += 1;
                    board.size - 1
                })
                .collect();
            board.chip_outs.push(outs);
        }
        let from: HashMap<&NestedPin, usize> = wires
            .iter()
            .filter_map(|(f, t)| Some((t, *at.get(f)?)))
            .collect();
        for c in &chips {
            board.chip_ins.push(
                c.ports
                    .iter()
                    .filter(|p| p.mode.inputs())
                    .map(|p| {
                        from.get(&NestedPin::Chip {
                            slot: c.slot,
                            label: p.label.clone(),
                        })
                        .copied()
                    })
                    .collect(),
            );
        }
        board.faces_out = std::array::from_fn(|f| from.get(&NestedPin::Out(f as u8)).copied());
        board.readers = vec![Vec::new(); board.size];
        for (ci, srcs) in board.chip_ins.iter().enumerate() {
            for &src in srcs.iter().flatten() {
                if !board.readers[src].contains(&ci) {
                    board.readers[src].push(ci);
                }
            }
        }
        board.drives = wires
            .iter()
            .filter_map(|(_, t)| match *t {
                NestedPin::Bus {
                    side: f,
                    channel: ch,
                } => Some(((f, ch), from.get(t).copied())),
                _ => None,
            })
            .collect();
        let levels = vec![0; board.size];
        let n_chips = chips.len();
        NestedAlu {
            chips,
            wires,
            drives: FastMap::default(),
            reads,
            sinks,
            reads_faces,
            board,
            pending: vec![true; n_chips],
            primed: false,
            levels,
            scratch: Vec::new(),
            read_now: Vec::new(),
            changed: Vec::new(),
            last: None,
            last_reads: Vec::new(),
            out: [0; 6],
            stable: false,
        }
    }

    /// One run: the faces' power and buses in, the chips settle, the
    /// faces' levels out (and what it drives onto the buses, in `drives`).
    pub(super) fn run(&mut self, inputs: [u8; 6], buses: [bool; 6]) -> [u8; 6] {
        if self.stable && self.last == Some((inputs, buses)) && self.last_reads == self.read_now {
            return self.out;
        }
        let b = &self.board;
        let lv = &mut self.levels;
        let pending = &mut self.pending;
        let get = |lv: &[u8], src: Option<usize>| src.map_or(0, |i| lv[i]);
        // A slot's new level: the chips reading it look again.
        let set = |lv: &mut [u8], pending: &mut [bool], slot: usize, level: u8| {
            if lv[slot] != level {
                lv[slot] = level;
                for &ci in &b.readers[slot] {
                    pending[ci] = true;
                }
            }
        };
        if !self.primed {
            for (c, outs) in self.chips.iter().zip(&b.chip_outs) {
                for (k, &i) in outs.iter().enumerate() {
                    lv[i] = c.sim.output_at(k);
                }
            }
            self.primed = true;
        }
        for (f, &l) in inputs.iter().enumerate() {
            set(lv, pending, f, l);
        }
        for (k, &l) in self.read_now.iter().enumerate() {
            set(lv, pending, 6 + k, l);
        }
        // Only the chips whose inputs changed or that haven't settled —
        // reading what the others put out as it was.
        let ins = &mut self.scratch;
        let mut stepped = Vec::new();
        for (ci, (c, srcs)) in self.chips.iter_mut().zip(&b.chip_ins).enumerate() {
            if !pending[ci] {
                continue;
            }
            pending[ci] = false;
            ins.clear();
            ins.extend(srcs.iter().map(|&s| get(lv, s)));
            if c.settled_on.as_deref() == Some(&ins[..]) {
                continue;
            }
            for (k, &level) in ins.iter().enumerate() {
                c.sim.set_input_at(k, level);
            }
            // Out of steps this tick: it waits for the next, as it would in the world.
            if c.left == 0 {
                pending[ci] = true;
                continue;
            }
            let used = c.sim.settle(c.left);
            let settled = used < c.left;
            c.left -= used;
            c.settled_on = settled.then(|| ins.clone());
            if !settled {
                pending[ci] = true;
            }
            stepped.push(ci);
        }
        // What the chips that stepped put out now.
        for ci in stepped {
            let c = &self.chips[ci];
            for (k, &i) in b.chip_outs[ci].iter().enumerate() {
                set(lv, pending, i, c.sim.output_at(k));
            }
        }
        // Onto a bus only where there is one; what changed, noted.
        for &((f, ch), src) in &b.drives {
            let now = buses[f as usize % 6].then(|| get(lv, src));
            let was = self.drives.get(&(f, ch)).copied();
            if now != was {
                match now {
                    Some(l) => self.drives.insert((f, ch), l),
                    None => self.drives.remove(&(f, ch)),
                };
                self.changed.push((f, ch));
            }
        }
        // Settled: no chip left to look at.
        self.stable = !self.pending.iter().any(|&p| p);
        self.out = std::array::from_fn(|f| get(lv, b.faces_out[f]));
        self.last = Some((inputs, buses));
        self.last_reads.clone_from(&self.read_now);
        self.out
    }
}
