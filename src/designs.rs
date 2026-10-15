//! Ready-made designs the workbench can write onto a blank Blueprint: the
//! basic gates, latches and flip-flops. Each is a `.vdesign` file in
//! `designs/` (the format is in `designs/README.md`), drawn layer by layer.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::chip::{CircuitBlock, CircuitData, Port};
use crate::content::Tier;
use crate::vm::PortMode;

const FILES: [&str; 11] = [
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
];

/// A design, as drawn.
#[derive(Debug, Clone)]
pub struct Design {
    pub name: String,
    /// Each layer's rows, bottom up.
    layers: Vec<Vec<Vec<char>>>,
    ports: HashMap<char, (String, PortMode)>,
    delays: HashMap<(u32, u32, u32), u8>,
    unlit: Vec<(u32, u32, u32)>,
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
        let mut d = Design { name: String::new(), layers: Vec::new(), ports: HashMap::new(), delays: HashMap::new(), unlit: Vec::new() };
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
                Some("layer") => d.layers.push(Vec::new()),
                _ => d.layers.last_mut().ok_or("a row before any layer")?.push(line.chars().collect()),
            }
        }
        if d.name.is_empty() {
            return Err("no name".into());
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
            let mut b = Bench { vm: c.vm(), ports };
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
            self.vm.get_port_output(x, y, z) > 0
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
