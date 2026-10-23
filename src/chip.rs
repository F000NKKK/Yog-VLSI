//! Microchip data: the metadata a chip item carries in its NBT and the
//! circuit (block grid + ports) kept in server storage.

use yog_api::Schema;

use crate::content::Blocks;
use crate::content::Tier;
use crate::nbt;
use crate::port;
use crate::res::Strings;
use crate::vm::{
    BlockType, ComparatorMode, Facing, MAX_POWER, NestedAlu, NestedChip, NestedPin, NestedPort,
    PortMode, RedstoneVM,
};

/// NBT tag of a fabricated chip's [`ChipMeta`].
const CHIP_TAG: &str = "YogVlsiChip";
/// NBT tag of a written blueprint's [`CircuitData`].
const BLUEPRINT_TAG: &str = "YogVlsiBlueprint";
/// Storage key prefix of fabricated chips' circuits.
const CIRCUIT_KEY: &str = "circuit/";

/// Unique id of a chip or design (UUID v4).
pub type ChipId = String;

pub fn new_id() -> ChipId {
    uuid::Uuid::new_v4().to_string()
}

impl PortMode {
    /// Localized long name (`Input`, …).
    pub fn label(self) -> String {
        match self {
            PortMode::Input => Strings::port_mode_input(),
            PortMode::Output => Strings::port_mode_output(),
            PortMode::Bidirectional => Strings::port_mode_bidi(),
        }
    }

    /// The color its pins are drawn in.
    pub fn color(self) -> u32 {
        match self {
            PortMode::Input => crate::res::Colors::PIN_INPUT,
            PortMode::Output => crate::res::Colors::PIN_OUTPUT,
            PortMode::Bidirectional => crate::res::Colors::PIN_BIDI,
        }
    }

    /// Drives a value out of the chip.
    pub fn outputs(self) -> bool {
        matches!(self, PortMode::Output | PortMode::Bidirectional)
    }

    /// Takes a value into the chip.
    pub fn inputs(self) -> bool {
        matches!(self, PortMode::Input | PortMode::Bidirectional)
    }
}

/// A named I/O point inside the chip's virtual world (DESIGN.md §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Schema)]
pub struct Port {
    pub label: String,
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub dir: PortMode,
}

/// What a fabricated chip item carries in its NBT.
#[derive(Debug, Clone, PartialEq, Eq, Schema)]
pub struct ChipMeta {
    /// Key of the circuit in server storage.
    pub id: ChipId,
    pub tier: Tier,
    pub name: String,
    pub ports: Vec<Port>,
}

impl ChipMeta {
    pub fn to_nbt(&self) -> String {
        nbt::wrap_data(CHIP_TAG, self)
    }

    pub fn from_nbt(snbt: &str) -> Option<Self> {
        nbt::read_data(snbt, CHIP_TAG)
    }
}

/// One block of the virtual world.
#[derive(Debug, Clone, PartialEq, Eq, Schema)]
pub struct CircuitBlock {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    /// Full block state, `minecraft:repeater[delay=2,facing=north,…]`.
    pub state: String,
    /// A container's items: (slot, item id, count).
    #[data(default)]
    pub items: Vec<(u8, String, u8)>,
    /// A Redstone Port's mode and name.
    #[data(default)]
    pub redstone_port: Option<crate::cables::RedstonePort>,
}

/// How many slots a container block has (0: not a container).
pub fn container_slots(id: &str) -> u8 {
    let path = yog_api::yog_ids::game_path(id).unwrap_or(id);
    match path {
        "chest" | "trapped_chest" | "barrel" => 27,
        p if p.ends_with("shulker_box") => 27,
        "hopper" | "brewing_stand" => 5,
        "dropper" | "dispenser" => 9,
        "furnace" | "blast_furnace" | "smoker" => 3,
        _ => 0,
    }
}

impl CircuitBlock {
    /// The block id, without its properties.
    pub fn id(&self) -> &str {
        self.state.split_once('[').map_or(&self.state, |(id, _)| id)
    }

