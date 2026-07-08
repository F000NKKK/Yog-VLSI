//! Commands for Yog VLSI — design management, fabrication, blueprint, ALU, debug.
//!
//! /vlsi                            — help
//! /vlsi chip <tier>                — give blank chip
//! /vlsi info                       — show held chip NBT
//! /vlsi test <tier>                — create programmed test chip
//! /vlsi vm step                    — step VM on held chip
//! /vlsi designs                    — list saved designs
//! /vlsi designs create <name> <tier> — create blank design
//! /vlsi designs delete <name>      — delete design
//! /vlsi fabricate <design> <tier>  — print chip from design (at workbench)
//! /vlsi blueprint export <design>  — export design to Blueprint item
//! /vlsi alu install                — install held chip into nearest ALU
//! /vlsi alu list                   — list chips in nearest ALU

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::player::Player;
use yog_api::{Registry, Storage};

use crate::chip::{ChipMeta, CircuitBlock, CircuitData, Port, PortDir, PortSide};
use crate::designs;
use crate::vm::{BlockType, ComparatorMode, Facing, PortMode, RedstoneVM, Tier};
use crate::workbench::BLUEPRINT_ID;

/// In-memory ALU state: (x, y, z) → list of (chip_id, tier)
pub static ALU_STATE: LazyLock<Mutex<HashMap<(i32, i32, i32), Vec<(String, Tier)>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// In-memory chip-VM cache: chip_id → RedstoneVM
pub static VM_CACHE: LazyLock<Mutex<HashMap<String, RedstoneVM>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ── Tier helper ──────────────────────────────────────────────────────────────

fn parse_tier(s: &str) -> Option<Tier> {
    match s.to_lowercase().as_str() {
        "wood" => Some(Tier::Wood),
        "stone" => Some(Tier::Stone),
        "gold" => Some(Tier::Gold),
        "iron" => Some(Tier::Iron),
        "diamond" => Some(Tier::Diamond),
        "netherite" => Some(Tier::Netherite),
        _ => None,
    }
}

// ── Registration ────────────────────────────────────────────────────────────

