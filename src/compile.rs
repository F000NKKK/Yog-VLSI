//! Chips run from tables: a circuit that answers its inputs the same way
//! every time is worked out once — every state it can be in, what each
//! input does to it — and then looked up instead of stepped.
//!
//! **Analysis.** A circuit qualifies when its next step is its cells'
//! business ([`RedstoneVM::tabulable`]). From the circuit as built, every
//! combination of its inputs (each off or on: 0 or 15) is applied and the
//! VM settled; where it ends up is a new state, explored the same way, until
//! no new state turns up. Each transition keeps how many steps it took and
//! the outputs after every one of them, and each state a snapshot of the VM.
//! The cost is forecast first and watched as it goes: past [`MAX_TRANSITIONS`]
//! or [`MAX_TIME`] (or a circuit that never settles — a clock) the circuit
//! stays redstone. It runs on a worker thread, once per circuit (its chip
//! id), however many chips run it.
//!
//! **Running.** [`ChipSim`] steps the VM until its table is ready and the
//! VM is at rest in one of its states; then it looks transitions up,
//! spending the steps they took — a chip is no faster than its redstone, a
//! transition cut short by the tick's budget shows the outputs of the step
//! it reached. Anything the table doesn't have (an input between off and
//! on, inputs changed in the middle of a transition) puts the VM back where
//! the table had it and steps on from there. Either way it's the same.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::vm::RedstoneVM;

/// The most transitions a table may have.
pub const MAX_TRANSITIONS: usize = 4096;
/// The most time working one out may take.
pub const MAX_TIME: Duration = Duration::from_millis(200);
/// Steps a transition may take: more, and it's a clock.
const MAX_STEPS: u32 = 1024;
const ON: u8 = 15;

/// One transition: where it ends, its steps, the outputs after each.
struct Move {
    next: usize,
    steps: u32,
    outputs: Vec<Vec<u8>>,
}

/// A circuit's table.
pub struct Table {
    /// Each state's VM, and its outputs.
    snapshots: Vec<RedstoneVM>,
    outputs: Vec<Vec<u8>>,
    /// The inputs each state was reached with (as a mask).
    inputs: Vec<u32>,
    /// What applying a mask in a state does: `moves[state][mask]`.
    moves: Vec<Vec<Move>>,
    /// Each state's fingerprint, to find a running VM's place in it.
    fingerprints: HashMap<u64, usize>,
}

