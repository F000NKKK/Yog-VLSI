//! The Display's screen (`display.yui`, this file is its code-behind):
//! right-click a Display (any block of its picture) to set its size and
//! first bus channel.

yog_api::ui_include!("ui/display");

use yog_api::Schema;
use yog_api::{BlockPos, EventPhase, Registry};
use yog_kit::prelude::*;

use crate::content::Blocks;
use crate::display::{self, CHANNELS};
use crate::res::Strings;

/// Server → client: the Display to edit. Client → server: what it became.
const CHANNEL: &str = "yog-vlsi:display";

#[derive(Debug, Clone, Schema)]
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
            let out = DisplayEdit {
                size,
                channel: self.first(),
                ..edit.clone()
            };
            let bytes = yog_api::yog_data::to_bytes(&out);
            yog_api::event::net::send_to_server(CHANNEL, &bytes);
        }
        close();
    }
}

pub fn setup(registry: &mut Registry) {
    let screen = register(registry);

    registry.on_client_packet(CHANNEL, move |ev| {
        if let Ok(edit) = yog_api::yog_data::read_bytes::<DisplayEdit>(&ev.payload) {
            screen.update(move |vm| vm.load(edit));
            open();
        }
    });

    registry.on_packet(CHANNEL, |ev| {
        let Ok(edit) = yog_api::yog_data::read_bytes::<DisplayEdit>(&ev.payload) else {
            return;
        };
        let (x, y, z) = edit.pos;
        let pos = BlockPos { x, y, z };
        if yog_api::event::block::state(&edit.dimension, pos)
            .is_some_and(|s| s.starts_with(Blocks::DISPLAY))
            && display::configure(&edit.dimension, pos, edit.size, edit.channel).is_err()
        {
            yog_api::event::chat::actionbar(&ev.player, Strings::display_no_room(edit.size));
        }
    });

    registry.on::<yog_api::event::block::Use, _>(|e, cx| -> bool {
        let phase = cx.phase();
        // The Display or any block of its picture.
        if phase != EventPhase::Pre
            || (e.block_id != Blocks::DISPLAY && e.block_id != Blocks::DISPLAY_PART)
        {
            return true;
        }
        let Some(at) = display::owner(&e.dimension, e.pos) else {
            return true;
        };
        let Some(d) = display::get(&e.dimension, at) else {
            return true;
        };
        let edit = DisplayEdit {
            dimension: e.dimension.clone(),
            pos: (at.x, at.y, at.z),
            size: d.size,
            channel: d.channel,
        };
        let bytes = yog_api::yog_data::to_bytes(&edit);
        yog_api::event::net::send_to_player(&e.player_name, CHANNEL, &bytes);
        false
    });
}