pub fn register(registry: &mut Registry) {
    // ── /vlsi ──────────────────────────────────────────────────────────────
    registry.on_command("vlsi", |_ctx, _srv| {
        Some(format!(
            "§6Yog VLSI §7— Very Large Scale Integration\n\
             §7/vlsi chip <tier>          §f— give blank chip\n\
             §7/vlsi info                 §f— show held chip\n\
             §7/vlsi test <tier>          §f— create test chip\n\
             §7/vlsi vm step              §f— step VM on held chip\n\
             §7/vlsi designs              §f— list saved designs\n\
             §7/vlsi designs create <n> <t> §f— create design\n\
             §7/vlsi designs delete <n>   §f— delete design\n\
             §7/vlsi fabricate <name> <t> §f— print chip from design\n\
             §7/vlsi blueprint export <n> §f— export design to Blueprint\n\
             §7/vlsi alu install          §f— install held chip into ALU\n\
             §7/vlsi alu list             §f— list ALU chips"
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
        let tier = match parse_tier(ctx.arg_str(1).unwrap_or("")) {
            Some(t) => t, None => return Some("§cUnknown tier.".into()),
        };
        let item_id = format!("yog-vlsi:chip_{}", tier.id());
        Player::new(srv, &ctx.source).give(&item_id, 1);
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
                        .map(|p| format!("  §e{}§7: {} {} idx={}", p.label, p.dir.name(), p.side.name(), p.index))
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
        let tier = match parse_tier(ctx.arg_str(1).unwrap_or("")) {
            Some(t) => t, None => return Some("§cUnknown tier.".into()),
        };
        let meta = create_test_chip(tier, srv);
        let item_id = format!("yog-vlsi:chip_{}", tier.id());
        Player::new(srv, &ctx.source).give(&item_id, 1);
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
            let mut vm = RedstoneVM::new(meta.tier);
            if let Some(circuit) = load_circuit(srv, &meta.id) {
                load_circuit_into_vm(&mut vm, &circuit);
            }
            vm
        });
        let before = vm.tick;
        vm.step();
        let outputs = vm.read_outputs(0);
        let out_str = if outputs.is_empty() { "§7(none)".into() }
        else { outputs.iter().map(|(x,z,p)| format!("({},{}): §c{}", x, z, p)).collect::<Vec<_>>().join(", ") };
        Some(format!("§6VM Step §f{}→{}\n§7Outputs: {}", before, vm.tick, out_str))
    });

    // ── /vlsi designs ──────────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "designs" { return None; }
        let game_dir = srv.game_dir();
        let list = designs::list_designs(&game_dir, &ctx.source);
        if list.is_empty() { return Some("§7No saved designs.".into()); }
        let lines: Vec<String> = list.iter().map(|d|
            format!("§e{}§7 [{}] {} ports — {}",
                d.name, d.tier.name(), d.port_count, d.id))
            .collect();
        Some(format!("§6Designs ({}):\n{}", list.len(), lines.join("\n")))
    });

    // ── /vlsi designs create <name> <tier> ─────────────────────────────────
    registry.on_typed_command("vlsi", "word word word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "designs" || ctx.arg_str(1).unwrap_or("") != "create" { return None; }
        let name = ctx.arg_str(2).unwrap_or("Unnamed");
        let tier = match parse_tier(ctx.arg_str(3).unwrap_or("")) {
            Some(t) => t, None => return Some("§cUsage: /vlsi designs create <name> <tier>".into()),
        };
        let game_dir = srv.game_dir();
        let id = designs::create_design(&game_dir, &ctx.source, name, tier);
        Some(format!("§aDesign '{}' created ({} tier, ID: {}).", name, tier.name(), &id[..8]))
    });

    // ── /vlsi designs delete <name> ────────────────────────────────────────
    registry.on_typed_command("vlsi", "word word word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "designs" || ctx.arg_str(1).unwrap_or("") != "delete" { return None; }
        let name = ctx.arg_str(2).unwrap_or("");
        let game_dir = srv.game_dir();
        let list = designs::list_designs(&game_dir, &ctx.source);
        if let Some(d) = list.iter().find(|d| d.name == name) {
            designs::delete_design(&game_dir, &ctx.source, &d.id);
            Some(format!("§aDesign '{}' deleted.", name))
        } else {
            Some(format!("§cDesign '{}' not found.", name))
        }
    });

    // ── /vlsi fabricate <design_name> <tier> ────────────────────────────────
    registry.on_typed_command("vlsi", "word word word", |ctx, srv| {
        let sub = ctx.arg_str(0).unwrap_or("");
        let action = ctx.arg_str(1).unwrap_or("");
        if sub != "fabricate" { return None; }
        let design_name = action;
        let tier = match parse_tier(ctx.arg_str(2).unwrap_or("")) {
            Some(t) => t, None => return Some("§cUsage: /vlsi fabricate <design_name> <tier>".into()),
        };
        let game_dir = srv.game_dir();
        let list = designs::list_designs(&game_dir, &ctx.source);
        let design = match list.iter().find(|d| d.name == design_name) {
            Some(d) => d.clone(),
            None => return Some(format!("§cDesign '{}' not found.", design_name)),
        };
        let entry = match designs::load_design(&game_dir, &ctx.source, &design.id) {
            Some(e) => e,
            None => return Some("§cFailed to load design data.".into()),
        };

        // Calculate resource cost
        let cost = calculate_cost(&entry.circuit.blocks);
        let cost_str: Vec<String> = cost.iter()
            .map(|(item, qty)| format!("{}: {}", item, qty))
            .collect();

        // Create the chip
        let meta = ChipMeta {
            id: crate::chip::new_chip_id(),
            tier,
            name: design_name.to_string(),
            ports: entry.circuit.ports.clone(),
        };
        save_circuit(srv, &entry.circuit);

        let item_id = format!("yog-vlsi:chip_{}", tier.id());
        Player::new(srv, &ctx.source).give(&item_id, 1);
        let _ = srv.set_held_item_nbt(&ctx.source, &meta.to_nbt());

        Some(format!(
            "§aFabricated '{}' ({} tier, {} ports).\n§7Cost: {}",
            meta.name, tier.name(), meta.ports.len(),
            if cost_str.is_empty() { "free (empty design)".into() } else { cost_str.join(", ") }
        ))
    });

    // ── /vlsi blueprint export <design_name> ───────────────────────────────
    registry.on_typed_command("vlsi", "word word word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "blueprint" || ctx.arg_str(1).unwrap_or("") != "export" { return None; }
        let design_name = ctx.arg_str(2).unwrap_or("");
        let game_dir = srv.game_dir();
        let list = designs::list_designs(&game_dir, &ctx.source);
        let design = match list.iter().find(|d| d.name == design_name) {
            Some(d) => d.clone(),
            None => return Some(format!("§cDesign '{}' not found.", design_name)),
        };
        let entry = match designs::load_design(&game_dir, &ctx.source, &design.id) {
            Some(e) => e,
            None => return Some("§cFailed to load design data.".into()),
        };

        // Create Blueprint with CircuitData in NBT
        let circuit_json = entry.circuit.to_json();
        let escaped = circuit_json.replace('\\', "\\\\").replace('"', "\\\"");
        let nbt = format!("{{YogVlsiBlueprint: \"{}\"}}", escaped);

        Player::new(srv, &ctx.source).give(BLUEPRINT_ID, 1);
        let _ = srv.set_held_item_nbt(&ctx.source, &nbt);
        Some(format!("§aBlueprint exported: '{}' ({} blocks, {} ports).",
            design_name, entry.circuit.blocks.len(), entry.circuit.ports.len()))
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
        let pos = match Player::new(srv, &ctx.source).position() {
            Some((x, y, z)) => ((x - 1.0) as i32, y as i32, (z - 1.0) as i32),
            None => return Some("§cCannot determine position.".into()),
        };
        let mut state = ALU_STATE.lock().unwrap();
        state.entry(pos).or_default().push((meta.id.clone(), meta.tier));
        // Preload VM
        if let Some(circuit) = load_circuit(srv, &meta.id) {
            let mut vm = RedstoneVM::new(meta.tier);
            load_circuit_into_vm(&mut vm, &circuit);
            VM_CACHE.lock().unwrap().insert(meta.id.clone(), vm);
        }
        let _ = srv.set_held_item_nbt(&ctx.source, "");
        Some(format!("§aInstalled '{}' into ALU at {:?}.", meta.name, pos))
    });

    // ── /vlsi alu list ─────────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "alu" || ctx.arg_str(1).unwrap_or("") != "list" { return None; }
        let pos = match Player::new(srv, &ctx.source).position() {
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
    let chip_id = crate::chip::new_chip_id();
    let mut blocks = Vec::new();

    blocks.push(CircuitBlock { x: 0, y: 0, z: 0, block_id: "yog-vlsi:port".into(), state_json: r#"{"mode":"input"}"#.into() });
    blocks.push(CircuitBlock { x: size-1, y: 0, z: 0, block_id: "yog-vlsi:port".into(), state_json: r#"{"mode":"output"}"#.into() });
    blocks.push(CircuitBlock { x: 1, y: 0, z: 0, block_id: "minecraft:redstone_block".into(), state_json: "{}".into() });
    for x in 2..size.saturating_sub(1) {
        blocks.push(CircuitBlock { x, y: 0, z: 0, block_id: "minecraft:redstone_wire".into(), state_json: "{}".into() });
    }
    if size > 2 {
        blocks.push(CircuitBlock { x: size-2, y: 0, z: 0, block_id: "minecraft:redstone_lamp".into(), state_json: r#"{"lit":"false"}"#.into() });
    }

    let ports = vec![
        Port { label: "IN".into(), side: PortSide::West, index: 0, dir: PortDir::Input },
        Port { label: "OUT".into(), side: PortSide::East, index: 0, dir: PortDir::Output },
    ];

    let circuit = CircuitData { chip_id: chip_id.clone(), width: size, height: size, blocks, ports };
    save_circuit(srv, &circuit);

    ChipMeta { id: chip_id, tier, name: format!("Test {}", tier.name()), ports: circuit.ports }
}

// ── Resource cost calculator ────────────────────────────────────────────────

/// Calculate fabrication cost at 25% vanilla rate.
fn calculate_cost(blocks: &[CircuitBlock]) -> Vec<(String, u64)> {
    let mut costs: HashMap<String, u64> = HashMap::new();
    for block in blocks {
        let recipe = block_cost(block);
        for (item, qty) in recipe {
            *costs.entry(item).or_default() += qty;
        }
    }
    // Apply 25% rate (divide by 4, ceil)
    costs.into_iter()
        .map(|(item, qty)| (item, (qty + 3) / 4))
        .filter(|(_, qty)| *qty > 0)
        .collect()
}

/// Vanilla recipe cost for a block (in items).
fn block_cost(block: &CircuitBlock) -> Vec<(String, u64)> {
    let mut c = Vec::new();
    match block.block_id.as_str() {
        "minecraft:redstone_wire" => { c.push(("minecraft:redstone".into(), 1)); }
        "minecraft:redstone_torch" | "minecraft:redstone_wall_torch" => {
            c.push(("minecraft:redstone".into(), 1));
            c.push(("minecraft:stick".into(), 1));
        }
        "minecraft:repeater" => {
            c.push(("minecraft:redstone".into(), 3));
            c.push(("minecraft:stick".into(), 2));
            c.push(("minecraft:stone".into(), 3));
        }
        "minecraft:comparator" => {
            c.push(("minecraft:redstone".into(), 3));
            c.push(("minecraft:quartz".into(), 1));
            c.push(("minecraft:stone".into(), 3));
        }
        "minecraft:redstone_lamp" => {
            c.push(("minecraft:redstone".into(), 4));
            c.push(("minecraft:glowstone_dust".into(), 1));
        }
        "minecraft:redstone_block" => { c.push(("minecraft:redstone".into(), 9)); }
        "minecraft:lever" => { c.push(("minecraft:stick".into(), 1)); c.push(("minecraft:cobblestone".into(), 1)); }
        "minecraft:stone_button" => { c.push(("minecraft:stone".into(), 1)); }
        "minecraft:observer" => {
            c.push(("minecraft:cobblestone".into(), 6));
            c.push(("minecraft:redstone".into(), 2));
            c.push(("minecraft:quartz".into(), 1));
        }
        "minecraft:piston" => {
            c.push(("minecraft:oak_planks".into(), 3));
            c.push(("minecraft:cobblestone".into(), 4));
            c.push(("minecraft:iron_ingot".into(), 1));
            c.push(("minecraft:redstone".into(), 1));
        }
        "minecraft:sticky_piston" => {
            c.push(("minecraft:piston".into(), 1)); // simplified: count as piston + slime
            c.push(("minecraft:slime_ball".into(), 1));
        }
        "minecraft:hopper" => {
            c.push(("minecraft:iron_ingot".into(), 5));
            c.push(("minecraft:chest".into(), 1));
        }
        "minecraft:dropper" | "minecraft:dispenser" => {
            c.push(("minecraft:cobblestone".into(), 7));
            c.push(("minecraft:redstone".into(), 1));
            if block.block_id == "minecraft:dispenser" { c.push(("minecraft:bow".into(), 1)); }
        }
        "minecraft:chest" => { c.push(("minecraft:oak_planks".into(), 8)); }
        "minecraft:trapped_chest" => {
            c.push(("minecraft:oak_planks".into(), 8));
            c.push(("minecraft:tripwire_hook".into(), 1));
        }
        "minecraft:target" => { c.push(("minecraft:redstone".into(), 4)); c.push(("minecraft:hay_block".into(), 1)); }
        "minecraft:note_block" => { c.push(("minecraft:oak_planks".into(), 8)); c.push(("minecraft:redstone".into(), 1)); }
        "minecraft:slime_block" => { c.push(("minecraft:slime_ball".into(), 9)); }
        "minecraft:honey_block" => { c.push(("minecraft:honey_bottle".into(), 4)); }
        "minecraft:tnt" => { c.push(("minecraft:sand".into(), 4)); c.push(("minecraft:gunpowder".into(), 5)); }
        "minecraft:powered_rail" => { c.push(("minecraft:gold_ingot".into(), 6)); c.push(("minecraft:stick".into(), 1)); c.push(("minecraft:redstone".into(), 1)); }
        "minecraft:detector_rail" => { c.push(("minecraft:iron_ingot".into(), 6)); c.push(("minecraft:stone_pressure_plate".into(), 1)); c.push(("minecraft:redstone".into(), 1)); }
        "yog-vlsi:port" => { c.push(("minecraft:redstone".into(), 2)); c.push(("minecraft:stone".into(), 4)); }
        _ => { c.push(("minecraft:stone".into(), 1)); } // default: 1 stone for any other solid block
    }
    c
}

// ── Storage helpers ─────────────────────────────────────────────────────────

fn circuit_key(chip_id: &str) -> String { format!("circuit/{}", chip_id) }

fn save_circuit(srv: &dyn yog_api::Server, circuit: &CircuitData) {
    let game_dir = srv.game_dir();
    let mut store = Storage::open(&game_dir, "yog-vlsi");
    store.set(&circuit_key(&circuit.chip_id), circuit.to_json());
    let _ = store.flush();
}

pub fn load_circuit(srv: &dyn yog_api::Server, chip_id: &str) -> Option<CircuitData> {
    let game_dir = srv.game_dir();
    let store = Storage::open(&game_dir, "yog-vlsi");
    store.get(&circuit_key(chip_id))
        .and_then(|v| v.as_str())
        .map(String::from)
        .and_then(|j| CircuitData::from_json(&j))
}

pub fn load_circuit_into_vm(vm: &mut RedstoneVM, circuit: &CircuitData) {
    for block in &circuit.blocks {
        vm.set_block(block.x, block.y, block.z, parse_block_type(&block.block_id, &block.state_json));
    }
}

pub fn parse_block_type(block_id: &str, state_json: &str) -> BlockType {
    let facing = || {
        if state_json.contains("\"facing\":\"north\"") { Facing::North }
        else if state_json.contains("\"facing\":\"south\"") { Facing::South }
        else if state_json.contains("\"facing\":\"east\"") { Facing::East }
        else if state_json.contains("\"facing\":\"west\"") { Facing::West }
        else if state_json.contains("\"facing\":\"up\"") { Facing::Up }
        else { Facing::Down }
    };
    match block_id {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => BlockType::Air,
        "minecraft:redstone_wire" => BlockType::RedstoneWire,
        "minecraft:redstone_torch" => BlockType::RedstoneTorch { lit: !state_json.contains("\"lit\":\"false\"") },
        "minecraft:redstone_wall_torch" => BlockType::RedstoneWallTorch { lit: !state_json.contains("\"lit\":\"false\""), facing: facing() },
        "minecraft:repeater" => {
            let delay: u8 = if state_json.contains("\"delay\":\"2\"") { 2 }
            else if state_json.contains("\"delay\":\"3\"") { 3 }
            else if state_json.contains("\"delay\":\"4\"") { 4 } else { 1 };
            BlockType::Repeater { delay_ticks: delay, facing: facing(), locked: state_json.contains("\"locked\":\"true\"") }
        }
        "minecraft:comparator" => {
            let mode = if state_json.contains("\"mode\":\"subtract\"") { ComparatorMode::Subtract } else { ComparatorMode::Compare };
            BlockType::Comparator { mode, facing: facing() }
        }
        "minecraft:redstone_lamp" => BlockType::RedstoneLamp,
        "minecraft:redstone_block" => BlockType::RedstoneBlock,
        "minecraft:lever" => BlockType::Lever { on: state_json.contains("\"powered\":\"true\"") },
        "minecraft:stone_button" => BlockType::StoneButton { pressed: state_json.contains("\"powered\":\"true\""), facing: facing() },
        "minecraft:observer" => BlockType::Observer { facing: facing(), powered: state_json.contains("\"powered\":\"true\"") },
        "minecraft:note_block" => BlockType::NoteBlock,
        "minecraft:target" => BlockType::TargetBlock { power: 0 },
        "minecraft:piston" => BlockType::Piston { extended: state_json.contains("\"extended\":\"true\""), facing: facing() },
        "minecraft:sticky_piston" => BlockType::StickyPiston { extended: state_json.contains("\"extended\":\"true\""), facing: facing() },
        "minecraft:chest" => BlockType::Chest,
        "minecraft:trapped_chest" => BlockType::TrappedChest,
        "minecraft:barrel" => BlockType::Barrel,
        "minecraft:hopper" => BlockType::Hopper { facing: facing(), enabled: !state_json.contains("\"enabled\":\"false\"") },
        "minecraft:dropper" => BlockType::Dropper { facing: facing(), triggered: state_json.contains("\"triggered\":\"true\"") },
        "minecraft:dispenser" => BlockType::Dispenser { facing: facing(), triggered: state_json.contains("\"triggered\":\"true\"") },
        "minecraft:slime_block" => BlockType::SlimeBlock,
        "minecraft:honey_block" => BlockType::HoneyBlock,
        "minecraft:tnt" => BlockType::Tnt { unstable: state_json.contains("\"unstable\":\"true\"") },
        "minecraft:powered_rail" => BlockType::PoweredRail { powered: state_json.contains("\"powered\":\"true\"") },
        "minecraft:detector_rail" => BlockType::DetectorRail { powered: state_json.contains("\"powered\":\"true\"") },
        "yog-vlsi:port" => {
            let mode = if state_json.contains("\"mode\":\"input\"") { PortMode::Input }
            else if state_json.contains("\"mode\":\"output\"") { PortMode::Output }
            else { PortMode::Bidirectional };
            BlockType::Port(mode)
        }
        _ => BlockType::Solid,
    }
}
