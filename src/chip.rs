//! Microchip data: the metadata a chip item carries in its NBT and the
//! circuit (block grid + ports) kept in server storage.

use serde::{Deserialize, Serialize};
use yog_api::Storage;

use crate::nbt;
use crate::res::Strings;
use crate::tier::Tier;
use crate::port;
use crate::vm::{BlockType, ComparatorMode, DoorHalf, Facing, PortMode, RedstoneVM};
use crate::MOD_ID;

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

    /// Short tag shown on pins (`IN`, `OUT`, `BIDI`).
    pub fn short(self) -> String {
        match self {
            PortMode::Input => Strings::port_short_input(),
            PortMode::Output => Strings::port_short_output(),
            PortMode::Bidirectional => Strings::port_short_bidi(),
        }
    }

    pub fn next(self) -> PortMode {
        match self {
            PortMode::Input => PortMode::Output,
            PortMode::Output => PortMode::Bidirectional,
            PortMode::Bidirectional => PortMode::Input,
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

    /// What the VM simulates for this block; anything it doesn't know is a
    /// plain solid block.
    pub fn block_type(&self) -> BlockType {
        let facing = || self.prop("facing").map(Facing::from_minecraft).unwrap_or(Facing::North);
        let powered = || self.is("powered", "true");
        let open = || self.is("open", "true");
        let half = || if self.is("half", "upper") || self.is("half", "top") { DoorHalf::Upper } else { DoorHalf::Lower };
        match self.id() {
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => BlockType::Air,
            "minecraft:redstone_wire" => BlockType::RedstoneWire,
            "minecraft:redstone_torch" => BlockType::RedstoneTorch { lit: !self.is("lit", "false") },
            "minecraft:redstone_wall_torch" => BlockType::RedstoneWallTorch { lit: !self.is("lit", "false"), facing: facing() },
            "minecraft:repeater" => BlockType::Repeater {
                delay_ticks: self.prop("delay").and_then(|d| d.parse().ok()).unwrap_or(1),
                facing: facing(),
                locked: self.is("locked", "true"),
            },
            "minecraft:comparator" => BlockType::Comparator {
                mode: if self.is("mode", "subtract") { ComparatorMode::Subtract } else { ComparatorMode::Compare },
                facing: facing(),
            },
            "minecraft:redstone_lamp" => BlockType::RedstoneLamp,
            "minecraft:redstone_block" => BlockType::RedstoneBlock,
            "minecraft:lever" => BlockType::Lever { on: powered() },
            "minecraft:stone_button" => BlockType::StoneButton { pressed: powered(), facing: facing() },
            "minecraft:oak_button" => BlockType::WoodButton { pressed: powered(), facing: facing() },
            "minecraft:stone_pressure_plate" => BlockType::StonePressurePlate { pressed: powered() },
            "minecraft:oak_pressure_plate" => BlockType::WoodPressurePlate { pressed: powered() },
            "minecraft:light_weighted_pressure_plate" => BlockType::LightWeightedPressurePlate { power: 0 },
            "minecraft:heavy_weighted_pressure_plate" => BlockType::HeavyWeightedPressurePlate { power: 0 },
            "minecraft:observer" => BlockType::Observer { facing: facing(), powered: powered() },
            "minecraft:note_block" => BlockType::NoteBlock,
            "minecraft:target" => BlockType::TargetBlock { power: 0 },
            "minecraft:piston" => BlockType::Piston { extended: self.is("extended", "true"), facing: facing() },
            "minecraft:sticky_piston" => BlockType::StickyPiston { extended: self.is("extended", "true"), facing: facing() },
            "minecraft:chest" => BlockType::Chest,
            "minecraft:trapped_chest" => BlockType::TrappedChest,
            "minecraft:ender_chest" => BlockType::EnderChest,
            id if id.ends_with("shulker_box") => BlockType::ShulkerBox,
            "minecraft:barrel" => BlockType::Barrel,
            "minecraft:hopper" => BlockType::Hopper { facing: facing(), enabled: !self.is("enabled", "false") },
            "minecraft:dropper" => BlockType::Dropper { facing: facing(), triggered: self.is("triggered", "true") },
            "minecraft:dispenser" => BlockType::Dispenser { facing: facing(), triggered: self.is("triggered", "true") },
            "minecraft:furnace" => BlockType::Furnace { lit: self.is("lit", "true") },
            "minecraft:blast_furnace" => BlockType::BlastFurnace { lit: self.is("lit", "true") },
            "minecraft:smoker" => BlockType::Smoker { lit: self.is("lit", "true") },
            "minecraft:brewing_stand" => BlockType::BrewingStand,
            "minecraft:slime_block" => BlockType::SlimeBlock,
            "minecraft:honey_block" => BlockType::HoneyBlock,
            "minecraft:tnt" => BlockType::Tnt { unstable: self.is("unstable", "true") },
            "minecraft:rail" => BlockType::Rail,
            "minecraft:powered_rail" => BlockType::PoweredRail { powered: powered() },
            "minecraft:detector_rail" => BlockType::DetectorRail { powered: powered() },
            "minecraft:activator_rail" => BlockType::ActivatorRail { powered: powered() },
            "minecraft:iron_door" => BlockType::IronDoor { open: open(), facing: facing(), half: half() },
            "minecraft:oak_door" => BlockType::WoodDoor { open: open(), facing: facing(), half: half() },
            "minecraft:iron_trapdoor" => BlockType::IronTrapdoor { open: open(), facing: facing(), half: half() },
            "minecraft:oak_trapdoor" => BlockType::WoodTrapdoor { open: open(), facing: facing(), half: half() },
            "minecraft:oak_fence_gate" => BlockType::FenceGate { open: open(), facing: facing() },
            "minecraft:glass" => BlockType::Glass,
            id => port::dir_for_block(id).map_or(BlockType::Solid, BlockType::Port),
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
}

impl CircuitData {
    pub fn empty(tier: Tier) -> Self {
        CircuitData { size: tier.world_size(), ..Default::default() }
    }

    /// Places the circuit — its blocks and ports — into `vm`.
    pub fn load_into(&self, vm: &mut RedstoneVM) {
        for block in &self.blocks {
            vm.set_block(block.x, block.y, block.z, block.block_type());
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

    pub fn to_blueprint_nbt(&self) -> String {
        nbt::wrap(BLUEPRINT_TAG, &serde_json::to_string(self).unwrap_or_default())
    }

    pub fn from_blueprint_nbt(snbt: &str) -> Option<Self> {
        serde_json::from_str(&nbt::read(snbt, BLUEPRINT_TAG)?).ok()
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
            blocks: vec![CircuitBlock { x: 0, y: 0, z: 0, state: "minecraft:repeater[delay=2]".into() }],
            ports: meta().ports,
        };
        assert_eq!(CircuitData::from_blueprint_nbt(&c.to_blueprint_nbt()), Some(c));
    }

    #[test]
    fn block_states_parse() {
        let b = |state: &str| CircuitBlock { x: 0, y: 0, z: 0, state: state.into() };
        assert_eq!(b("minecraft:repeater[delay=3,facing=east,locked=false]").block_type(),
            BlockType::Repeater { delay_ticks: 3, facing: Facing::East, locked: false });
        assert_eq!(b("minecraft:lever[powered=true]").block_type(), BlockType::Lever { on: true });
        assert_eq!(b("minecraft:lime_shulker_box").block_type(), BlockType::ShulkerBox);
        assert_eq!(b("minecraft:stone").block_type(), BlockType::Solid);
        assert_eq!(b(port::PORT_OUTPUT).block_type(), BlockType::Port(PortMode::Output));
    }

    #[test]
    fn port_modes_cycle() {
        let mut m = PortMode::Input;
        for _ in 0..3 {
            m = m.next();
        }
        assert_eq!(m, PortMode::Input);
        assert!(PortMode::Bidirectional.inputs() && PortMode::Bidirectional.outputs());
    }
}