    /// One property of the state (`facing` → `north`).
    pub fn prop(&self, key: &str) -> Option<&str> {
        let (_, props) = self.state.split_once('[')?;
        props
            .trim_end_matches(']')
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| k.trim() == key)
            .map(|(_, v)| v.trim())
    }

    fn is(&self, key: &str, value: &str) -> bool {
        self.prop(key) == Some(value)
    }

    /// The block a lever or button is mounted on, from its `face` and
    /// `facing`.
    fn mount(&self) -> Facing {
        match self.prop("face") {
            Some("floor") => Facing::Down,
            Some("ceiling") => Facing::Up,
            _ => self.facing().opposite(),
        }
    }

    fn facing(&self) -> Facing {
        self.prop("facing")
            .map(Facing::from_minecraft)
            .unwrap_or(Facing::North)
    }

    fn power_prop(&self) -> u8 {
        self.prop("power").and_then(|p| p.parse().ok()).unwrap_or(0)
    }

    /// What the VM simulates for this block: the redstone parts as they
    /// are, full blocks as conductors, everything else as air-like blocks
    /// that cut no wire and carry no power.
    pub fn block_type(&self) -> BlockType {
        let id = self.id();
        let path = yog_api::yog_ids::game_path(id).unwrap_or(id);
        self.own_block(id)
            .or_else(|| self.redstone_part(path))
            .or_else(|| self.moving_part(path))
            .or_else(|| self.container(id, path))
            .unwrap_or_else(|| shape(id, path))
    }

    /// This mod's blocks: cables and ports.
    fn own_block(&self, id: &str) -> Option<BlockType> {
        Some(match id {
            Blocks::ANALOG_CABLE => BlockType::Cable,
            Blocks::DIGITAL_CABLE => BlockType::Bus,
            Blocks::REDSTONE_PORT => BlockType::RedstonePort {
                output: self.redstone_port.as_ref().is_some_and(|p| p.output),
            },
            _ => return None,
        })
    }

    /// Wire, torches, repeaters, comparators, lamps and what gives power.
    fn redstone_part(&self, path: &str) -> Option<BlockType> {
        let powered = self.is("powered", "true");
        let lit = !self.is("lit", "false");
        Some(match path {
            "redstone_wire" => BlockType::RedstoneWire,
            "redstone_torch" => BlockType::RedstoneTorch {
                lit,
                attach: Facing::Down,
            },
            "redstone_wall_torch" => BlockType::RedstoneTorch {
                lit,
                attach: self.facing().opposite(),
            },
            "repeater" => BlockType::Repeater {
                delay_ticks: self.prop("delay").and_then(|d| d.parse().ok()).unwrap_or(1),
                facing: self.facing(),
            },
            "comparator" => BlockType::Comparator {
                mode: if self.is("mode", "subtract") {
                    ComparatorMode::Subtract
                } else {
                    ComparatorMode::Compare
                },
                facing: self.facing(),
            },
            "redstone_lamp" => BlockType::RedstoneLamp,
            "redstone_block" => BlockType::RedstoneBlock,
            "lever" => BlockType::Lever {
                on: powered,
                attach: self.mount(),
            },
            p if p.ends_with("_button") => BlockType::Button {
                pressed: powered,
                attach: self.mount(),
            },
            p if p.ends_with("weighted_pressure_plate") => BlockType::PressurePlate {
                power: self.power_prop(),
            },
            p if p.ends_with("_pressure_plate") => BlockType::PressurePlate {
                power: if powered { MAX_POWER } else { 0 },
            },
            "observer" => BlockType::Observer {
                facing: self.facing(),
            },
            "target" => BlockType::TargetBlock {
                power: self.power_prop(),
            },
            "detector_rail" => BlockType::Source {
                power: if powered { MAX_POWER } else { 0 },
            },
            "daylight_detector" => BlockType::Source {
                power: self.power_prop(),
            },
            _ => return None,
        })
    }

    /// Pistons and what they push, drag or can't move.
    fn moving_part(&self, path: &str) -> Option<BlockType> {
        Some(match path {
            "air" | "cave_air" | "void_air" | "moving_piston" => BlockType::Air,
            "piston" | "sticky_piston" => BlockType::Piston {
                facing: self.facing(),
                sticky: path == "sticky_piston",
                extended: self.is("extended", "true"),
            },
            "piston_head" => BlockType::PistonHead {
                facing: self.facing(),
                sticky: self.is("type", "sticky"),
            },
            "slime_block" => BlockType::Slime,
            "honey_block" => BlockType::Honey,
            "obsidian"
            | "crying_obsidian"
            | "bedrock"
            | "barrier"
            | "reinforced_deepslate"
            | "respawn_anchor"
            | "enchanting_table"
            | "ender_chest"
            | "end_portal_frame" => BlockType::Immovable,
            _ => return None,
        })
    }

    /// Hoppers, droppers and containers (a double chest's other half).
    fn container(&self, id: &str, path: &str) -> Option<BlockType> {
        Some(match path {
            "hopper" => BlockType::Hopper {
                facing: self.facing(),
            },
            "dropper" | "dispenser" => BlockType::Dropper {
                facing: self.facing(),
                triggered: self.is("triggered", "true"),
            },
            "chest" | "trapped_chest" => BlockType::Container {
                slots: container_slots(id),
                solid: false,
                partner: match self.prop("type") {
                    Some("left") => Some(self.facing().clockwise()),
                    Some("right") => Some(self.facing().clockwise().opposite()),
                    _ => None,
                },
            },
            p if container_slots(id) > 0 => BlockType::Container {
                slots: container_slots(id),
                solid: !p.ends_with("shulker_box") && p != "brewing_stand",
                partner: None,
            },
            _ => return None,
        })
    }
}

