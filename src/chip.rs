//! Microchip data: the metadata a chip item carries in its NBT and the
//! circuit (block grid + ports) kept in server storage.

use serde::{Deserialize, Serialize};
use yog_api::Storage;

use crate::nbt;
use crate::res::Strings;
use crate::tier::Tier;
use crate::port;
use crate::vm::{BlockType, ComparatorMode, Facing, NestedAlu, NestedChip, NestedPin, NestedPort, PortMode, RedstoneVM};
use crate::MOD_ID;
use crate::content::Blocks;

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub label: String,
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub dir: PortMode,
}

/// What a fabricated chip item carries in its NBT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChipMeta {
    /// Key of the circuit in server storage.
    pub id: ChipId,
    pub tier: Tier,
    pub name: String,
    pub ports: Vec<Port>,
}

impl ChipMeta {
    pub fn to_nbt(&self) -> String {
        nbt::wrap(CHIP_TAG, &serde_json::to_string(self).unwrap_or_default())
    }

    pub fn from_nbt(snbt: &str) -> Option<Self> {
        serde_json::from_str(&nbt::read(snbt, CHIP_TAG)?).ok()
    }
}

/// One block of the virtual world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CircuitBlock {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    /// Full block state, `minecraft:repeater[delay=2,facing=north,…]`.
    pub state: String,
    /// A container's items: (slot, item id, count).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<(u8, String, u8)>,
    /// A Redstone Port's mode and name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redstone_port: Option<crate::cables::RedstonePort>,
}

/// How many slots a container block has (0: not a container).
pub fn container_slots(id: &str) -> u8 {
    let path = id.strip_prefix("minecraft:").unwrap_or(id);
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
        self.prop("facing").map(Facing::from_minecraft).unwrap_or(Facing::North)
    }

    fn power_prop(&self) -> u8 {
        self.prop("power").and_then(|p| p.parse().ok()).unwrap_or(0)
    }

    /// What the VM simulates for this block: the redstone parts as they
    /// are, full blocks as conductors, everything else as air-like blocks
    /// that cut no wire and carry no power.
    pub fn block_type(&self) -> BlockType {
        let powered = self.is("powered", "true");
        let id = self.id();
        let path = id.strip_prefix("minecraft:").unwrap_or(id);
        match path {
            _ if id == Blocks::ANALOG_CABLE => BlockType::Cable,
            // Buses don't run inside a chip: a digital cable there is just a block.
            _ if id == Blocks::DIGITAL_CABLE => BlockType::Transparent,
            _ if id == Blocks::REDSTONE_PORT => BlockType::RedstonePort { output: self.redstone_port.as_ref().is_some_and(|p| p.output) },
            "air" | "cave_air" | "void_air" => BlockType::Air,
            "redstone_wire" => BlockType::RedstoneWire,
            "redstone_torch" => BlockType::RedstoneTorch { lit: !self.is("lit", "false"), attach: Facing::Down },
            "redstone_wall_torch" => {
                BlockType::RedstoneTorch { lit: !self.is("lit", "false"), attach: self.facing().opposite() }
            }
            "repeater" => BlockType::Repeater {
                delay_ticks: self.prop("delay").and_then(|d| d.parse().ok()).unwrap_or(1),
                facing: self.facing(),
            },
            "comparator" => BlockType::Comparator {
                mode: if self.is("mode", "subtract") { ComparatorMode::Subtract } else { ComparatorMode::Compare },
                facing: self.facing(),
            },
            "redstone_lamp" => BlockType::RedstoneLamp,
            "redstone_block" => BlockType::RedstoneBlock,
            "lever" => BlockType::Lever { on: powered, attach: self.mount() },
            p if p.ends_with("_button") => BlockType::Button { pressed: powered, attach: self.mount() },
            p if p.ends_with("weighted_pressure_plate") => BlockType::PressurePlate { power: self.power_prop() },
            p if p.ends_with("_pressure_plate") => BlockType::PressurePlate { power: if powered { 15 } else { 0 } },
            "observer" => BlockType::Observer { facing: self.facing() },
            "piston" | "sticky_piston" => BlockType::Piston {
                facing: self.facing(),
                sticky: path == "sticky_piston",
                extended: self.is("extended", "true"),
            },
            "piston_head" => BlockType::PistonHead { facing: self.facing(), sticky: self.is("type", "sticky") },
            "moving_piston" => BlockType::Air,
            "slime_block" => BlockType::Slime,
            "honey_block" => BlockType::Honey,
            "obsidian" | "crying_obsidian" | "bedrock" | "barrier" | "reinforced_deepslate" | "respawn_anchor"
            | "enchanting_table" | "ender_chest" | "end_portal_frame" => BlockType::Immovable,
            "hopper" => BlockType::Hopper { facing: self.facing() },
            "dropper" | "dispenser" => BlockType::Dropper { facing: self.facing(), triggered: self.is("triggered", "true") },
            "chest" | "trapped_chest" => BlockType::Container {
                slots: 27,
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
            "target" => BlockType::TargetBlock { power: self.power_prop() },
            "detector_rail" => BlockType::Source { power: if powered { 15 } else { 0 } },
            "daylight_detector" => BlockType::Source { power: self.power_prop() },
            p if p.contains("glass")
                || p.contains("slab")
                || p.contains("stairs")
                || p.contains("leaves")
                || p.contains("fence")
                || p.contains("wall")
                || p.contains("door")
                || p.contains("rail")
                || p.contains("carpet")
                || p == "barrier" => BlockType::Transparent,
            _ if Tier::from_alu_block(id).is_some() => BlockType::Alu,
            _ => port::dir_for_block(id).map_or(BlockType::Solid, BlockType::Port),
        }
    }
}

