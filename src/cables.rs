//! Cables: two signal networks of Yog Pipes (DESIGN.md §4.4).
//!
//! - **Analog** (`analog_cable`): one level 0–15. A Redstone Port joins it
//!   to vanilla redstone. Right-click opens its screen (`port.yui`): its
//!   name, and Input (redstone → cable) or Output (cable → redstone). The
//!   ALU screen shows that name on the side whose cable reaches the port.
//! - **Digital** (`digital_cable`): ALU to ALU — a bus of [`BUS_CHANNELS`]
//!   channels, each a level 0–15 (the highest any ALU on it drives). An
//!   ALU side a digital cable reaches is a bus node in its graph.
//!
//! An ALU reads and drives each of its six sides through the cable block
//! next to it, so each side is its own connection.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
/// A digital bus's channels (an ALU's tier sees the first of them).
pub const BUS_CHANNELS: u16 = 256;

/// A Redstone Port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedstonePort {
    /// `true`: the cable's level comes out as redstone; `false`: the
    /// redstone it receives goes into the cable.
    pub output: bool,
    pub name: String,
}

/// (dimension, x, y, z) of a block.
pub type Key = (String, i32, i32, i32);

/// Every Redstone Port, by (dimension, position).
static PORTS: LazyLock<Mutex<HashMap<Key, RedstonePort>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// A cable was placed or broken: the ALUs look at their sides again.
static CABLES_CHANGED: AtomicBool = AtomicBool::new(false);
/// Ticks since the ports were last looked for in the world.
static PORT_TICKS: AtomicU64 = AtomicU64::new(0);
/// How often: a port broken by hand goes at once (`register`); one blown
/// up or pushed away, by the next look.
const PORT_CHECK_EVERY: u64 = 20;

/// Whether a cable was placed or broken since the last call.
pub fn cables_changed() -> bool {
    CABLES_CHANGED.swap(false, Ordering::Relaxed)
}

pub fn redstone_port(dimension: &str, pos: BlockPos) -> RedstonePort {
    PORTS
        .lock()
        .unwrap()
        .get(&(dimension.to_string(), pos.x, pos.y, pos.z))
        .cloned()
        .unwrap_or(RedstonePort { output: false, name: String::new() })
}

/// A cable or Redstone Port the mod set or cleared itself (the editor's
/// field), not a player: Yog Pipes looks at it again, and a port gone is
/// forgotten.
pub fn refresh(dim: &str, pos: BlockPos, block: &str) {
    if (is_cable(block) || block == Blocks::REDSTONE_PORT)
        && let Err(e) = yog_pipes::refresh(pipe_pos(dim, pos))
    {
        warn!("[yog-vlsi] Yog Pipes: {e}");
    }
}

/// Forgets the Redstone Port that stood at `pos`.
pub fn forget_port(dim: &str, pos: BlockPos) {
    PORTS.lock().unwrap().remove(&(dim.to_string(), pos.x, pos.y, pos.z));
}

/// Sets a Redstone Port's mode and name.
pub fn set_redstone_port(srv: &dyn yog_api::Server, dimension: &str, pos: BlockPos, port: RedstonePort) {
    if !port.output {
        srv.set_redstone_output(dimension, pos, 0);
    }
    PORTS.lock().unwrap().insert((dimension.to_string(), pos.x, pos.y, pos.z), port);
}

/// The analog network a cable block is part of.
fn network_at(dim: &str, pos: BlockPos) -> Option<u64> {
    yog_pipes::network_info(ANALOG.to_string(), pipe_pos(dim, pos)).ok().flatten().map(|n| n.network)
}

/// The names of the named Redstone Ports, by the analog network they're on.
pub fn port_names(srv: &dyn yog_api::Server) -> HashMap<(String, u64), String> {
    let ports: Vec<(Key, String)> = PORTS
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, p)| !p.name.is_empty())
        .map(|(k, p)| (k.clone(), p.name.clone()))
        .collect();
    let mut names = HashMap::new();
    for ((dim, x, y, z), name) in ports {
        for side in ["N", "S", "E", "W", "U", "D"] {
            let at = beside((x, y, z), side);
            if cable_at(srv, &dim, at) == Some(ANALOG)
                && let Some(net) = network_at(&dim, at)
            {
                names.insert((dim.clone(), net), name.clone());
            }
        }
    }
    names
}