/// The rest by shape: see-through and partial blocks carry no power, an
/// ALU and a chip's port are themselves, full blocks conduct.
fn shape(id: &str, path: &str) -> BlockType {
    const SEE_THROUGH: [&str; 9] = [
        "glass", "slab", "stairs", "leaves", "fence", "wall", "door", "rail", "carpet",
    ];
    if SEE_THROUGH.iter().any(|p| path.contains(p)) {
        BlockType::Transparent
    } else if Tier::from_alu(id).is_some() {
        BlockType::Alu
    } else {
        port::dir_for_block(id).map_or(BlockType::Solid, BlockType::Port)
    }
}

/// A chip's whole circuit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Schema)]
pub struct CircuitData {
    /// Edge length of the virtual world (the tier's).
    pub size: u32,
    pub blocks: Vec<CircuitBlock>,
    pub ports: Vec<Port>,
    /// The ALUs in it, with their chips and boards: they run inside it.
    #[data(default)]
    pub alus: Vec<SavedAlu>,
}

/// How deep ALUs nest: an ALU in a chip in an ALU in a chip … stops here.
pub const MAX_NESTING: u32 = 4;

/// An ALU in a circuit, as it was in the editor: where, its tier, the chips
/// in its slots and its board's wires (between its sides and its chips' pins).
#[derive(Debug, Clone, PartialEq, Eq, Schema)]
pub struct SavedAlu {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub tier: Tier,
    pub chips: Vec<(u8, ChipMeta)>,
    pub wires: Vec<(NestedPin, NestedPin)>,
}

impl CircuitData {
    pub fn empty(tier: Tier) -> Self {
        CircuitData {
            size: tier.world_size(),
            ..Default::default()
        }
    }

    /// Places the circuit — its blocks, their items and its ports — into
    /// `vm`.
    pub fn load_into(&self, vm: &mut RedstoneVM) {
        for block in &self.blocks {
            vm.set_block(block.x, block.y, block.z, block.block_type());
            if !block.items.is_empty() {
                vm.set_items(block.x, block.y, block.z, &block.items);
            }
        }
        for port in &self.ports {
            vm.set_block(port.x, port.y, port.z, BlockType::Port(port.dir));
        }
    }

