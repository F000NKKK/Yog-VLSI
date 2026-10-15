//! Ready-made designs the workbench can write onto a blank Blueprint: the
//! basic gates, latches and flip-flops. Each is a `.vdesign` file in
//! `designs/` (the format is in `designs/README.md`), drawn layer by layer.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::chip::{ChipMeta, CircuitBlock, CircuitData, Port, SavedAlu};
use crate::content::Tier;
use crate::vm::{NestedPin, PortMode};

const FILES: [&str; 18] = [
    include_str!("designs/not.vdesign"),
    include_str!("designs/and.vdesign"),
    include_str!("designs/or.vdesign"),
    include_str!("designs/nand.vdesign"),
    include_str!("designs/nor.vdesign"),
    include_str!("designs/xor.vdesign"),
    include_str!("designs/xnor.vdesign"),
    include_str!("designs/rs_latch.vdesign"),
    include_str!("designs/d_latch.vdesign"),
    include_str!("designs/d_flip_flop.vdesign"),
    include_str!("designs/t_flip_flop.vdesign"),
    include_str!("designs/half_adder.vdesign"),
    include_str!("designs/full_adder.vdesign"),
    include_str!("designs/mux.vdesign"),
    include_str!("designs/demux.vdesign"),
    include_str!("designs/decoder.vdesign"),
    include_str!("designs/register_bit.vdesign"),
    include_str!("designs/memory_bit.vdesign"),
];

/// The chip id of a ready-made design's chip in a ready-made design's ALU:
/// its circuit is the design's ([`CircuitData::load_for_chip`]).
pub const CHIP_PREFIX: &str = "design:";

/// An ALU in a design: its tier, the designs in its slots, its board.
#[derive(Debug, Clone, Default)]
struct Alu {
    tier: Option<Tier>,
    chips: Vec<(u8, String)>,
    wires: Vec<(NestedPin, NestedPin)>,
}

/// A design, as drawn.
#[derive(Debug, Clone)]
pub struct Design {
    pub name: String,
    /// Each layer's rows, bottom up.
    layers: Vec<Vec<Vec<char>>>,
    ports: HashMap<char, (String, PortMode)>,
    delays: HashMap<(u32, u32, u32), u8>,
    unlit: Vec<(u32, u32, u32)>,
    alus: HashMap<char, Alu>,
}

/// A board pin: `in:W`, `out:E`, `bus:E:3` (channel from 1), `0:A` (slot: port).
fn pin(s: &str) -> Result<NestedPin, String> {
    let face = |f: &str| ["N", "S", "E", "W", "U", "D"].iter().position(|x| *x == f).map(|i| i as u8).ok_or(format!("pin {s}: face {f}"));
    let parts: Vec<&str> = s.split(':').collect();
    Ok(match parts[..] {
        ["in", f] => NestedPin::In(face(f)?),
        ["out", f] => NestedPin::Out(face(f)?),
        ["bus", f, ch] => NestedPin::Bus(face(f)?, ch.parse::<u16>().ok().and_then(|c| c.checked_sub(1)).ok_or(format!("pin {s}: channel"))?),
        [slot, label] => NestedPin::Chip(slot.parse().map_err(|_| format!("pin {s}: slot"))?, label.to_string()),
        _ => return Err(format!("pin {s}: in:<face>, out:<face>, bus:<face>:<channel> or <slot>:<port>")),
    })
}

static ALL: LazyLock<Vec<Design>> =
    LazyLock::new(|| FILES.iter().map(|f| Design::parse(f).unwrap_or_else(|e| panic!("design: {e}"))).collect());

/// Every design, in menu order.
pub fn all() -> &'static [Design] {
    &ALL
}

/// The design called `name`.
pub fn named(name: &str) -> Option<&'static Design> {
    all().iter().find(|d| d.name == name)
}

/// A horizontal direction from its char's arrow.
fn dir(c: char) -> Option<(i32, i32, &'static str)> {
    Some(match c {
        '^' | '↑' | '△' => (0, -1, "north"),
        '>' | '→' | '▷' => (1, 0, "east"),
        'v' | '↓' | '▽' => (0, 1, "south"),
        '<' | '←' | '◁' => (-1, 0, "west"),
        _ => return None,
    })
}

fn opposite(facing: &str) -> &'static str {
    match facing {
        "north" => "south",
        "south" => "north",
        "east" => "west",
        _ => "east",
    }
}

