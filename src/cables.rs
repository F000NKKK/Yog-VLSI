//! Cables: two signal networks of Yog Pipes (DESIGN.md §4.4).
//!
//! - **Analog** (`analog_cable`): one level 0–15. A Redstone Port joins it
//!   to vanilla redstone; right-click flips the port between Input
//!   (redstone → cable) and Output (cable → redstone).
//! - **Digital** (`digital_cable`): ALU to ALU.
//!
//! An ALU reads and drives each of its six sides through the cable block
//! next to it, so each side is its own connection.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use yog_api::{BlockPos, EventPhase, Registry, Storage, warn};
use yog_pipes::{
    AttachDef, CapabilityDef, Distribution, EndpointDef, EndpointTarget, Mechanic, PipePos,
};

use crate::content::Blocks;

pub const ANALOG: &str = "yog-vlsi:analog";
pub const DIGITAL: &str = "yog-vlsi:digital";
/// The channel an ALU side's level travels on.
const SIDE_CHANNEL: u8 = 0;

/// How a Redstone Port faces the redstone next to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortMode {
    /// Redstone power it receives goes into the cable.
    Input,
    /// The cable's level comes out as redstone.
    Output,
}

/// Every Redstone Port: (dimension, position) → its mode.
static PORTS: LazyLock<Mutex<HashMap<(String, i32, i32, i32), PortMode>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn pipe_pos(dim: &str, p: BlockPos) -> PipePos {
    PipePos { dim: dim.to_string(), x: p.x, y: p.y, z: p.z }
}

/// The block next to `pos` on ALU side `side` (`N` `S` `E` `W` `U` `D`).
pub fn beside(pos: (i32, i32, i32), side: &str) -> BlockPos {
    let (x, y, z) = pos;
    let (dx, dy, dz) = match side {
        "N" => (0, 0, -1),
        "S" => (0, 0, 1),
        "E" => (1, 0, 0),
        "W" => (-1, 0, 0),
        "U" => (0, 1, 0),
        _ => (0, -1, 0),
    };
    BlockPos { x: x + dx, y: y + dy, z: z + dz }
}

/// The cable network next to an ALU side: (capability, cable position).
fn cable_at(srv: &dyn yog_api::Server, dim: &str, pos: BlockPos) -> Option<&'static str> {
    match srv.get_block_state(dim, pos)?.split('[').next()? {
        id if id == Blocks::ANALOG_CABLE => Some(ANALOG),
        id if id == Blocks::DIGITAL_CABLE => Some(DIGITAL),
        _ => None,
    }
}

/// The level arriving at an ALU side from its cable (`None`: no cable).
pub fn side_input(srv: &dyn yog_api::Server, dim: &str, alu: (i32, i32, i32), side: &str) -> Option<u8> {
    let at = beside(alu, side);
    let cap = cable_at(srv, dim, at)?;
    yog_pipes::level(cap.to_string(), pipe_pos(dim, at), SIDE_CHANNEL).ok()
}

/// Drives `level` out of an ALU side into its cable (if there is one).
pub fn side_output(srv: &dyn yog_api::Server, dim: &str, alu: (i32, i32, i32), side: &str, level: u8) {
    let at = beside(alu, side);
    if let Some(cap) = cable_at(srv, dim, at)
        && let Err(e) = yog_pipes::emit(cap.to_string(), pipe_pos(dim, at), SIDE_CHANNEL, level.min(15))
    {
        warn!("[yog-vlsi] emitting on {cap}: {e}");
    }
}