    /// A VM running this circuit.
    pub fn vm(&self) -> RedstoneVM {
        let mut vm = RedstoneVM::new(self.size);
        self.load_into(&mut vm);
        vm
    }

    /// [`Self::vm`] with its ALUs running inside it: their chips' circuits
    /// from `load` (by chip id), nested down to [`MAX_NESTING`] (`depth`:
    /// how deep this circuit already is). An ALU's tier powers its first
    /// slots, as in the world; each chip keeps its own tier's speed.
    pub fn vm_with(&self, load: &dyn Fn(&str) -> Option<CircuitData>, depth: u32) -> RedstoneVM {
        let mut vm = self.vm();
        if depth >= MAX_NESTING {
            return vm;
        }
        for a in &self.alus {
            let chips = a
                .chips
                .iter()
                .filter(|(slot, _)| (*slot as usize) < a.tier.max_chips())
                .filter_map(|(slot, meta)| {
                    // Its ports where its circuit has them now (a copy on an
                    // item may be older).
                    let circuit = load(&meta.id)?;
                    let ports = circuit
                        .ports
                        .iter()
                        .map(|p| NestedPort {
                            label: p.label.clone(),
                            x: p.x,
                            y: p.y,
                            z: p.z,
                            mode: p.dir,
                        })
                        .collect();
                    Some(NestedChip::new(
                        *slot,
                        ports,
                        meta.tier.steps_per_tick(),
                        circuit.vm_with(load, depth + 1),
                        &meta.id,
                    ))
                })
                .collect();
            vm.set_nested(a.x, a.y, a.z, NestedAlu::new(chips, a.wires.clone()));
        }
        vm
    }

    /// Saves a fabricated chip's circuit (chips can travel between worlds,
    /// so these live in the game's store).
    pub fn save_for_chip(&self, chip: &str) {
        let store = yog_api::storage::game();
        store.set(&format!("{CIRCUIT_KEY}{chip}"), self);
        if let Err(e) = store.flush() {
            yog_api::warn!("[yog-vlsi] saving chip circuit {chip}: {e}");
        }
    }

    pub fn load_for_chip(chip: &str) -> Option<Self> {
        // A ready-made design's chip (in a ready-made design's ALU).
        if let Some(name) = chip.strip_prefix(crate::designs::CHIP_PREFIX) {
            return crate::designs::named(name).map(|d| d.circuit(d.tier()));
        }
        yog_api::storage::game().get(&format!("{CIRCUIT_KEY}{chip}"))
    }
}

/// A design, written on a Blueprint item (DESIGN.md §2.4): what the
/// workbench edits in the editor and fabricates chips from.
#[derive(Debug, Clone, PartialEq, Schema)]
pub struct Blueprint {
    pub name: String,
    pub tier: Tier,
    pub circuit: CircuitData,
}

impl Blueprint {
    /// A new, empty design.
    pub fn new(name: impl Into<String>, tier: Tier) -> Self {
        Blueprint {
            name: name.into(),
            tier,
            circuit: CircuitData::empty(tier),
        }
    }

    /// Its NBT: the design as Yog Data (`nbt::wrap_data`).
    pub fn to_nbt(&self) -> String {
        nbt::wrap_data(BLUEPRINT_TAG, self)
    }

