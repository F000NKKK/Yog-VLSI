//! Microchip data: the metadata a chip item carries in its NBT and the
//! circuit (block grid + ports) kept in server storage.

use serde::{Deserialize, Serialize};
use yog_api::Storage;

use crate::nbt;
use crate::res::Strings;
use crate::tier::Tier;
use crate::vm::PortMode;
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
    fn port_modes_cycle() {
        let mut m = PortMode::Input;
        for _ in 0..3 {
            m = m.next();
        }
        assert_eq!(m, PortMode::Input);
        assert!(PortMode::Bidirectional.inputs() && PortMode::Bidirectional.outputs());
    }
}
