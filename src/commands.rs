//! Commands for Yog VLSI. Designs are made in the workbench, on Blueprints
//! (see `workbench.rs`), not by command.
//!
//! /vlsi                            — help
//! /vlsi info                       — show the held chip
//! /vlsi save                       — leave the editor (`editor.rs`)

use yog_api::Registry;

use crate::chip::ChipMeta;
use crate::res::Strings;

// ── Registration ────────────────────────────────────────────────────────────

pub fn register(registry: &mut Registry) {
    // ── /vlsi ──────────────────────────────────────────────────────────────
    registry.on_command("vlsi", |_ctx| Some(Strings::command_help()));

    // ── /vlsi info ─────────────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word", |ctx| {
        if ctx.arg_str(0).unwrap_or("") != "info" {
            return None;
        }
        let nbt = match yog_api::event::player::held_item_nbt(&ctx.source) {
            Some(n) => n,
            None => return Some(Strings::command_no_nbt()),
        };
        match ChipMeta::from_nbt(&nbt) {
            Some(meta) => {
                let ports = if meta.ports.is_empty() {
                    Strings::command_no_ports()
                } else {
                    meta.ports
                        .iter()
                        .map(|p| Strings::command_port(&p.label, p.dir.label(), p.x, p.y, p.z))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                Some(Strings::command_chip(
                    &meta.name,
                    &meta.id,
                    meta.tier.name(),
                    ports,
                ))
            }
            None => Some(Strings::command_no_chip()),
        }
    });
}
