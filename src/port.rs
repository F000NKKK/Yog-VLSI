//! VLSI Port block: what players place inside the circuit editor to give a
//! design its pins. The mode (Input/Output/Bidirectional) is which of the
//! three block variants stands there; the name is kept here, per position.
//! Right-click opens the port's screen (`port.yui`, this file is its
//! code-behind) to name it and pick its mode. Saving the design (`/vlsi
//! save`) turns every port in the cube into a pin of that name.

yog_api::ui_include!("port");

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use yog_api::{BlockPos, EventPhase, Registry, Server, Storage};
use yog_kit::prelude::*;

use crate::content::Blocks;
use crate::vm::PortMode;

pub const PORT_INPUT: &str = Blocks::PORT_INPUT;
pub const PORT_OUTPUT: &str = Blocks::PORT_OUTPUT;
pub const PORT_BIDI: &str = Blocks::PORT_BIDI;

/// Server → client: the port to edit. Client → server: what it became.
const CHANNEL: &str = "yog-vlsi:port";

/// Every named port: (dimension, x, y, z) → name.
static LABELS: LazyLock<Mutex<HashMap<(String, i32, i32, i32), String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn key(dimension: &str, pos: BlockPos) -> (String, i32, i32, i32) {
    (dimension.to_string(), pos.x, pos.y, pos.z)
}

pub fn is_port_block(block_id: &str) -> bool {
    matches!(block_id, PORT_INPUT | PORT_OUTPUT | PORT_BIDI)
}

pub fn dir_for_block(block_id: &str) -> Option<PortMode> {
    match block_id {
        PORT_INPUT => Some(PortMode::Input),
        PORT_OUTPUT => Some(PortMode::Output),
        PORT_BIDI => Some(PortMode::Bidirectional),
        _ => None,
    }
}

pub fn block_for_dir(dir: PortMode) -> &'static str {
    match dir {
        PortMode::Input => PORT_INPUT,
        PortMode::Output => PORT_OUTPUT,
        PortMode::Bidirectional => PORT_BIDI,
    }
}

/// The name of the port at `pos`, if it has one.
pub fn read_label(_srv: &dyn yog_api::Server, dimension: &str, pos: BlockPos) -> Option<String> {
    LABELS.lock().unwrap().get(&key(dimension, pos)).cloned()
}

/// Names the port at `pos` (an empty name forgets it).
pub fn write_label(_srv: &dyn yog_api::Server, dimension: &str, pos: BlockPos, label: &str) {
    let mut labels = LABELS.lock().unwrap();
    if label.trim().is_empty() {
        labels.remove(&key(dimension, pos));
    } else {
        labels.insert(key(dimension, pos), label.trim().to_string());
    }
}

/// One port being edited.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PortEdit {
    dimension: String,
    pos: (i32, i32, i32),
    label: String,
    mode: PortMode,
}

// ── The screen's view model ──────────────────────────────────────────────────

#[derive(Default)]
pub struct PortVm {
    edit: Option<PortEdit>,
    label: String,
    mode: Option<PortMode>,
}

impl ViewModel for PortVm {}

impl PortVm {
    fn load(&mut self, edit: PortEdit) {
        self.label = edit.label.clone();
        self.mode = Some(edit.mode);
        self.edit = Some(edit);
    }
    pub fn mode(&self) -> PortMode {
        self.mode.unwrap_or(PortMode::Input)
    }
    pub fn set_mode(&mut self, mode: PortMode) {
        self.mode = Some(mode);
    }
    pub fn done(&mut self, cx: &mut Cx<'_, Self>) {
        if let Some(edit) = &self.edit {
            let out = PortEdit { label: self.label.trim().to_string(), mode: self.mode(), ..edit.clone() };
            if let (Some(srv), Ok(json)) = (yog_api::server(), serde_json::to_string(&out)) {
                srv.send_to_server(CHANNEL, json.as_bytes());
            }
        }
        cx.close();
    }
}

// ── The server side ──────────────────────────────────────────────────────────

const STORE_FILE: &str = "yog-vlsi/port_labels";
const STORE_KEY: &str = "labels";

pub fn load(srv: &dyn yog_api::Server) {
    let store = Storage::open(&srv.game_dir(), STORE_FILE);
    let saved: Vec<((String, i32, i32, i32), String)> = store
        .get(STORE_KEY)
        .and_then(|v| v.as_str())
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    *LABELS.lock().unwrap() = saved.into_iter().collect();
}

pub fn save(srv: &dyn yog_api::Server) {
    let labels: Vec<_> = LABELS.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    if let Ok(json) = serde_json::to_string(&labels) {
        let mut store = Storage::open(&srv.game_dir(), STORE_FILE);
        store.set(STORE_KEY, &*json);
        let _ = store.flush();
    }
}

pub fn setup(registry: &mut Registry) {
    let screen = register(registry);

    // Server → client: open the screen on this port.
    registry.on_client_packet(CHANNEL, move |ev, _srv| {
        if let Ok(edit) = serde_json::from_slice::<PortEdit>(&ev.payload) {
            screen.update(move |vm| vm.load(edit));
            open();
        }
    });

    // Client → server: the port's new name and mode.
    registry.on_packet(CHANNEL, |ev, srv| {
        let Ok(edit) = serde_json::from_slice::<PortEdit>(&ev.payload) else { return };
        let (x, y, z) = edit.pos;
        let pos = BlockPos { x, y, z };
        let Some(state) = srv.get_block_state(&edit.dimension, pos) else { return };
        if !is_port_block(state.split('[').next().unwrap_or(&state)) {
            return;
        }
        write_label(srv, &edit.dimension, pos, &edit.label);
        srv.set_block_state(&edit.dimension, pos, block_for_dir(edit.mode));
    });

    // Right-click: edit it.
    registry.on_use_block(|e, phase, srv| -> bool {
        if phase != EventPhase::Pre || !is_port_block(&e.block_id) {
            return true;
        }
        let edit = PortEdit {
            dimension: e.dimension.clone(),
            pos: (e.pos.x, e.pos.y, e.pos.z),
            label: read_label(srv, &e.dimension, e.pos).unwrap_or_default(),
            mode: dir_for_block(&e.block_id).unwrap_or(PortMode::Input),
        };
        if let Ok(json) = serde_json::to_string(&edit) {
            srv.send_to_player(&e.player_name, CHANNEL, json.as_bytes());
        }
        false
    });
}
