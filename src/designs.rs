//! Per-player design library.
//!
//! Designs (чертежи) are stored per-player via Storage::open_player.
//! They are available from any workbench — not tied to a specific block.

use serde::{Deserialize, Serialize};
use yog_api::Storage;

use crate::chip::{CircuitData, Port};
use crate::vm::Tier;

// ── Design metadata ──────────────────────────────────────────────────────────

/// Metadata for a saved design. The full circuit data is stored separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignMeta {
    /// Unique design ID.
    pub id: String,
    /// Player-assigned name.
    pub name: String,
    /// Chip tier.
    pub tier: Tier,
    /// Short description (optional).
    pub description: String,
    /// When the design was last saved (Unix timestamp).
    pub saved_at: u64,
    /// Port count summary.
    pub port_count: usize,
}

/// A design entry for listing in the library.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignEntry {
    pub meta: DesignMeta,
    /// Full circuit data.
    pub circuit: CircuitData,
}

// ── Storage keys ─────────────────────────────────────────────────────────────

fn designs_index_key() -> String {
    "designs_index".into()
}

fn design_circuit_key(design_id: &str) -> String {
    format!("design_circuit/{}", design_id)
}

// ── Public API ───────────────────────────────────────────────────────────────

/// List all designs for a player.
pub fn list_designs(game_dir: &str, player_uuid: &str) -> Vec<DesignMeta> {
    let store = Storage::open_player(game_dir, "yog-vlsi", player_uuid);
    let json = store.get(&designs_index_key())
        .and_then(|v| v.as_str())
        .map(String::from);
    json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
}

/// Save the design index.
fn save_index(game_dir: &str, player_uuid: &str, designs: &[DesignMeta]) {
    let mut store = Storage::open_player(game_dir, "yog-vlsi", player_uuid);
    let json = serde_json::to_string(designs).unwrap_or_default();
    store.set(&designs_index_key(), &json);
    let _ = store.flush();
}

/// Save a full design (meta + circuit) for a player.
pub fn save_design(game_dir: &str, player_uuid: &str, entry: &DesignEntry) {
    // Save circuit data
    {
        let mut store = Storage::open_player(game_dir, "yog-vlsi", player_uuid);
        store.set(&design_circuit_key(&entry.meta.id), entry.circuit.to_json());
        let _ = store.flush();
    }

    // Update index
    let mut designs = list_designs(game_dir, player_uuid);
    designs.retain(|d| d.id != entry.meta.id);
    designs.push(entry.meta.clone());
    save_index(game_dir, player_uuid, &designs);
}

/// Load a full design by ID.
pub fn load_design(game_dir: &str, player_uuid: &str, design_id: &str) -> Option<DesignEntry> {
    let designs = list_designs(game_dir, player_uuid);
    let meta = designs.into_iter().find(|d| d.id == design_id)?;

    let store = Storage::open_player(game_dir, "yog-vlsi", player_uuid);
    let json = store.get(&design_circuit_key(design_id))
        .and_then(|v| v.as_str())
        .map(String::from);
    let circuit = json.and_then(|j| CircuitData::from_json(&j))?;

    Some(DesignEntry { meta, circuit })
}

/// Delete a design.
pub fn delete_design(game_dir: &str, player_uuid: &str, design_id: &str) {
    let mut designs = list_designs(game_dir, player_uuid);
    designs.retain(|d| d.id != design_id);
    save_index(game_dir, player_uuid, &designs);
}

/// Import a design from a CircuitData (e.g., from a Blueprint).
/// Creates a new design ID and saves it to the player's library.
pub fn import_design(
    game_dir: &str,
    player_uuid: &str,
    name: &str,
    tier: Tier,
    ports: Vec<Port>,
    circuit: CircuitData,
) -> String {
    let design_id = crate::chip::new_chip_id();
    let entry = DesignEntry {
        meta: DesignMeta {
            id: design_id.clone(),
            name: name.to_string(),
            tier,
            description: format!("Imported — {} ports", circuit.ports.len()),
            saved_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            port_count: circuit.ports.len(),
        },
        circuit,
    };
    save_design(game_dir, player_uuid, &entry);
    design_id
}

/// Create a new blank design with the given parameters.
pub fn create_design(
    game_dir: &str,
    player_uuid: &str,
    name: &str,
    tier: Tier,
) -> String {
    let design_id = crate::chip::new_chip_id();
    let size = tier.world_size();
    let entry = DesignEntry {
        meta: DesignMeta {
            id: design_id.clone(),
            name: name.to_string(),
            tier,
            description: String::new(),
            saved_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            port_count: 0,
        },
        circuit: CircuitData::new(design_id.clone(), size, size, Vec::new()),
    };
    save_design(game_dir, player_uuid, &entry);
    design_id
}
