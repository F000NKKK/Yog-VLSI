//! The Display's screen (`display.yui`, this file is its code-behind):
//! right-click a Display to set its size and first bus channel.

yog_api::ui_include!("ui/display");

use serde::{Deserialize, Serialize};
use yog_api::{BlockPos, EventPhase, Registry, Server};
use yog_kit::prelude::*;

use crate::content::Blocks;
use crate::display::{self, CHANNELS};
use crate::res::Strings;

/// Server → client: the Display to edit. Client → server: what it became.
const CHANNEL: &str = "yog-vlsi:display";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DisplayEdit {
    dimension: String,
    pos: (i32, i32, i32),
    size: u16,
    channel: u16,
}

#[derive(Default)]
pub struct DisplayVm {
    edit: Option<DisplayEdit>,
    size: String,
    channel: String,
}

impl ViewModel for DisplayVm {}

impl DisplayVm {
    fn load(&mut self, edit: DisplayEdit) {
        self.size = edit.size.to_string();
        self.channel = edit.channel.to_string();
        self.edit = Some(edit);
    }
    fn first(&self) -> u16 {
        self.channel.trim().parse::<u16>().unwrap_or(1).max(1)
    }
    /// What the channels from the first carry.
    pub fn channels(&self) -> String {
        let first = self.first();
        Strings::screen_display_channels(first, first + CHANNELS - 1)
    }
    pub fn done(&mut self) {
        if let Some(edit) = &self.edit {
            let size = self.size.trim().parse::<u16>().unwrap_or(edit.size);
            let out = DisplayEdit { size, channel: self.first(), ..edit.clone() };
            if let (Some(srv), Ok(json)) = (yog_api::server(), serde_json::to_string(&out)) {
                srv.send_to_server(CHANNEL, json.as_bytes());
            }
        }
        close();
    }
}

pub fn setup(registry: &mut Registry) {
    let screen = register(registry);

    registry.on_client_packet(CHANNEL, move |ev, _srv| {
        if let Ok(edit) = serde_json::from_slice::<DisplayEdit>(&ev.payload) {
            screen.update(move |vm| vm.load(edit));
            open();
        }
    });

    registry.on_packet(CHANNEL, |ev, srv| {
        let Ok(edit) = serde_json::from_slice::<DisplayEdit>(&ev.payload) else { return };
        let (x, y, z) = edit.pos;
        let pos = BlockPos { x, y, z };
        if srv.get_block_state(&edit.dimension, pos).is_some_and(|s| s.starts_with(Blocks::DISPLAY)) {
            display::configure(&edit.dimension, pos, edit.size, edit.channel);
        }
    });

    registry.on_use_block(|e, phase, srv| -> bool {
        if phase != EventPhase::Pre || e.block_id != Blocks::DISPLAY {
            return true;
        }
        let Some(d) = display::get(&e.dimension, e.pos) else { return true };
        let edit = DisplayEdit { dimension: e.dimension.clone(), pos: (e.pos.x, e.pos.y, e.pos.z), size: d.size, channel: d.channel };
        if let Ok(json) = serde_json::to_string(&edit) {
            srv.send_to_player(&e.player_name, CHANNEL, json.as_bytes());
        }
        false
    });
}