    /// The design on a Blueprint's NBT; `None` for a blank one.
    pub fn from_nbt(snbt: &str) -> Option<Self> {
        nbt::read_data(snbt, BLUEPRINT_TAG)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> ChipMeta {
        ChipMeta {
            id: "abc".into(),
            tier: Tier::Gold,
            name: "Adder \"v2\"".into(),
            ports: vec![Port {
                label: "A0".into(),
                x: 1,
                y: 0,
                z: 2,
                dir: PortMode::Input,
            }],
        }
    }

    #[test]
    fn chip_meta_nbt_round_trip() {
        let m = meta();
        assert_eq!(ChipMeta::from_nbt(&m.to_nbt()), Some(m));
        assert_eq!(ChipMeta::from_nbt("{}"), None);
    }

    #[test]
    fn blueprint_nbt_round_trip() {
        let c = CircuitData {
            size: 16,
            blocks: vec![CircuitBlock {
                x: 0,
                y: 0,
                z: 0,
                state: "minecraft:repeater[delay=2]".into(),
                items: Vec::new(),
                redstone_port: None,
            }],
            ports: meta().ports,
            alus: Vec::new(),
        };
        let bp = Blueprint {
            name: "Adder".into(),
            tier: Tier::Iron,
            circuit: c,
        };
        assert_eq!(Blueprint::from_nbt(&bp.to_nbt()), Some(bp));
    }

    #[test]
    fn block_states_parse() {
        let b = |state: &str| CircuitBlock {
            x: 0,
            y: 0,
            z: 0,
            state: state.into(),
            items: Vec::new(),
            redstone_port: None,
        };
        assert_eq!(
            b("minecraft:repeater[delay=3,facing=east,locked=false]").block_type(),
            BlockType::Repeater {
                delay_ticks: 3,
                facing: Facing::East
            }
        );
        assert_eq!(
            b("minecraft:lever[face=wall,facing=east,powered=true]").block_type(),
            BlockType::Lever {
                on: true,
                attach: Facing::West
            }
        );
        assert_eq!(
            b("minecraft:lever[face=floor,facing=east,powered=false]").block_type(),
            BlockType::Lever {
                on: false,
                attach: Facing::Down
            }
        );
        assert_eq!(
            b("minecraft:redstone_wall_torch[facing=north,lit=true]").block_type(),
            BlockType::RedstoneTorch {
                lit: true,
                attach: Facing::South
            }
        );
        assert_eq!(
            b("minecraft:polished_blackstone_button[face=ceiling,facing=north,powered=false]")
                .block_type(),
            BlockType::Button {
                pressed: false,
                attach: Facing::Up
            }
        );
        assert_eq!(b("minecraft:glass").block_type(), BlockType::Transparent);
        assert_eq!(
            b("minecraft:lime_shulker_box").block_type(),
            BlockType::Container {
                slots: 27,
                solid: false,
                partner: None
            }
        );
        assert_eq!(
            b("minecraft:chest[facing=north,type=left,waterlogged=false]").block_type(),
            BlockType::Container {
                slots: 27,
                solid: false,
                partner: Some(Facing::East)
            }
        );
        assert_eq!(
            b("minecraft:sticky_piston[extended=false,facing=up]").block_type(),
            BlockType::Piston {
                facing: Facing::Up,
                sticky: true,
                extended: false
            }
        );
        assert_eq!(b("minecraft:obsidian").block_type(), BlockType::Immovable);
        assert_eq!(b("minecraft:stone").block_type(), BlockType::Solid);
        assert_eq!(b("minecraft:stone").block_type(), BlockType::Solid);
        assert_eq!(
            b(port::PORT_OUTPUT).block_type(),
            BlockType::Port(PortMode::Output)
        );
    }

    #[test]
    fn a_bidirectional_port_goes_both_ways() {
        assert!(PortMode::Bidirectional.inputs() && PortMode::Bidirectional.outputs());
    }

    /// Runs a chip's circuit from a file (`CHIP=…`) for `TICKS` server
    /// ticks, `PRESS=<port>:<from tick>:<to tick>,…` held, printing its
    /// PC, A and 8×8 screen ports (`PXrc`) and how long a tick took.
    #[test]
    #[ignore]
    fn run_a_chip_file() {
        let Ok(path) = std::env::var("CHIP") else {
            return;
        };
        let c: CircuitData = yog_api::yog_data::from_text(&std::fs::read_to_string(path).unwrap())
            .unwrap()
            .value;
        let ticks: u32 = std::env::var("TICKS")
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or(100);
        let press: Vec<(String, u32, u32)> = std::env::var("PRESS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|p| {
                let mut f = p.split(':');
                Some((
                    f.next()?.to_string(),
                    f.next()?.parse().ok()?,
                    f.next()?.parse().ok()?,
                ))
            })
            .collect();
        let started = std::time::Instant::now();
        let mut vm = c.vm_with(&|id| CircuitData::load_for_chip(id), 1);
        println!("built in {:?}", started.elapsed());
        let port = |l: &str| {
            c.ports
                .iter()
                .find(|p| p.label == l)
                .map(|p| (p.x, p.y, p.z))
        };
        let word = |vm: &RedstoneVM, prefix: &str| {
            (0..8)
                .filter_map(|i| {
                    port(&format!("{prefix}{i}"))
                        .map(|(x, y, z)| ((vm.get_port_output(x, y, z) > 0) as u32) << i)
                })
                .sum::<u32>()
        };
        let started = std::time::Instant::now();
        // With a Display port: a Display's side of it, one write a tick.
        let (mut toggle, mut writes) = (false, Vec::<(u32, u32)>::new());
        for t in 0..ticks {
            if let Some((x, y, z)) = port("ACK") {
                vm.set_port_input(x, y, z, if toggle { 15 } else { 0 });
            }
            for (label, from, to) in &press {
                if let Some((x, y, z)) = port(label) {
                    vm.set_port_input(x, y, z, if (*from..*to).contains(&t) { 15 } else { 0 });
                }
            }
            vm.new_tick();
            vm.settle(
                std::env::var("STEPS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(Tier::Netherite.steps_per_tick()),
            );
            if let Some((x, y, z)) = port("T")
                && (vm.get_port_output(x, y, z) > 0) != toggle
            {
                toggle = !toggle;
                writes.push((word(&vm, "R"), word(&vm, "D")));
            }
            if std::env::var("TRACE").is_ok() {
                println!("T {} {}", word(&vm, "PC"), word(&vm, "A"));
            } else if t % 10 == 9 || t + 1 == ticks {
                println!(
                    "tick {t}: PC {} A {} ({:?}/tick)",
                    word(&vm, "PC"),
                    word(&vm, "A"),
                    started.elapsed() / (t + 1)
                );
            }
        }
        if port("T").is_some() {
            println!("{} writes to the Display", writes.len());
            if let Ok(out) = std::env::var("WRITES") {
                std::fs::write(out, yog_api::yog_data::to_text(&writes)).unwrap();
            }
            return;
        }
        for r in 0..8 {
            let row: String = (0..8)
                .map(|c| match port(&format!("PX{r}{c}")) {
                    Some((x, y, z)) if vm.get_port_output(x, y, z) > 0 => '#',
                    _ => '.',
                })
                .collect();
            println!("{row}");
        }
    }

    #[test]
    fn a_big_design_is_packed_and_read_back() {
        let block = |x| CircuitBlock {
            x,
            y: 0,
            z: 0,
            state: "minecraft:redstone_wire".into(),
            items: Vec::new(),
            redstone_port: None,
        };
        let bp = Blueprint {
            name: "Big".into(),
            tier: Tier::Netherite,
            circuit: CircuitData {
                size: 256,
                blocks: (0..4000).map(block).collect(),
                ..Default::default()
            },
        };
        let nbt = bp.to_nbt();
        assert!(nbt.len() < 60_000, "{}", nbt.len());
        assert!(nbt.contains(nbt::PACKED));
        assert_eq!(Blueprint::from_nbt(&nbt), Some(bp));
        let small = Blueprint::new("Small", Tier::Wood);
        assert!(!small.to_nbt().contains(nbt::PACKED));
        assert_eq!(Blueprint::from_nbt(&small.to_nbt()), Some(small));
    }
}