enum Slot {
    Working,
    Ready(Arc<Table>),
    /// It stays redstone (and why).
    Redstone(#[allow(dead_code)] String),
}

static TABLES: LazyLock<Mutex<HashMap<String, Slot>>> = LazyLock::new(Default::default);

fn mask_of(levels: &[u8]) -> Option<u32> {
    let mut m = 0;
    for (i, &l) in levels.iter().enumerate() {
        match l {
            0 => {}
            ON => m |= 1 << i,
            _ => return None,
        }
    }
    Some(m)
}

fn read(vm: &RedstoneVM, at: &[(u32, u32, u32)]) -> Vec<u8> {
    at.iter().map(|&(x, y, z)| vm.get_port_output(x, y, z)).collect()
}

/// Works out the table of the circuit in `vm` (as built), or why not.
pub fn analyse(vm: &RedstoneVM, ins: &[(u32, u32, u32)], outs: &[(u32, u32, u32)]) -> Result<Table, String> {
    vm.tabulable().map_err(|why| format!("not a table: {why}"))?;
    if ins.len() > 12 {
        return Err(format!("{} inputs: 2^{} combinations a state", ins.len(), ins.len()));
    }
    let combos = 1usize << ins.len();
    let started = Instant::now();
    let mut t = Table { snapshots: vec![vm.clone()], outputs: vec![read(vm, outs)], inputs: vec![0], moves: Vec::new(), fingerprints: HashMap::new() };
    t.fingerprints.insert(vm.fingerprint(ins), 0);
    let mut s = 0;
    while s < t.snapshots.len() {
        let mut row = Vec::with_capacity(combos);
        for m in 0..combos as u32 {
            let mut v = t.snapshots[s].clone();
            for (i, &(x, y, z)) in ins.iter().enumerate() {
                v.set_port_input(x, y, z, if m >> i & 1 == 1 { ON } else { 0 });
            }
            let mut trail = Vec::new();
            let steps = v.settle_traced(MAX_STEPS, &mut |vm| trail.push(read(vm, outs)));
            if steps == MAX_STEPS {
                return Err("never settles (a clock?)".into());
            }
            let fp = v.fingerprint(ins);
            let next = match t.fingerprints.get(&fp) {
                Some(&n) => n,
                None => {
                    let n = t.snapshots.len();
                    t.fingerprints.insert(fp, n);
                    t.outputs.push(read(&v, outs));
                    t.inputs.push(m);
                    t.snapshots.push(v);
                    n
                }
            };
            row.push(Move { next, steps, outputs: trail });
        }
        t.moves.push(row);
        s += 1;
        // The forecast: the states known so far, each a row of `combos`.
        let done = t.moves.len() * combos;
        let total = t.snapshots.len() * combos;
        if total > MAX_TRANSITIONS {
            return Err(format!("over {MAX_TRANSITIONS} transitions ({} states so far × {combos})", t.snapshots.len()));
        }
        let forecast = started.elapsed().mul_f64(total as f64 / done as f64);
        if forecast > MAX_TIME {
            return Err(format!("would take about {forecast:?}"));
        }
    }
    Ok(t)
}

/// The table of circuit `key` if it's ready; starts working it out (on a
/// worker thread) the first time it's asked for.
fn table_for(key: &str, vm: &RedstoneVM, ins: &[(u32, u32, u32)], outs: &[(u32, u32, u32)]) -> Option<Arc<Table>> {
    let mut tables = TABLES.lock().unwrap();
    match tables.get(key) {
        Some(Slot::Ready(t)) => return Some(t.clone()),
        Some(_) => return None,
        None => {}
    }
    tables.insert(key.to_string(), Slot::Working);
    drop(tables);
    let (key, vm, ins, outs) = (key.to_string(), vm.clone(), ins.to_vec(), outs.to_vec());
    let work = move || {
        let started = Instant::now();
        let slot = match analyse(&vm, &ins, &outs) {
            Ok(t) => {
                yog_api::info!("[yog-vlsi] chip {key}: a table of {} states in {:?}", t.snapshots.len(), started.elapsed());
                Slot::Ready(Arc::new(t))
            }
            Err(why) => {
                yog_api::info!("[yog-vlsi] chip {key} runs as redstone: {why}");
                Slot::Redstone(why)
            }
        };
        let ready = match &slot {
            Slot::Ready(t) => Some(t.clone()),
            _ => None,
        };
        TABLES.lock().unwrap().insert(key, slot);
        ready
    };
    // Tests have it at once; the game on a worker thread, stepping meanwhile.
    if cfg!(test) {
        return work();
    }
    std::thread::spawn(work);
    None
}

/// Where a chip running from its table is.
#[derive(Clone)]
struct Place {
    table: Arc<Table>,
    state: usize,
    /// A transition under way: its mask and the steps done.
    moving: Option<(u32, u32)>,
}

/// A chip's circuit, stepped or looked up — the same either way.
#[derive(Clone)]
pub struct ChipSim {
    vm: RedstoneVM,
    ins: Vec<(u32, u32, u32)>,
    outs: Vec<(u32, u32, u32)>,
    key: Option<String>,
    /// Running from the table: where (the VM is then where it was left).
    place: Option<Place>,
    /// The inputs given since the last settle, and whether any changed.
    given: Vec<u8>,
    touched: bool,
}

impl ChipSim {
    /// `key`: what the circuit is known by, to share its table — `None`:
    /// stepped always.
    pub fn new(vm: RedstoneVM, ins: Vec<(u32, u32, u32)>, outs: Vec<(u32, u32, u32)>, key: Option<&str>) -> Self {
        let given = vec![0; ins.len()];
        ChipSim { vm, ins, outs, key: key.map(str::to_string), place: None, given, touched: false }
    }

    /// The VM, stepped from now on (its table's place made real).
    pub fn vm_mut(&mut self) -> &mut RedstoneVM {
        self.unpack();
        &mut self.vm
    }

    /// Back to stepping: the VM where the table has it.
    fn unpack(&mut self) {
        let Some(p) = self.place.take() else { return };
        let mut vm = p.table.snapshots[p.state].clone();
        if let Some((m, k)) = p.moving {
            for (i, &(x, y, z)) in self.ins.iter().enumerate() {
                vm.set_port_input(x, y, z, if m >> i & 1 == 1 { ON } else { 0 });
            }
            vm.settle(k);
        }
        // What was given since stays given.
        for (&(x, y, z), &l) in self.ins.iter().zip(&self.given) {
            vm.set_port_input(x, y, z, l);
        }
        self.vm = vm;
    }

    pub fn set_input(&mut self, x: u32, y: u32, z: u32, level: u8) {
        let Some(i) = self.ins.iter().position(|&p| p == (x, y, z)) else {
            self.vm_mut().set_port_input(x, y, z, level);
            return;
        };
        if self.place.is_none() {
            self.vm.set_port_input(x, y, z, level);
        }
        if self.given[i] != level {
            self.given[i] = level;
            self.touched = true;
        }
    }

    pub fn output(&self, x: u32, y: u32, z: u32) -> u8 {
        let Some(p) = &self.place else { return self.vm.get_port_output(x, y, z) };
        let Some(o) = self.outs.iter().position(|&q| q == (x, y, z)) else {
            return p.table.snapshots[p.state].get_port_output(x, y, z);
        };
        match p.moving {
            Some((m, k)) if k > 0 => p.table.moves[p.state][m as usize].outputs[k as usize - 1][o],
            _ => p.table.outputs[p.state][o],
        }
    }