/// Tells Yog Pipes about the cables and the Redstone Port — once every mod
/// has loaded.
fn register_networks() {
    let signal = |name: &str, prefix: &str| CapabilityDef {
        name: name.to_string(),
        kinds: Vec::new(),
        exclusive: true,
        state_prefix: prefix.to_string(),
        indicators: Vec::new(),
        distribution: Distribution::Nearest,
        events: None,
        mechanic: Mechanic::Signal { channels: 1, attenuation: 0 },
    };
    let steps: [Result<(), String>; 5] = [
        yog_pipes::register_capability(signal(ANALOG, "analog")),
        yog_pipes::register_capability(signal(DIGITAL, "digital")),
        yog_pipes::attach(AttachDef {
            block: Blocks::ANALOG_CABLE.to_string(),
            capability: ANALOG.to_string(),
            groups: vec!["analog".into()],
            params: HashMap::new(),
        }),
        yog_pipes::attach(AttachDef {
            block: Blocks::DIGITAL_CABLE.to_string(),
            capability: DIGITAL.to_string(),
            groups: vec!["digital".into()],
            params: HashMap::new(),
        }),
        yog_pipes::register_endpoint(EndpointDef {
            capability: ANALOG.to_string(),
            target: EndpointTarget::Block(Blocks::REDSTONE_PORT.to_string()),
            kinds: Vec::new(),
            can_insert: false,
            can_extract: false,
            listens_signals: true,
            listens_messages: false,
            groups: Vec::new(),
            priority: 0,
            max_packet: 0,
            pressure: 0,
            handler: None,
        }),
    ];
    for r in steps {
        if let Err(e) = r {
            warn!("[yog-vlsi] Yog Pipes: {e}");
        }
    }
}

/// Every Redstone Port, each tick: redstone in, or cable level out.
pub fn tick(srv: &dyn yog_api::Server) {
    let ports: Vec<_> = PORTS.lock().unwrap().iter().map(|(k, m)| (k.clone(), *m)).collect();
    for ((dim, x, y, z), mode) in ports {
        let pos = BlockPos { x, y, z };
        if srv.get_block_state(&dim, pos).is_none_or(|s| !s.starts_with(Blocks::REDSTONE_PORT)) {
            PORTS.lock().unwrap().remove(&(dim, x, y, z));
            continue;
        }
        match mode {
            PortMode::Input => {
                let power = srv.redstone_power(&dim, pos).unwrap_or(0);
                let _ = yog_pipes::emit(ANALOG.to_string(), pipe_pos(&dim, pos), SIDE_CHANNEL, power);
            }
            PortMode::Output => {
                let _ = yog_pipes::emit(ANALOG.to_string(), pipe_pos(&dim, pos), SIDE_CHANNEL, 0);
                let level = yog_pipes::level(ANALOG.to_string(), pipe_pos(&dim, pos), SIDE_CHANNEL).unwrap_or(0);
                srv.set_redstone_output(&dim, pos, level);
            }
        }
    }
}

const STORE_FILE: &str = "yog-vlsi/redstone_ports";
const STORE_KEY: &str = "ports";

pub fn load(srv: &dyn yog_api::Server) {
    register_networks();
    let store = Storage::open(&srv.game_dir(), STORE_FILE);
    let saved: Vec<((String, i32, i32, i32), PortMode)> = store
        .get(STORE_KEY)
        .and_then(|v| v.as_str())
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    *PORTS.lock().unwrap() = saved.into_iter().collect();
}

pub fn save(srv: &dyn yog_api::Server) {
    let ports: Vec<_> = PORTS.lock().unwrap().iter().map(|(k, m)| (k.clone(), *m)).collect();
    if let Ok(json) = serde_json::to_string(&ports) {
        let mut store = Storage::open(&srv.game_dir(), STORE_FILE);
        store.set(STORE_KEY, &*json);
        let _ = store.flush();
    }
}

pub fn register(registry: &mut Registry) {
    // A placed port starts as an input.
    registry.on_player_place_block(|e, phase, _srv| {
        if phase == EventPhase::Pre && e.block_id == Blocks::REDSTONE_PORT {
            PORTS
                .lock()
                .unwrap()
                .insert((e.dimension.clone(), e.pos.x, e.pos.y, e.pos.z), PortMode::Input);
        }
        true
    });
    // Right-click flips it.
    registry.on_use_block(|e, phase, srv| -> bool {
        if phase != EventPhase::Pre || e.block_id != Blocks::REDSTONE_PORT {
            return true;
        }
        let key = (e.dimension.clone(), e.pos.x, e.pos.y, e.pos.z);
        let mode = {
            let mut ports = PORTS.lock().unwrap();
            let m = ports.entry(key).or_insert(PortMode::Input);
            *m = if *m == PortMode::Input { PortMode::Output } else { PortMode::Input };
            *m
        };
        if mode == PortMode::Input {
            srv.set_redstone_output(&e.dimension, e.pos, 0);
        }
        let text = match mode {
            PortMode::Input => crate::res::Strings::redstone_port_input(),
            PortMode::Output => crate::res::Strings::redstone_port_output(),
        };
        srv.send_actionbar(&e.player_name, &text);
        false
    });
}
