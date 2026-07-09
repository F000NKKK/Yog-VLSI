//! Microchip system: item registration per tier, data model, NBT serialization.
//!
//! Chip items store metadata (id, tier, ports) in their NBT.
//! The actual circuit data (block grid) lives in server-side Storage
//! to keep item NBT compact.

use serde::{Deserialize, Serialize};
use yog_api::{ItemDef, Registry};
use crate::vm::Tier;

// ── Chip ID ───────────────────────────────────────────────────────────────────

/// Unique chip identifier (UUID v4 as string).
pub type ChipId = String;

/// Generate a new chip ID.
pub fn new_chip_id() -> ChipId {
    uuid::Uuid::new_v4().to_string()
}

// ── Port definitions ──────────────────────────────────────────────────────────

/// Which edge of the chip a port sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortSide {
    North,
    South,
    East,
    West,
}

impl PortSide {
    pub fn name(self) -> &'static str {
        match self {
            PortSide::North => "North",
            PortSide::South => "South",
            PortSide::East  => "East",
            PortSide::West  => "West",
        }
    }
}

/// I/O direction for a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortDir {
    Input,
    Output,
    Bidirectional,
}

impl PortDir {
    pub fn name(self) -> &'static str {
        match self {
            PortDir::Input => "IN",
            PortDir::Output => "OUT",
            PortDir::Bidirectional => "BIDI",
        }
    }
}

/// A single I/O port on the chip boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Port {
    /// Display label (e.g. "A0", "CLK", "Q")
    pub label: String,
    /// Which edge of the chip
    pub side: PortSide,
    /// Position along the edge (0-based index within the world size)
    pub index: u32,
    /// I/O direction
    pub dir: PortDir,
}

// ── Chip metadata (item NBT) ──────────────────────────────────────────────────

/// Chip metadata stored in the item's NBT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipMeta {
    /// Unique chip ID (references circuit data in server storage).
    pub id: ChipId,
    /// Microchip tier.
    pub tier: Tier,
    /// User-assigned name.
    pub name: String,
    /// I/O port list.
    pub ports: Vec<Port>,
}

impl ChipMeta {
    /// Serialize to a JSON string (for storage in item NBT).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Deserialize from a JSON string.
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    /// Build the SNBT string for this chip's item NBT tag.
    /// The chip meta is stored as a JSON string inside the `YogVlsiChip` NBT key.
    pub fn to_nbt(&self) -> String {
        // Escape the JSON for safe embedding in SNBT string
        let json = self.to_json();
        let escaped = json.replace('\\', "\\\\").replace('"', "\\\"");
        format!("{{YogVlsiChip: \"{}\"}}", escaped)
    }

    /// Parse chip meta from an SNBT string.
    pub fn from_nbt(nbt: &str) -> Option<Self> {
        // Simple extraction: find "YogVlsiChip":"<json>"
        let key = "YogVlsiChip:\"";
        if let Some(start) = nbt.find(key) {
            let start = start + key.len();
            // Find the closing unescaped quote
            let mut end = start;
            let chars: Vec<char> = nbt[start..].chars().collect();
            let mut i = 0;
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 2; // skip escaped char
                } else if chars[i] == '"' {
                    end = start + i;
                    break;
                } else {
                    i += 1;
                }
            }
            let json = &nbt[start..end].replace("\\\"", "\"").replace("\\\\", "\\");
            Self::from_json(json)
        } else {
            None
        }
    }
}

// ── Circuit data (server storage) ─────────────────────────────────────────────

/// A single block in the virtual circuit world.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBlock {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    /// Minecraft block ID (e.g. "minecraft:redstone_wire")
    pub block_id: String,
    /// JSON-encoded block state properties (e.g. `{"power":"0","north":"none",...}`)
    pub state_json: String,
}

/// Full circuit data for a chip — stored server-side via Storage API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitData {
    /// Chip ID this data belongs to.
    pub chip_id: ChipId,
    /// Virtual world dimensions (matches tier).
    pub width: u32,
    pub height: u32,
    /// Placed blocks in the virtual world.
    pub blocks: Vec<CircuitBlock>,
    /// Port definitions (also in ChipMeta, but duplicated here for self-containment).
    pub ports: Vec<Port>,
}

impl CircuitData {
    pub fn new(chip_id: ChipId, width: u32, height: u32, ports: Vec<Port>) -> Self {
        CircuitData {
            chip_id,
            width,
            height,
            blocks: Vec::new(),
            ports,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

// ── Item registration ─────────────────────────────────────────────────────────

pub fn register(registry: &mut Registry) {
    for tier in Tier::ALL {
        let item_id = format!("yog-vlsi:chip_{}", tier.id());

        let tooltip = format!(
            "§7Tier: §e{}§7 | §b{} ticks/s§7 | §a{}×{} world\n§7Place in a §6VLSI Workbench§7 to design circuits.\n§7Ports: none (unprogrammed)",
            tier.name(),
            tier.tick_rate(),
            tier.world_size(),
            tier.world_size()
        );

        registry.register_item(
            ItemDef::new(&item_id)
                .tooltip(&tooltip)
                .max_stack(1)
        );

        // Register programmed variant (same item, NBT distinguishes programmed vs blank)
        // The item is the same — we detect blank vs programmed by presence of NBT.
    }
}
