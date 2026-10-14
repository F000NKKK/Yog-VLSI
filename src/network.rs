//! Client ↔ server networking of the ALU editor (the workbench has its own,
//! in `workbench.rs`).
//!
//! A screen's commands run client-side, so anything that changes
//! server-authoritative state (installing a chip, …) goes to the server as
//! a request over a packet channel.
//!
//! Payload format is a plain `\x1f`-separated string (consistent with the
//! rest of this mod's preference for simple text over a binary codec).

use yog_api::{Registry, Server};

pub const ALU_CHANNEL: &str = "yog-vlsi:alu_action";
pub const OPEN_UI_CHANNEL: &str = "yog-vlsi:open_ui";

const SEP: char = '\u{1f}';

/// Send an ALU GUI action to the server.
pub fn send_alu_action(parts: &[&str]) {
    if let Some(srv) = yog_api::server() {
        let payload = parts.join(&SEP.to_string());
        srv.send_to_server(ALU_CHANNEL, payload.as_bytes());
    }
}

/// Ask the server for the requesting player's programmed chips, so the ALU
/// GUI's "+ Add Chip" selector has something to list.
pub fn request_chip_list() {
    send_alu_action(&["list_chips"]);
}

/// Ask a specific player's client to open a Yog UI. Call from server-side
/// code (e.g. an `on_use_block` handler): `yog_api::open_ui()` only actually
/// opens a screen when called from client-side code — the loader invokes
/// `on_use_block`/`on_use_item` server-side only, so calling it directly from
/// those handlers silently no-ops. This bridges the gap with a packet the
/// client immediately turns back into a real `open_ui()` call.
pub fn open_ui_for(srv: &dyn yog_api::Server, player_name: &str, ui_id: &str) {
    srv.send_to_player(player_name, OPEN_UI_CHANNEL, ui_id.as_bytes());
}

fn split(payload: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(payload).split(SEP).map(str::to_owned).collect()
}

pub fn register(registry: &mut Registry) {
    registry.on_client_packet(OPEN_UI_CHANNEL, |ev, _srv| {
        let ui_id = String::from_utf8_lossy(&ev.payload).into_owned();
        yog_api::open_ui(&ui_id, true, false);
    });

    registry.on_packet(ALU_CHANNEL, |ev, srv| {
        let parts = split(&ev.payload);
        let player = ev.player.clone();
        match parts.first().map(String::as_str) {
            Some("install") if parts.len() >= 5 => {
                let slot: u32 = parts[1].parse().unwrap_or(u32::MAX);
                let alu_pos = (
                    parts[2].parse().unwrap_or(0),
                    parts[3].parse().unwrap_or(0),
                    parts[4].parse().unwrap_or(0),
                );
                let reply = crate::commands::install_chip_from_slot(srv, &player, slot, alu_pos);
                srv.send_actionbar(&player, &reply);
            }
            Some("list_chips") => {
                let inv = yog_api::player::Player::new(srv, &player).inventory();
                let mut lines = Vec::new();
                for (slot, item_id, _count) in inv {
                    if !item_id.starts_with("yog-vlsi:chip_") { continue; }
                    let Some((_, _, nbt)) = srv.get_slot_item(&player, slot) else { continue };
                    let Some(meta) = crate::chip::ChipMeta::from_nbt(&nbt) else { continue };
                    lines.push(format!("{}\t{}\t{}", slot, meta.tier.name(), meta.name));
                }
                srv.send_to_player(&player, crate::alu::CHIP_LIST_CHANNEL, lines.join("\n").as_bytes());
            }
            Some("save_links") => {
                srv.send_actionbar(&player, "§aLink graph saved.");
            }
            _ => {}
        }
    });
}