impl Design {
    pub fn parse(text: &str) -> Result<Design, String> {
        let mut d = Design {
            name: String::new(),
            layers: Vec::new(),
            ports: HashMap::new(),
            delays: HashMap::new(),
            unlit: Vec::new(),
            alus: HashMap::new(),
        };
        let ch = |w: Option<&str>| w.and_then(|w| w.chars().next()).ok_or_else(|| format!("{text:?}: no char"));
        let num = |s: Option<&str>| s.and_then(|s| s.parse::<u32>().ok()).ok_or_else(|| format!("bad number in {text:?}"));
        for line in text.lines().map(str::trim_end).filter(|l| !l.is_empty() && !l.starts_with("//")) {
            let mut words = line.split_whitespace();
            match words.next() {
                Some("name") => d.name = line["name".len()..].trim().to_string(),
                Some("port") => {
                    let ch = words.next().and_then(|w| w.chars().next()).ok_or("port: no char")?;
                    let label = words.next().ok_or("port: no label")?.to_string();
                    let mode = match words.next() {
                        Some("in") => PortMode::Input,
                        Some("out") => PortMode::Output,
                        Some("bidi") => PortMode::Bidirectional,
                        m => return Err(format!("port {label}: mode {m:?}")),
                    };
                    d.ports.insert(ch, (label, mode));
                }
                Some("delay") => {
                    let at = (num(words.next())?, num(words.next())?, num(words.next())?);
                    d.delays.insert(at, num(words.next())?.clamp(1, 4) as u8);
                }
                Some("unlit") => d.unlit.push((num(words.next())?, num(words.next())?, num(words.next())?)),
                Some("alu") => {
                    let c = ch(words.next())?;
                    let tier = words.next().and_then(Tier::from_id).ok_or(format!("alu {c}: tier"))?;
                    d.alus.entry(c).or_default().tier = Some(tier);
                }
                Some("chip") => {
                    let c = ch(words.next())?;
                    let slot = num(words.next())? as u8;
                    let name = words.collect::<Vec<_>>().join(" ");
                    d.alus.entry(c).or_default().chips.push((slot, name));
                }
                Some("wire") => {
                    let c = ch(words.next())?;
                    let from = pin(words.next().ok_or("wire: from")?)?;
                    let to = pin(words.next().ok_or("wire: to")?)?;
                    d.alus.entry(c).or_default().wires.push((from, to));
                }
                Some("layer") => d.layers.push(Vec::new()),
                _ => d.layers.last_mut().ok_or("a row before any layer")?.push(line.chars().collect()),
            }
        }
        if d.name.is_empty() {
            return Err("no name".into());
        }
        if let Some((c, _)) = d.alus.iter().find(|(_, a)| a.tier.is_none()) {
            return Err(format!("{}: alu {c} has no tier", d.name));
        }
        Ok(d)
    }

    /// The char at `(x, y, z)` (`.` off the drawing).
    fn at(&self, x: i32, y: i32, z: i32) -> char {
        if x < 0 || y < 0 || z < 0 {
            return '.';
        }
        let row = self.layers.get(y as usize).and_then(|l| l.get(z as usize));
        row.and_then(|r| r.get(x as usize)).copied().unwrap_or('.')
    }

    fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
        self.at(x, y, z) == '#'
    }

    /// What wire beside `(x, y, z)` turns toward, going `(dx, dz)`.
    fn connects(&self, x: i32, y: i32, z: i32, dx: i32, dz: i32) -> bool {
        match self.at(x + dx, y, z + dz) {
            '-' | 'T' | '^' | '>' | 'v' | '<' | '△' | '▷' | '▽' | '◁' => true,
            c if self.alus.contains_key(&c) => true,
            c @ ('↑' | '→' | '↓' | '←') => dir(c).is_some_and(|(rx, rz, _)| rx.abs() == dx.abs() && rz.abs() == dz.abs()),
            c => self.ports.contains_key(&c),
        }
    }

    /// A wire's state: which way it runs (side, or up a step), as placed.
    fn wire_state(&self, x: i32, y: i32, z: i32) -> String {
        let open_above = !self.is_solid(x, y + 1, z);
        let mut sides = [("north", 0, -1), ("east", 1, 0), ("south", 0, 1), ("west", -1, 0)].map(|(name, dx, dz)| {
            let how = if self.connects(x, y, z, dx, dz) {
                "side"
            } else if self.is_solid(x + dx, y, z + dz) && open_above && self.at(x + dx, y + 1, z + dz) == '-' {
                "up"
            } else if !self.is_solid(x + dx, y, z + dz) && self.at(x + dx, y - 1, z + dz) == '-' {
                "side"
            } else {
                "none"
            };
            (name, how)
        });
        // A wire going one way only runs on the other way too.
        let ways: Vec<usize> = (0..4).filter(|&i| sides[i].1 != "none").collect();
        if let [one] = ways[..] {
            sides[(one + 2) % 4].1 = "side";
        }
        let props: Vec<String> = sides.iter().map(|(n, h)| format!("{n}={h}")).collect();
        format!("minecraft:redstone_wire[{},power=0]", props.join(","))
    }

    /// The smallest tier its drawing fits.
    pub fn tier(&self) -> Tier {
        let width = self.layers.iter().flatten().map(Vec::len).max().unwrap_or(0) as u32;
        let depth = self.layers.iter().map(Vec::len).max().unwrap_or(0) as u32;
        let extent = width.max(depth).max(self.layers.len() as u32);
        Tier::ALL.into_iter().find(|t| t.world_size() >= extent).unwrap_or(Tier::Netherite)
    }

    /// The design as a circuit for a `tier` chip, in the middle of its world.
    pub fn circuit(&self, tier: Tier) -> CircuitData {
        let size = tier.world_size();
        let width = self.layers.iter().flatten().map(Vec::len).max().unwrap_or(0) as u32;
        let depth = self.layers.iter().map(Vec::len).max().unwrap_or(0) as u32;
        let (ox, oz) = (size.saturating_sub(width) / 2, size.saturating_sub(depth) / 2);
        let mut c = CircuitData::empty(tier);
        for (y, layer) in self.layers.iter().enumerate() {
            for (z, row) in layer.iter().enumerate() {
                for (x, &ch) in row.iter().enumerate() {
                    let (x, y, z) = (x as u32, y as u32, z as u32);
                    let at = (x + ox, y, z + oz);
                    let state = match ch {
                        '.' | ' ' => continue,
                        '#' => "minecraft:stone".to_string(),
                        '-' => self.wire_state(x as i32, y as i32, z as i32),
                        'T' => format!("minecraft:redstone_torch[lit={}]", !self.unlit.contains(&(x, y, z))),
                        '^' | '>' | 'v' | '<' => format!(
                            "minecraft:redstone_wall_torch[facing={},lit={}]",
                            dir(ch).map_or("north", |d| d.2),
                            !self.unlit.contains(&(x, y, z))
                        ),
                        '↑' | '→' | '↓' | '←' => format!(
                            "minecraft:repeater[delay={},facing={},locked=false,powered=false]",
                            self.delays.get(&(x, y, z)).copied().unwrap_or(1),
                            opposite(dir(ch).map_or("north", |d| d.2))
                        ),
                        '△' | '▷' | '▽' | '◁' => format!(
                            "minecraft:comparator[facing={},mode=subtract,powered=false]",
                            opposite(dir(ch).map_or("north", |d| d.2))
                        ),
                        a if self.alus.contains_key(&a) => {
                            let alu = &self.alus[&a];
                            let t = alu.tier.unwrap_or(Tier::Wood);
                            let chips = alu
                                .chips
                                .iter()
                                .filter_map(|(slot, name)| {
                                    let d = named(name)?;
                                    let inner = d.circuit(d.tier());
                                    Some((*slot, ChipMeta { id: format!("{CHIP_PREFIX}{name}"), tier: d.tier(), name: name.clone(), ports: inner.ports }))
                                })
                                .collect();
                            c.alus.push(SavedAlu { x: at.0, y: at.1, z: at.2, tier: t, chips, wires: alu.wires.clone() });
                            t.alu().to_string()
                        }
                        p => {
                            if let Some((label, mode)) = self.ports.get(&p) {
                                c.ports.push(Port { label: label.clone(), x: at.0, y: at.1, z: at.2, dir: *mode });
                            }
                            continue;
                        }
                    };
                    c.blocks.push(CircuitBlock { x: at.0, y: at.1, z: at.2, state, items: Vec::new(), redstone_port: None });
                }
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::RedstoneVM;

    /// A design running in a VM, its ports by label.
    struct Bench {
        vm: RedstoneVM,
        ports: HashMap<String, (u32, u32, u32)>,
    }

    impl Bench {
        fn new(name: &str) -> Bench {
            let c = named(name).unwrap_or_else(|| panic!("no design {name}")).circuit(Tier::Wood);
            let ports = c.ports.iter().map(|p| (p.label.clone(), (p.x, p.y, p.z))).collect();
            let vm = c.vm_with(&|id| CircuitData::load_for_chip("", id), 1);
            let mut b = Bench { vm, ports };
            b.run(4);
            b
        }

        /// Sets the inputs (`1` on) and runs `ticks` server ticks.
        fn set(&mut self, inputs: &[(&str, bool)], ticks: u32) -> &mut Self {
            for (label, on) in inputs {
                let (x, y, z) = self.ports[*label];
                self.vm.set_port_input(x, y, z, if *on { 15 } else { 0 });
            }
            self.run(ticks);
            self
        }

        fn run(&mut self, ticks: u32) {
            for _ in 0..ticks {
                self.vm.new_tick();
                self.vm.settle(Tier::Wood.steps_per_tick());
            }
        }

        fn out(&self, label: &str) -> bool {
            let (x, y, z) = self.ports[label];
            let level = self.vm.get_port_output(x, y, z);
            // Full levels only: a table takes a chip's inputs as off or on.
            assert!(level == 0 || level == 15, "{label} at {level}");
            level > 0
        }
    }

    fn truth_table(name: &str, f: fn(bool, bool) -> bool) {
        let mut b = Bench::new(name);
        for (a, bb) in [(false, false), (true, false), (false, true), (true, true), (false, false)] {
            b.set(&[("A", a), ("B", bb)], 3);
            assert_eq!(b.out("Q"), f(a, bb), "{name}({a}, {bb})");
        }
    }

    #[test]
    fn every_design_parses_and_fits_the_smallest_chip() {
        for d in all() {
            let c = d.circuit(Tier::Wood);
            assert!(!c.ports.is_empty(), "{}: no ports", d.name);
            assert!(c.blocks.iter().all(|b| b.x < 16 && b.y < 16 && b.z < 16), "{}", d.name);
            let labels: Vec<_> = c.ports.iter().map(|p| &p.label).collect();
            assert!(labels.iter().all(|l| labels.iter().filter(|m| m == &l).count() == 1), "{}: a label twice", d.name);
        }
    }

    /// In the editor a design's blocks are real: what needs a block under
    /// it (wire, repeaters, comparators, standing torches) has one.
    #[test]
    fn nothing_hangs_in_the_air() {
        for d in all() {
            for (y, layer) in d.layers.iter().enumerate().skip(1) {
                for (z, row) in layer.iter().enumerate() {
                    for (x, &c) in row.iter().enumerate() {
                        if matches!(c, '-' | 'T' | '↑' | '→' | '↓' | '←' | '△' | '▷' | '▽' | '◁') {
                            let below = d.at(x as i32, y as i32 - 1, z as i32);
                            assert!(below == '#' || d.alus.contains_key(&below), "{}: {c} at {x},{y},{z} on {below:?}", d.name);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn not() {
        let mut b = Bench::new("NOT");
        assert!(b.set(&[("A", false)], 2).out("Q"));
        assert!(!b.set(&[("A", true)], 2).out("Q"));
        assert!(b.set(&[("A", false)], 2).out("Q"));
    }

    #[test]
    fn two_input_gates() {
        truth_table("AND", |a, b| a && b);
        truth_table("OR", |a, b| a || b);
        truth_table("NAND", |a, b| !(a && b));
        truth_table("NOR", |a, b| !(a || b));
        truth_table("XOR", |a, b| a != b);
        truth_table("XNOR", |a, b| a == b);
    }

    #[test]
    fn rs_latch() {
        let mut b = Bench::new("RS latch");
        assert!(!b.out("Q") && b.out("NQ"), "starts reset");
        assert!(b.set(&[("S", true)], 3).out("Q"));
        assert!(b.set(&[("S", false)], 3).out("Q"), "holds set");
        assert!(!b.out("NQ"));
        assert!(!b.set(&[("R", true)], 3).out("Q"));
        assert!(!b.set(&[("R", false)], 3).out("Q"), "holds reset");
        assert!(b.out("NQ"));
    }

    #[test]
    fn d_latch() {
        let mut b = Bench::new("D latch");
        assert!(b.set(&[("E", true), ("D", true)], 3).out("Q"), "open: follows D");
        assert!(!b.set(&[("D", false)], 3).out("Q"));
        b.set(&[("D", true)], 3);
        assert!(b.set(&[("E", false)], 3).out("Q"));
        assert!(b.set(&[("D", false)], 3).out("Q"), "closed: holds");
        assert!(!b.set(&[("E", true)], 3).out("Q"));
    }

    #[test]
    fn d_flip_flop() {
        let mut b = Bench::new("D flip-flop");
        assert!(!b.set(&[("D", true)], 3).out("Q"), "no edge yet");
        assert!(b.set(&[("C", true)], 3).out("Q"), "takes D on the rising edge");
        assert!(b.set(&[("D", false)], 3).out("Q"), "holds while C is on");
        assert!(b.set(&[("C", false)], 3).out("Q"), "and when it goes off");
        assert!(!b.set(&[("C", true)], 3).out("Q"), "takes the new D");
    }

    #[test]
    fn adders() {
        let mut b = Bench::new("Half adder");
        for (x, y) in [(false, false), (true, false), (false, true), (true, true)] {
            b.set(&[("A", x), ("B", y)], 4);
            assert_eq!((b.out("S"), b.out("C")), (x != y, x && y), "half adder {x} {y}");
        }
        let mut b = Bench::new("Full adder");
        for n in 0..8 {
            let (x, y, c) = (n & 1 != 0, n & 2 != 0, n & 4 != 0);
            b.set(&[("A", x), ("B", y), ("CI", c)], 4);
            let sum = x as u8 + y as u8 + c as u8;
            assert_eq!((b.out("S"), b.out("CO")), (sum & 1 != 0, sum > 1), "full adder {x} {y} {c}");
        }
    }

    #[test]
    fn mux_demux_decoder() {
        let mut b = Bench::new("MUX 2:1");
        b.set(&[("A", true), ("B", false), ("S", false)], 4);
        assert!(b.out("Q"));
        assert!(!b.set(&[("S", true)], 4).out("Q"));
        assert!(b.set(&[("B", true)], 4).out("Q"));
        let mut b = Bench::new("DEMUX 1:2");
        b.set(&[("D", true), ("S", false)], 4);
        assert!(b.out("Q0") && !b.out("Q1"));
        b.set(&[("S", true)], 4);
        assert!(!b.out("Q0") && b.out("Q1"));
        let mut b = Bench::new("Decoder 2:4");
        for n in 0..4u8 {
            b.set(&[("A0", n & 1 != 0), ("A1", n & 2 != 0)], 4);
            let on: Vec<bool> = ["Y0", "Y1", "Y2", "Y3"].iter().map(|y| b.out(y)).collect();
            assert_eq!(on, (0..4).map(|i| i == n).collect::<Vec<_>>(), "decoder {n}");
        }
    }

    #[test]
    fn register_and_memory_bits() {
        let mut b = Bench::new("Register bit");
        b.set(&[("D", true), ("L", false)], 4);
        b.set(&[("C", true)], 4).set(&[("C", false)], 4);
        assert!(!b.out("Q"), "not loaded");
        b.set(&[("L", true)], 4);
        b.set(&[("C", true)], 4).set(&[("C", false)], 4);
        assert!(b.out("Q"), "loaded on the edge");
        b.set(&[("D", false), ("L", false)], 4);
        b.set(&[("C", true)], 4).set(&[("C", false)], 4);
        assert!(b.out("Q"), "kept");
        let mut m = Bench::new("Memory bit");
        m.set(&[("D", true), ("S", true), ("W", true)], 4).set(&[("W", false)], 4);
        assert!(m.out("Q"), "written");
        m.set(&[("D", false)], 4);
        assert!(m.out("Q"), "held without W");
        assert!(!m.set(&[("S", false)], 4).out("Q"), "hidden unselected");
        m.set(&[("W", true)], 4);
        assert!(m.set(&[("W", false), ("S", true)], 4).out("Q"), "not written unselected");
    }

    #[test]
    fn t_flip_flop() {
        let mut b = Bench::new("T flip-flop");
        let mut q = b.out("Q");
        for _ in 0..4 {
            b.set(&[("C", true)], 3);
            assert_ne!(b.out("Q"), q, "flips on the rising edge");
            q = b.out("Q");
            assert_eq!(b.set(&[("C", false)], 3).out("Q"), q, "not on the falling one");
        }
    }
}

#[cfg(test)]
mod export {
    /// Every ready-made design as a Blueprint's JSON (`OUT`), at its tier —
    /// for tools that put them in a save or a README.
    #[test]
    #[ignore]
    fn export_blueprints() {
        let Ok(out) = std::env::var("OUT") else { return };
        let all: Vec<_> = super::all()
            .iter()
            .map(|d| crate::chip::Blueprint { name: d.name.clone(), tier: d.tier(), circuit: d.circuit(d.tier()) })
            .collect();
        std::fs::write(out, serde_json::to_string(&all).unwrap()).unwrap();
    }
}