/// A chip's whole circuit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CircuitData {
    /// Edge length of the virtual world (the tier's).
    pub size: u32,
    pub blocks: Vec<CircuitBlock>,
    pub ports: Vec<Port>,
    /// The ALUs in it, with their chips and boards: they run inside it.
    #[serde(default)]
    pub alus: Vec<SavedAlu>,
}

/// How deep ALUs nest: an ALU in a chip in an ALU in a chip … stops here.
pub const MAX_NESTING: u32 = 4;

/// An ALU in a circuit, as it was in the editor: where, its tier, the chips
/// in its slots and its board's wires (between its sides and its chips' pins).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        CircuitData { size: tier.world_size(), ..Default::default() }
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
                    let circuit = load(&meta.id)?;
                    let ports = meta
                        .ports
                        .iter()
                        .map(|p| NestedPort { label: p.label.clone(), x: p.x, y: p.y, z: p.z, mode: p.dir })
                        .collect();
                    Some(NestedChip::new(*slot, ports, meta.tier.steps_per_tick(), circuit.vm_with(load, depth + 1)))
                })
                .collect();
            vm.set_nested(a.x, a.y, a.z, NestedAlu { chips, wires: a.wires.clone() });
        }
        vm
    }

    /// Saves a fabricated chip's circuit (chips can travel between worlds,
    /// so these live in the global store).
    pub fn save_for_chip(&self, game_dir: &str, chip: &str) {
        let mut store = Storage::open(game_dir, MOD_ID);
        store.set(format!("{CIRCUIT_KEY}{chip}"), serde_json::to_string(self).unwrap_or_default());
        if let Err(e) = store.flush() {
            yog_api::warn!("[yog-vlsi] saving chip circuit {chip}: {e}");
        }
    }

    pub fn load_for_chip(game_dir: &str, chip: &str) -> Option<Self> {
        let store = Storage::open(game_dir, MOD_ID);
        serde_json::from_str(store.get_str(&format!("{CIRCUIT_KEY}{chip}"))?).ok()
    }
}

/// A design, written on a Blueprint item (DESIGN.md §2.4): what the
/// workbench edits in the editor and fabricates chips from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blueprint {
    pub name: String,
    pub tier: Tier,
    pub circuit: CircuitData,
}

impl Blueprint {
    /// A new, empty design.
    pub fn new(name: impl Into<String>, tier: Tier) -> Self {
        Blueprint { name: name.into(), tier, circuit: CircuitData::empty(tier) }
    }

    pub fn to_nbt(&self) -> String {
        nbt::wrap(BLUEPRINT_TAG, &serde_json::to_string(self).unwrap_or_default())
    }

    /// The design on a Blueprint's NBT; `None` for a blank one.
    pub fn from_nbt(snbt: &str) -> Option<Self> {
        serde_json::from_str(&nbt::read(snbt, BLUEPRINT_TAG)?).ok()
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
            ports: vec![Port { label: "A0".into(), x: 1, y: 0, z: 2, dir: PortMode::Input }],
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
            blocks: vec![CircuitBlock { x: 0, y: 0, z: 0, state: "minecraft:repeater[delay=2]".into(), items: Vec::new(), redstone_port: None }],
            ports: meta().ports,
            alus: Vec::new(),
        };
        let bp = Blueprint { name: "Adder".into(), tier: Tier::Iron, circuit: c };
        assert_eq!(Blueprint::from_nbt(&bp.to_nbt()), Some(bp));
    }

    #[test]
    fn block_states_parse() {
        let b = |state: &str| CircuitBlock { x: 0, y: 0, z: 0, state: state.into(), items: Vec::new(), redstone_port: None };
        assert_eq!(b("minecraft:repeater[delay=3,facing=east,locked=false]").block_type(),
            BlockType::Repeater { delay_ticks: 3, facing: Facing::East });
        assert_eq!(b("minecraft:lever[face=wall,facing=east,powered=true]").block_type(),
            BlockType::Lever { on: true, attach: Facing::West });
        assert_eq!(b("minecraft:lever[face=floor,facing=east,powered=false]").block_type(),
            BlockType::Lever { on: false, attach: Facing::Down });
        assert_eq!(b("minecraft:redstone_wall_torch[facing=north,lit=true]").block_type(),
            BlockType::RedstoneTorch { lit: true, attach: Facing::South });
        assert_eq!(b("minecraft:polished_blackstone_button[face=ceiling,facing=north,powered=false]").block_type(),
            BlockType::Button { pressed: false, attach: Facing::Up });
        assert_eq!(b("minecraft:glass").block_type(), BlockType::Transparent);
        assert_eq!(b("minecraft:lime_shulker_box").block_type(), BlockType::Container { slots: 27, solid: false, partner: None });
        assert_eq!(b("minecraft:chest[facing=north,type=left,waterlogged=false]").block_type(),
            BlockType::Container { slots: 27, solid: false, partner: Some(Facing::East) });
        assert_eq!(b("minecraft:sticky_piston[extended=false,facing=up]").block_type(),
            BlockType::Piston { facing: Facing::Up, sticky: true, extended: false });
        assert_eq!(b("minecraft:obsidian").block_type(), BlockType::Immovable);
        assert_eq!(b("minecraft:stone").block_type(), BlockType::Solid);
        assert_eq!(b("minecraft:stone").block_type(), BlockType::Solid);
        assert_eq!(b(port::PORT_OUTPUT).block_type(), BlockType::Port(PortMode::Output));
    }

    #[test]
    fn a_bidirectional_port_goes_both_ways() {
        assert!(PortMode::Bidirectional.inputs() && PortMode::Bidirectional.outputs());
    }
}