/// The name of the Redstone Port the cable on an ALU side reaches.
pub fn side_port_name(
    srv: &dyn yog_api::Server,
    alu: &crate::board::AluId,
    side: &str,
    names: &HashMap<(String, u64), String>,
) -> Option<String> {
    let at = beside((alu.x, alu.y, alu.z), side);
    (cable_at(srv, &alu.dimension, at)? == ANALOG).then_some(())?;
    names.get(&(alu.dimension.clone(), network_at(&alu.dimension, at)?)).cloned()
}

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
pub fn cable_at(srv: &dyn yog_api::Server, dim: &str, pos: BlockPos) -> Option<&'static str> {
    match srv.get_block_state(dim, pos)?.split('[').next()? {
        id if id == Blocks::ANALOG_CABLE => Some(ANALOG),
        id if id == Blocks::DIGITAL_CABLE => Some(DIGITAL),
        _ => None,
    }
}

fn is_cable(block: &str) -> bool {
    block == Blocks::ANALOG_CABLE || block == Blocks::DIGITAL_CABLE
}


/// The level on channel `ch` of the bus at ALU side `side`.
pub fn bus_level(dim: &str, alu: (i32, i32, i32), side: &str, ch: u8) -> u8 {
    yog_pipes::level(DIGITAL.to_string(), pipe_pos(dim, beside(alu, side)), ch).unwrap_or(0)
}

/// Drives `level` onto channel `ch` of the bus at ALU side `side`.
pub fn bus_drive(dim: &str, alu: (i32, i32, i32), side: &str, ch: u8, level: u8) {
    if let Err(e) = yog_pipes::emit(DIGITAL.to_string(), pipe_pos(dim, beside(alu, side)), ch, level.min(15)) {
        warn!("[yog-vlsi] driving the bus: {e}");
    }
}

/// The level arriving at an ALU side from the cable there (`cable`, as
/// [`cable_at`] said): an analog one's, else `None`.
pub fn side_input(dim: &str, alu: (i32, i32, i32), side: &str, cable: Option<&str>) -> Option<u8> {
    (cable? == ANALOG).then_some(())?;
    yog_pipes::level(ANALOG.to_string(), pipe_pos(dim, beside(alu, side)), SIDE_CHANNEL).ok()
}

/// Drives `level` out of an ALU side into the cable there, if analog.
pub fn side_output(dim: &str, alu: (i32, i32, i32), side: &str, cable: Option<&str>, level: u8) {
    let at = beside(alu, side);
    if cable == Some(ANALOG)
        && let Err(e) = yog_pipes::emit(ANALOG.to_string(), pipe_pos(dim, at), SIDE_CHANNEL, level.min(15))
    {
        warn!("[yog-vlsi] driving an analog cable: {e}");
    }
}

