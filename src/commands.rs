//! Commands for Yog VLSI. Designs are made in the workbench, on Blueprints
//! (see `workbench.rs`), not by command.
//!
//! /vlsi                            — help
//! /vlsi info                       — show the held chip
//! /vlsi save                       — leave the editor (`editor.rs`)

use yog_api::Registry;

use crate::chip::ChipMeta;

// ── Registration ────────────────────────────────────────────────────────────

pub fn register(registry: &mut Registry) {
    // ── /vlsi ──────────────────────────────────────────────────────────────
    registry.on_command("vlsi", |_ctx, _srv| {
        Some(
            "§6Yog VLSI §7— Very Large Scale Integration\n\
             §7Designs live on Blueprints: put one in a VLSI Workbench.\n\
             §7/vlsi save                 §f— save the circuit you're editing and return\n\
             §7/vlsi info                 §f— show the held chip"
                .into(),
        )
    });

    // ── /vlsi info ─────────────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "info" { return None; }
        let nbt = match srv.get_held_item_nbt(&ctx.source) {
            Some(n) => n, None => return Some("§cNo NBT on held item.".into()),
        };
        match ChipMeta::from_nbt(&nbt) {
            Some(meta) => {
                let ports = if meta.ports.is_empty() { "§7(none)".into() }
                else {
                    meta.ports.iter()
                        .map(|p| format!("  §e{}§7: {} ({}, {}, {})", p.label, p.dir.label(), p.x, p.y, p.z))
                        .collect::<Vec<_>>().join("\n")
                };
                Some(format!("§6Chip: §f{}\n§7ID: §f{}\n§7Tier: §e{}\n§7Ports:\n{}", meta.name, meta.id, meta.tier.name(), ports))
            }
            None => Some("§cNo VLSI chip data.".into()),
        }
    });
}