    pub fn new_tick(&mut self) {
        if self.place.is_none() {
            self.vm.new_tick();
            self.adopt_table();
        }
    }

    /// [`settle`](Self::settle), stopping early at `deadline` while stepped.
    pub fn settle_until(&mut self, budget: u32, deadline: Instant) -> u32 {
        if self.place.is_none() {
            self.touched = false;
            return self.vm.settle_until(budget, deadline);
        }
        self.settle(budget)
    }

    /// Settles for up to `budget` steps; how many it took — as
    /// [`RedstoneVM::settle`].
    pub fn settle(&mut self, budget: u32) -> u32 {
        let touched = std::mem::take(&mut self.touched);
        let Some(p) = &mut self.place else { return self.vm.settle(budget) };
        let Some(m) = mask_of(&self.given) else {
            self.unpack();
            return self.vm.settle(budget);
        };
        let t = p.table.clone();
        let (total, done) = match p.moving {
            Some((mm, k)) if mm == m => (t.moves[p.state][m as usize].steps, k),
            // Inputs changed in the middle: step it.
            Some(_) => {
                self.unpack();
                return self.vm.settle(budget);
            }
            // Its own inputs again, but changed and changed back: one step to see.
            None if touched && m == t.inputs[p.state] && t.moves[p.state][m as usize].steps == 0 => {
                if budget == 0 {
                    p.moving = None;
                    return 0;
                }
                return 1;
            }
            None => (t.moves[p.state][m as usize].steps, 0),
        };
        let left = total - done;
        if left <= budget {
            p.state = t.moves[p.state][m as usize].next;
            p.moving = None;
            left
        } else {
            p.moving = Some((m, done + budget));
            budget
        }
    }

    /// Takes the table up if it's ready and the VM is at rest in one of its states.
    fn adopt_table(&mut self) {
        let Some(key) = &self.key else { return };
        let Some(t) = table_for(key, &self.vm, &self.ins, &self.outs) else { return };
        if self.touched {
            return;
        }
        if let Some(&state) = t.fingerprints.get(&self.vm.fingerprint(&self.ins)) {
            self.place = Some(Place { table: t, state, moving: None });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Tier;
    use crate::designs;

    /// Every ready-made design without ALUs, stepped and from its table side
    /// by side: the same outputs after every settle — on and off, levels in
    /// between, inputs changed mid-transition, budgets cut short.
    #[test]
    fn a_table_runs_exactly_as_the_redstone() {
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut rnd = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let mut tabled = 0;
        for d in designs::all() {
            let c = d.circuit(d.tier());
            if !c.alus.is_empty() {
                continue;
            }
            let ins: Vec<_> = c.ports.iter().filter(|p| p.dir.inputs()).map(|p| (p.x, p.y, p.z)).collect();
            let outs: Vec<_> = c.ports.iter().filter(|p| p.dir.outputs()).map(|p| (p.x, p.y, p.z)).collect();
            let mut real = ChipSim::new(c.vm(), ins.clone(), outs.clone(), None);
            let mut fast = ChipSim::new(c.vm(), ins.clone(), outs.clone(), Some(&format!("test:{}", d.name)));
            for t in 0..3000 {
                real.new_tick();
                fast.new_tick();
                for &(x, y, z) in &ins {
                    if rnd(3) == 0 {
                        let level = match rnd(10) {
                            0 => 7,
                            1..=4 => 0,
                            _ => 15,
                        };
                        real.set_input(x, y, z, level);
                        fast.set_input(x, y, z, level);
                    }
                }
                let budget = [0, 1, 2, 3, 8, 8, 8, 64][rnd(8) as usize] as u32;
                assert_eq!(real.settle(budget), fast.settle(budget), "{} at {t}: steps", d.name);
                for &(x, y, z) in &outs {
                    assert_eq!(real.output(x, y, z), fast.output(x, y, z), "{} at {t}: output", d.name);
                }
            }
            tabled += fast.place.is_some() as u32 + 1;
        }
        assert!(tabled > 0);
    }

    #[test]
    fn every_gate_gets_a_table() {
        // (Not the RS latch: S and R let go at once, it oscillates — as redstone.)
        for name in ["NOT", "AND", "OR", "XOR", "NAND", "NOR", "XNOR", "D latch", "D flip-flop", "T flip-flop"] {
            let d = designs::named(name).unwrap();
            let c = d.circuit(Tier::Wood);
            let ins: Vec<_> = c.ports.iter().filter(|p| p.dir.inputs()).map(|p| (p.x, p.y, p.z)).collect();
            let outs: Vec<_> = c.ports.iter().filter(|p| p.dir.outputs()).map(|p| (p.x, p.y, p.z)).collect();
            let t = analyse(&c.vm(), &ins, &outs).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(t.snapshots.len() < 64, "{name}: {} states", t.snapshots.len());
        }
    }
}