/// Tells Yog Pipes about the cables and the Redstone Port — once every mod
/// has loaded.
fn register_networks() {
    let signal = |name: &str, prefix: &str, channels: u16| CapabilityDef {
        name: name.to_string(),
        kinds: Vec::new(),
        exclusive: true,
        state_prefix: prefix.to_string(),
        indicators: Vec::new(),
        distribution: Distribution::Nearest,
        events: None,
        mechanic: Mechanic::Signal { channels, attenuation: 0 },
    };
    let steps: [Result<(), String>; 5] = [
        yog_pipes::register_capability(signal(ANALOG, "analog", 1)),
        yog_pipes::register_capability(signal(DIGITAL, "digital", BUS_CHANNELS)),
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

/// The Redstone Ports, with whether they're outputs — every
/// [`PORT_CHECK_EVERY`] ticks, those that still stand (`check`).
fn ports(srv: &dyn yog_api::Server, check: bool) -> Vec<((String, i32, i32, i32), bool)> {
    let all: Vec<(Key, bool)> = PORTS.lock().unwrap().iter().map(|(k, p)| (k.clone(), p.output)).collect();
    if !check {
        return all;
    }
    all.into_iter()
        .filter(|((dim, x, y, z), _)| {
            let there = srv
                .get_block_state(dim, BlockPos { x: *x, y: *y, z: *z })
                .is_some_and(|s| s.starts_with(Blocks::REDSTONE_PORT));
            if !there {
                PORTS.lock().unwrap().remove(&(dim.clone(), *x, *y, *z));
            }
            there
        })
        .collect()
}

/// Redstone into the cables, before the ALUs step: what reaches a port this
/// tick is in its network for them this tick.
pub fn tick_inputs(srv: &dyn yog_api::Server) {
    let check = PORT_TICKS.fetch_add(1, Ordering::Relaxed).is_multiple_of(PORT_CHECK_EVERY);
    for ((dim, x, y, z), output) in ports(srv, check) {
        let pos = BlockPos { x, y, z };
        if output {
            let _ = yog_pipes::emit(ANALOG.to_string(), pipe_pos(&dim, pos), SIDE_CHANNEL, 0);
        } else {
            let power = srv.redstone_power(&dim, pos).unwrap_or(0);
            let _ = yog_pipes::emit(ANALOG.to_string(), pipe_pos(&dim, pos), SIDE_CHANNEL, power);
        }
    }
}

/// Cable levels out as redstone, after the ALUs stepped: what they put out
/// this tick leaves this tick.
pub fn tick_outputs(srv: &dyn yog_api::Server) {
    for ((dim, x, y, z), output) in ports(srv, false) {
        if output {
            let pos = BlockPos { x, y, z };
            let level = yog_pipes::level(ANALOG.to_string(), pipe_pos(&dim, pos), SIDE_CHANNEL).unwrap_or(0);
            srv.set_redstone_output(&dim, pos, level);
        }
    }
}

const STORE_FILE: &str = "yog-vlsi/redstone_ports";
const STORE_KEY: &str = "ports";

pub fn load(srv: &dyn yog_api::Server) {
    register_networks();
    let store = Storage::open(&srv.game_dir(), STORE_FILE);
    let saved: Vec<(Key, RedstonePort)> = store
        .get(STORE_KEY)
        .and_then(|v| v.as_str())
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    *PORTS.lock().unwrap() = saved.into_iter().collect();
}

pub fn save(srv: &dyn yog_api::Server) {
    let ports: Vec<(Key, RedstonePort)> = PORTS.lock().unwrap().iter().map(|(k, p)| (k.clone(), p.clone())).collect();
    if let Ok(json) = serde_json::to_string(&ports) {
        let mut store = Storage::open(&srv.game_dir(), STORE_FILE);
        store.set(STORE_KEY, &*json);
        let _ = store.flush();
    }
}

pub fn register(registry: &mut Registry) {
    // A placed port starts as an unnamed input; right-click opens its
    // screen (`port.rs`).
    registry.on_player_place_block(|e, phase, _srv| {
        if phase == EventPhase::Pre && e.block_id == Blocks::REDSTONE_PORT {
            PORTS
                .lock()
                .unwrap()
                .insert((e.dimension.clone(), e.pos.x, e.pos.y, e.pos.z), RedstonePort { output: false, name: String::new() });
        }
        if phase == EventPhase::Post && is_cable(&e.block_id) {
            CABLES_CHANGED.store(true, Ordering::Relaxed);
        }
        true
    });
    registry.on_block_break(|e, phase, _srv| {
        if phase == EventPhase::Post {
            if e.block_id == Blocks::REDSTONE_PORT {
                PORTS.lock().unwrap().remove(&(e.dimension.clone(), e.pos.x, e.pos.y, e.pos.z));
            } else if is_cable(&e.block_id) {
                CABLES_CHANGED.store(true, Ordering::Relaxed);
            }
        }
        true
    });
}
