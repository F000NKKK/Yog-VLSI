//! Port screens (`port.yui`, this file is its code-behind): right-click a
//! port to name it and pick its mode.
//!
//! - **VLSI Port** — what players place inside the circuit editor to give a
//!   design its pins. The mode (Input/Output/Bidirectional) is which of the
//!   three block variants stands there; the name is kept here, per
//!   position. Saving the design (`/vlsi save`) turns every port in the
//!   cube into a pin of that name.
//! - **Redstone Port** — joins an analog cable to vanilla redstone
//!   (`cables.rs`): Input or Output, and the name the ALU screen shows on
//!   the side whose cable reaches it.

yog_api::ui_include!("ui/port");

use std::collections::HashMap;
use yog_api::sync::{LazyLock, Mutex};

use yog_api::Schema;
use yog_api::{BlockPos, EventPhase, Registry};
use yog_kit::prelude::*;

use crate::cables::{self, RedstonePort};
use crate::content::Blocks;
use crate::res::Strings;
use crate::vm::PortMode;

pub const PORT_INPUT: &str = Blocks::PORT_INPUT;
pub const PORT_OUTPUT: &str = Blocks::PORT_OUTPUT;
pub const PORT_BIDI: &str = Blocks::PORT_BIDI;

/// Server → client: the port to edit. Client → server: what it became.
const CHANNEL: &str = "yog-vlsi:port";

/// Every named port: (dimension, x, y, z) → name.
static LABELS: LazyLock<Mutex<HashMap<crate::cables::Key, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
pub fn read_label(dimension: &str, pos: BlockPos) -> Option<String> {
    LABELS.lock().get(&key(dimension, pos)).cloned()
}

/// Names the port at `pos` (an empty name forgets it).
pub fn write_label(dimension: &str, pos: BlockPos, label: &str) {
    let mut labels = LABELS.lock();
    if label.trim().is_empty() {
        labels.remove(&key(dimension, pos));
    } else {
        labels.insert(key(dimension, pos), label.trim().to_string());
    }
}

/// One port being edited.
#[derive(Debug, Clone, Schema)]
struct PortEdit {
    dimension: String,
    pos: (i32, i32, i32),
    label: String,
    mode: PortMode,
    /// A Redstone Port (Input or Output only), not a VLSI Port.
    redstone: bool,
    /// A Redstone Port's bus channel (0: on an analog cable).
    #[data(default)]
    channel: u16,
}

// ── The screen's view model ──────────────────────────────────────────────────

pub struct PortVm {
    edit: Option<PortEdit>,
    label: String,
    mode: PortMode,
    /// A Redstone Port's bus channel, as typed (empty: an analog cable).
    channel: String,
}

impl Default for PortVm {
    fn default() -> Self {
        Self {
            edit: None,
            label: String::new(),
            mode: PortMode::Input,
            channel: String::new(),
        }
    }
}

impl ViewModel for PortVm {}

impl PortVm {
    fn load(&mut self, edit: PortEdit) {
        self.label = edit.label.clone();
        self.mode = edit.mode;
        self.channel = if edit.channel == 0 {
            String::new()
        } else {
            edit.channel.to_string()
        };
        self.edit = Some(edit);
    }
    pub fn title(&self) -> String {
        if self.chip_port() {
            Strings::screen_port_title()
        } else {
            Strings::screen_port_redstone_title()
        }
    }
    /// A VLSI Port: it can be bidirectional too.
    pub fn chip_port(&self) -> bool {
        !self.edit.as_ref().is_some_and(|e| e.redstone)
    }
    pub fn set_mode(&mut self, mode: PortMode) {
        self.mode = mode;
    }
    pub fn done(&mut self) {
        if let Some(edit) = &self.edit {
            let channel = self
                .channel
                .trim()
                .parse::<u16>()
                .map_or(0, |c| c.min(cables::BUS_CHANNELS));
            let out = PortEdit {
                label: self.label.trim().to_string(),
                mode: self.mode,
                channel,
                ..edit.clone()
            };
            let bytes = yog_api::yog_data::to_bytes(&out);
            yog_api::event::net::send_to_server(CHANNEL, &bytes);
        }
        close();
    }
}

// ── The server side ──────────────────────────────────────────────────────────

/// Where it's kept in the world store.
const STORE_KEY: &str = "port_labels";

pub fn load() {
    let saved: Vec<((String, i32, i32, i32), String)> = crate::saved::load(STORE_KEY);
    *LABELS.lock() = saved.into_iter().collect();
}

pub fn save() {
    let labels: Vec<_> = LABELS
        .lock()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    crate::saved::save(STORE_KEY, &labels);
}

pub fn setup(registry: &mut Registry) {
    let screen = register(registry);

    // Server → client: open the screen on this port.
    registry.on_client_packet(CHANNEL, move |ev| {
        if let Ok(edit) = yog_api::yog_data::read_bytes::<PortEdit>(&ev.payload) {
            screen.update(move |vm| vm.load(edit));
            open();
        }
    });

    // Client → server: the port's new name and mode.
    registry.on_packet(CHANNEL, |ev| {
        let Ok(edit) = yog_api::yog_data::read_bytes::<PortEdit>(&ev.payload) else {
            return;
        };
        let (x, y, z) = edit.pos;
        let pos = BlockPos { x, y, z };
        let Some(state) = yog_api::event::block::state(&edit.dimension, pos) else {
            return;
        };
        let id = state.split('[').next().unwrap_or(&state);
        if edit.redstone && id == Blocks::REDSTONE_PORT {
            let port = RedstonePort {
                output: edit.mode == PortMode::Output,
                name: edit.label.trim().to_string(),
                channel: edit.channel,
            };
            cables::set_redstone_port(&edit.dimension, pos, port);
        } else if !edit.redstone && is_port_block(id) {
            write_label(&edit.dimension, pos, &edit.label);
            yog_api::event::block::set_state(&edit.dimension, pos, block_for_dir(edit.mode));
        }
    });

    // Right-click: edit it.
    registry.on::<yog_api::event::block::Use, _>(|e, cx| -> bool {
        let phase = cx.phase();
        if phase != EventPhase::Pre {
            return true;
        }
        let at = (e.pos.x, e.pos.y, e.pos.z);
        let edit = if is_port_block(&e.block_id) {
            PortEdit {
                dimension: e.dimension.clone(),
                pos: at,
                label: read_label(&e.dimension, e.pos).unwrap_or_default(),
                mode: dir_for_block(&e.block_id).unwrap_or(PortMode::Input),
                redstone: false,
                channel: 0,
            }
        } else if e.block_id == Blocks::REDSTONE_PORT {
            let port = cables::redstone_port(&e.dimension, e.pos);
            PortEdit {
                dimension: e.dimension.clone(),
                pos: at,
                label: port.name,
                mode: if port.output {
                    PortMode::Output
                } else {
                    PortMode::Input
                },
                redstone: true,
                channel: port.channel,
            }
        } else {
            return true;
        };
        let bytes = yog_api::yog_data::to_bytes(&edit);
        yog_api::event::net::send_to_player(&e.player_name, CHANNEL, &bytes);
        false
    });
}
