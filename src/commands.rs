//! Commands for Yog VLSI — chips and debugging. Designs are made in the
//! workbench, on Blueprints (see `workbench.rs`), not by command.
//!
//! /vlsi                            — help
//! /vlsi chip <tier>                — give blank chip
//! /vlsi info                       — show held chip NBT
//! /vlsi test <tier>                — create programmed test chip
//! /vlsi vm step                    — run the held chip's circuit a second

use yog_api::player::Player;
use yog_api::Registry;

use crate::chip::{ChipMeta, CircuitBlock, CircuitData, Port};
use crate::tier::Tier;
use crate::vm::PortMode;

// ── Registration ────────────────────────────────────────────────────────────

pub fn register(registry: &mut Registry) {
    // ── /vlsi ──────────────────────────────────────────────────────────────
    registry.on_command("vlsi", |_ctx, _srv| {
        Some(format!(
            "§6Yog VLSI §7— Very Large Scale Integration\n\
             §7Designs live on Blueprints: put one in a VLSI Workbench.\n\
             §7/vlsi save                 §f— save the circuit you're editing and return\n\
             §7/vlsi chip <tier>          §f— give a blank chip\n\
             §7/vlsi info                 §f— show the held chip\n\
             §7/vlsi test <tier>          §f— make a test chip\n\
             §7/vlsi vm step              §f— run the held chip's circuit a second"
        ))
    });

    // ── /vlsi chip <tier> ──────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word", |ctx, _srv| {
        let sub = ctx.arg_str(0).unwrap_or("");
        if sub == "chip" { Some("§cUsage: /vlsi chip <tier>".into()) } else { None }
    });

    registry.on_typed_command("vlsi", "word word", |ctx, srv| {
        let sub = ctx.arg_str(0).unwrap_or("");
        if sub != "chip" { return None; }
        let tier = match Tier::parse(ctx.arg_str(1).unwrap_or("")) {
            Some(t) => t, None => return Some("§cUnknown tier.".into()),
        };
        let item_id = tier.chip_item();
        Player::new(srv, &ctx.source).give(item_id, 1);
        Some(format!("§aGiven 1× {} Microchip.", tier.name()))
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

    // ── /vlsi test <tier> ──────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word word", |ctx, srv| {
        let sub = ctx.arg_str(0).unwrap_or("");
        if sub != "test" { return None; }
        let tier = match Tier::parse(ctx.arg_str(1).unwrap_or("")) {
            Some(t) => t, None => return Some("§cUnknown tier.".into()),
        };
        let meta = create_test_chip(tier, srv);
        let item_id = tier.chip_item();
        Player::new(srv, &ctx.source).give(item_id, 1);
        let _ = srv.set_held_item_nbt(&ctx.source, &meta.to_nbt());
        Some(format!("§aCreated test chip '{}'.", meta.name))
    });

    // ── /vlsi vm step ──────────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "vm" || ctx.arg_str(1).unwrap_or("") != "step" { return None; }
        let nbt = match srv.get_held_item_nbt(&ctx.source) {
            Some(n) => n, None => return Some("§cNo NBT.".into()),
        };
        let meta = match ChipMeta::from_nbt(&nbt) {
            Some(m) => m, None => return Some("§cNo VLSI chip data.".into()),
        };
        let Some(circuit) = load_circuit(srv, &meta.id) else { return Some("§cThis chip's circuit is missing.".into()) };
        let mut vm = circuit.vm();
        for _ in 0..20 {
            vm.step();
        }
        let outs: Vec<String> = circuit
            .ports
            .iter()
            .filter(|p| p.dir.outputs())
            .map(|p| format!("§e{}§7: §c{}", p.label, vm.get_port_output(p.x, p.y, p.z)))
            .collect();
        let out_str = if outs.is_empty() { "§7(none)".into() } else { outs.join(", ") };
        Some(format!("§6After 20 steps, inputs at 0:\n§7{}", out_str))
    });
}

// ── Test chip factory ──────────────────────────────────────────────────────

fn create_test_chip(tier: Tier, srv: &dyn yog_api::Server) -> ChipMeta {
    let size = tier.world_size();
    let block = |x, state: &str| CircuitBlock { x, y: 0, z: 0, state: state.into(), items: Vec::new(), redstone_port: None };
    let mut blocks = vec![block(1, "minecraft:redstone_block")];
    for x in 2..size.saturating_sub(1) {
        blocks.push(block(x, "minecraft:redstone_wire"));
    }
    if size > 2 {
        blocks.push(block(size - 2, "minecraft:redstone_lamp[lit=false]"));
    }
    let port = |label: &str, x, dir| Port { label: label.into(), x, y: 0, z: 0, dir };
    let circuit = CircuitData {
        size,
        blocks,
        ports: vec![port("IN", 0, PortMode::Input), port("OUT", size - 1, PortMode::Output)],
        alus: Vec::new(),
    };
    let id = crate::chip::new_id();
    circuit.save_for_chip(&srv.game_dir(), &id);
    ChipMeta { id, tier, name: format!("Test {}", tier.name()), ports: circuit.ports }
}

// ── Storage helpers ─────────────────────────────────────────────────────────

pub fn load_circuit(srv: &dyn yog_api::Server, chip_id: &str) -> Option<CircuitData> {
    CircuitData::load_for_chip(&srv.game_dir(), chip_id)
}
