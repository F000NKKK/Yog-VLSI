//! Commands for Yog VLSI — ALU and debugging. Designs are made in the
//! workbench, on Blueprints (see `workbench.rs`), not by command.
//!
//! /vlsi                            — help
//! /vlsi chip <tier>                — give blank chip
//! /vlsi info                       — show held chip NBT
//! /vlsi test <tier>                — create programmed test chip
//! /vlsi vm step                    — step VM on held chip
//! /vlsi alu install                — install held chip into nearest ALU
//! /vlsi alu list                   — list chips in nearest ALU

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::player::Player;
use yog_api::Registry;

use crate::chip::{ChipMeta, CircuitBlock, CircuitData, Port};
use crate::tier::Tier;
use crate::vm::{PortMode, RedstoneVM};

/// In-memory ALU state: (x, y, z) → list of (chip_id, tier)
pub static ALU_STATE: LazyLock<Mutex<HashMap<(i32, i32, i32), Vec<(String, Tier)>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// In-memory chip-VM cache: chip_id → RedstoneVM
pub static VM_CACHE: LazyLock<Mutex<HashMap<String, RedstoneVM>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Port list per installed chip (cached at install time so the tick loop
/// doesn't need a Storage read every tick): chip_id → ports.
pub static CHIP_PORTS: LazyLock<Mutex<HashMap<String, Vec<Port>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Server-authoritative link graph, keyed like the UI shows it:
/// `(chip_id, port_label) → (chip_id, port_label)`. An ALU's own 6 external
/// ports are addressed with the chip_id `"__ext_<x>_<y>_<z>"` and a label of
/// `"N"`/`"S"`/`"E"`/`"W"`/`"U"`/`"D"`, so external ↔ chip links live in the
/// same map as chip ↔ chip links.
pub static LINKS: LazyLock<Mutex<HashMap<(String, String), (String, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// I/O mode per ALU external port: `"__ext_<x>_<y>_<z>:<side>"` → mode string.
pub static IO_MODES: LazyLock<Mutex<HashMap<String, crate::vm::PortMode>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Latest computed value for each ALU's external analog ports (what a
/// Redstone Port *would* drive onto its Analog Cable). Actually asserting
/// this onto real-world redstone needs a loader-level redstone-power R/W
/// hook that does not exist yet — see `alu::tick_all` for details.
pub static EXT_VALUES: LazyLock<Mutex<HashMap<(i32, i32, i32), HashMap<String, u8>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Display name per installed chip (for the ALU node editor): chip_id → name.
pub static CHIP_NAMES: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Each ALU's dimension, by position — recorded when it's opened (its
/// cables are read and driven there); saved with the ALU state.
pub static ALU_DIMS: LazyLock<Mutex<HashMap<(i32, i32, i32), String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Each ALU's tier, by position — recorded when it's opened (its chip limit).
pub static ALU_TIERS: LazyLock<Mutex<HashMap<(i32, i32, i32), Tier>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn ext_chip_id(alu_pos: (i32, i32, i32)) -> String {
    format!("__ext_{}_{}_{}", alu_pos.0, alu_pos.1, alu_pos.2)
}

/// Install a chip (by its item NBT) into the ALU at `alu_pos`, preloading its
/// VM and port cache. Shared by `/vlsi alu install` and the ALU GUI's
/// network-driven chip installer.
pub fn install_chip(srv: &dyn yog_api::Server, meta: &ChipMeta, alu_pos: (i32, i32, i32)) {
    ALU_STATE.lock().unwrap().entry(alu_pos).or_default().push((meta.id.clone(), meta.tier));
    CHIP_NAMES.lock().unwrap().insert(meta.id.clone(), meta.name.clone());
    if let Some(circuit) = load_circuit(srv, &meta.id) {
        VM_CACHE.lock().unwrap().insert(meta.id.clone(), circuit.vm());
        CHIP_PORTS.lock().unwrap().insert(meta.id.clone(), circuit.ports);
    }
}

/// Install the chip sitting in the player's inventory `slot` into the ALU at
/// `alu_pos`, clearing that slot. Used by the ALU GUI's chip selector, which
/// has to round-trip through a packet since inventory access is a
/// server-authoritative call unavailable from client-side UI code.
pub fn install_chip_from_slot(srv: &dyn yog_api::Server, player_name: &str, slot: u32, alu_pos: (i32, i32, i32)) -> String {
    let Some((item_id, _count, nbt)) = srv.get_slot_item(player_name, slot) else {
        return "§cInvalid slot.".into();
    };
    if !item_id.starts_with("yog-vlsi:chip_") {
        return "§cThat slot isn't a microchip.".into();
    }
    let Some(meta) = ChipMeta::from_nbt(&nbt) else {
        return "§cThat chip hasn't been programmed with a design yet.".into();
    };
    if let Some(tier) = ALU_TIERS.lock().unwrap().get(&alu_pos).copied() {
        let installed = ALU_STATE.lock().unwrap().get(&alu_pos).map_or(0, Vec::len);
        if installed >= tier.max_chips() {
            return format!("§c{}", crate::res::Strings::screen_alu_full(tier.max_chips()));
        }
    }
    install_chip(srv, &meta, alu_pos);
    Player::new(srv, player_name).set_slot(slot, "minecraft:air", 0);
    format!("§aInstalled '{}' into the ALU.", meta.name)
}

// ── Registration ────────────────────────────────────────────────────────────

pub fn register(registry: &mut Registry) {
    // ── /vlsi ──────────────────────────────────────────────────────────────
    registry.on_command("vlsi", |_ctx, _srv| {
        Some(format!(
            "§6Yog VLSI §7— Very Large Scale Integration\n\
             §7Designs live on Blueprints: put one in a VLSI Workbench.\n\
             §7/vlsi save                 §f— save the circuit you're editing and return\n\
             §7/vlsi port <name>          §f— name the port you last right-clicked\n\
             §7/vlsi alu install          §f— install the held chip into an ALU\n\
             §7/vlsi alu list             §f— list an ALU's chips\n\
             §7/vlsi chip <tier>          §f— give a blank chip\n\
             §7/vlsi info                 §f— show the held chip\n\
             §7/vlsi test <tier>          §f— make a test chip\n\
             §7/vlsi vm step              §f— step the held chip's VM"
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
        let mut cache = VM_CACHE.lock().unwrap();
        let vm = cache.entry(meta.id.clone()).or_insert_with(|| {
            load_circuit(srv, &meta.id).map_or_else(|| RedstoneVM::new(meta.tier.world_size()), |c| c.vm())
        });
        let before = vm.tick;
        vm.step();
        let outputs = vm.read_outputs(0);
        let out_str = if outputs.is_empty() { "§7(none)".into() }
        else { outputs.iter().map(|(x,z,p)| format!("({},{}): §c{}", x, z, p)).collect::<Vec<_>>().join(", ") };
        Some(format!("§6VM Step §f{}→{}\n§7Outputs: {}", before, vm.tick, out_str))
    });

    // ── /vlsi alu install ──────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "alu" || ctx.arg_str(1).unwrap_or("") != "install" { return None; }
        let nbt = match srv.get_held_item_nbt(&ctx.source) {
            Some(n) => n, None => return Some("§cNo NBT on held item.".into()),
        };
        let meta = match ChipMeta::from_nbt(&nbt) {
            Some(m) => m, None => return Some("§cNo VLSI chip data.".into()),
        };
        // Install into ALU at player position (simplified: nearest ALU)
        let pos = match Player::with_uuid(srv, &ctx.source, &ctx.uuid).position() {
            Some((x, y, z)) => ((x - 1.0) as i32, y as i32, (z - 1.0) as i32),
            None => return Some("§cCannot determine position.".into()),
        };
        install_chip(srv, &meta, pos);
        let _ = srv.set_held_item_nbt(&ctx.source, "");
        Some(format!("§aInstalled '{}' into ALU at {:?}.", meta.name, pos))
    });

    // ── /vlsi alu list ─────────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "alu" || ctx.arg_str(1).unwrap_or("") != "list" { return None; }
        let pos = match Player::with_uuid(srv, &ctx.source, &ctx.uuid).position() {
            Some((x, y, z)) => ((x - 1.0) as i32, y as i32, (z - 1.0) as i32),
            None => return Some("§cCannot determine position.".into()),
        };
        let state = ALU_STATE.lock().unwrap();
        if let Some(chips) = state.get(&pos) {
            let lines: Vec<String> = chips.iter()
                .map(|(id, tier)| format!("§e{}§7 ({})", &id[..8], tier.name()))
                .collect();
            Some(format!("§6ALU chips ({}):\n{}", chips.len(), lines.join("\n")))
        } else {
            Some("§7No chips in nearby ALU.".into())
        }
    });
}

// ── Test chip factory ──────────────────────────────────────────────────────

fn create_test_chip(tier: Tier, srv: &dyn yog_api::Server) -> ChipMeta {
    let size = tier.world_size();
    let block = |x, state: &str| CircuitBlock { x, y: 0, z: 0, state: state.into() };
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
    };
    let id = crate::chip::new_id();
    circuit.save_for_chip(&srv.game_dir(), &id);
    ChipMeta { id, tier, name: format!("Test {}", tier.name()), ports: circuit.ports }
}

// ── Storage helpers ─────────────────────────────────────────────────────────

pub fn load_circuit(srv: &dyn yog_api::Server, chip_id: &str) -> Option<CircuitData> {
    CircuitData::load_for_chip(&srv.game_dir(), chip_id)
}
