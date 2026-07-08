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
use crate::port;
use crate::vm::{BlockType, ComparatorMode, Facing, PortMode, RedstoneVM, ShulkerColor, Tier};
use crate::workbench::{BLUEPRINT_ID, RESOURCES};

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
pub static IO_MODES: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Latest computed value for each ALU's external analog ports (what a
/// Redstone Port *would* drive onto its Analog Cable). Actually asserting
/// this onto real-world redstone needs a loader-level redstone-power R/W
/// hook that does not exist yet — see `alu::tick_all` for details.
pub static EXT_VALUES: LazyLock<Mutex<HashMap<(i32, i32, i32), HashMap<String, u8>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Display name per installed chip (for the ALU node editor): chip_id → name.
pub static CHIP_NAMES: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

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
        let mut vm = RedstoneVM::new(meta.tier);
        load_circuit_into_vm(&mut vm, &circuit);
        VM_CACHE.lock().unwrap().insert(meta.id.clone(), vm);
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
    install_chip(srv, &meta, alu_pos);
    Player::new(srv, player_name).set_slot(slot, "minecraft:air", 0);
    format!("§aInstalled '{}' into the ALU.", meta.name)
}

// ── Tier helper ──────────────────────────────────────────────────────────────

pub fn parse_tier_pub(s: &str) -> Option<Tier> {
    parse_tier(s)
}

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
        Some(do_fabricate(srv, &ctx.source, &ctx.uuid, design_name, tier))
    });

    // ── /vlsi blueprint export <design_name> ───────────────────────────────
    registry.on_typed_command("vlsi", "word word word", |ctx, srv| {
        if ctx.arg_str(0).unwrap_or("") != "blueprint" || ctx.arg_str(1).unwrap_or("") != "export" { return None; }
        let design_name = ctx.arg_str(2).unwrap_or("");
        Some(do_export_blueprint(srv, &ctx.source, design_name))
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

// ── Fabrication / Blueprint export (shared by commands and GUI network actions) ─

pub fn do_fabricate(srv: &dyn yog_api::Server, player_name: &str, player_uuid: &str, design_name: &str, tier: Tier) -> String {
    let game_dir = srv.game_dir();
    let list = designs::list_designs(&game_dir, player_name);
    let design = match list.iter().find(|d| d.name == design_name) {
        Some(d) => d.clone(),
        None => return format!("§cDesign '{}' not found.", design_name),
    };
    let entry = match designs::load_design(&game_dir, player_name, &design.id) {
        Some(e) => e,
        None => return "§cFailed to load design data.".into(),
    };

    let cost = calculate_cost(&entry.circuit.blocks);

    // Try to consume resources from a workbench the player has previously fed.
    let pos = match Player::with_uuid(srv, player_name, player_uuid).position() {
        Some((px, py, pz)) => {
            let mut found = None;
            'search: for dx in -3..=3i32 {
                for dy in -3..=3i32 {
                    for dz in -3..=3i32 {
                        let key = ((px as i32) + dx, (py as i32) + dy, (pz as i32) + dz);
                        if RESOURCES.lock().unwrap().contains_key(&key) {
                            found = Some(key);
                            break 'search;
                        }
                    }
                }
            }
            found
        }
        None => None,
    };

    if let Some(wb_key) = pos {
        let mut resources = RESOURCES.lock().unwrap();
        let wb_res = resources.entry(wb_key).or_default();
        let missing: Vec<String> = cost.iter()
            .filter_map(|(item, qty)| {
                let have = wb_res.get(item).copied().unwrap_or(0);
                if have < *qty { Some(format!("{}: need {} have {}", item, qty, have)) } else { None }
            })
            .collect();
        if !missing.is_empty() {
            return format!("§cInsufficient resources in workbench:\n{}", missing.join("\n"));
        }
        for (item, qty) in &cost {
            let have = wb_res.get_mut(item).unwrap();
            *have = have.saturating_sub(*qty);
        }
    }

    let cost_str: Vec<String> = cost.iter().map(|(item, qty)| format!("{}: {}", item, qty)).collect();

    let meta = ChipMeta {
        id: crate::chip::new_chip_id(),
        tier,
        name: design_name.to_string(),
        ports: entry.circuit.ports.clone(),
    };
    save_circuit(srv, &entry.circuit);

    let item_id = format!("yog-vlsi:chip_{}", tier.id());
    Player::new(srv, player_name).give(&item_id, 1);
    let _ = srv.set_held_item_nbt(player_name, &meta.to_nbt());

    format!(
        "§aFabricated '{}' ({} tier, {} ports).\n§7Cost: {}",
        meta.name, tier.name(), meta.ports.len(),
        if cost_str.is_empty() { "free (empty design)".into() } else { cost_str.join(", ") }
    )
}

pub fn do_export_blueprint(srv: &dyn yog_api::Server, player_name: &str, design_name: &str) -> String {
    let game_dir = srv.game_dir();
    let list = designs::list_designs(&game_dir, player_name);
    let design = match list.iter().find(|d| d.name == design_name) {
        Some(d) => d.clone(),
        None => return format!("§cDesign '{}' not found.", design_name),
    };
    let entry = match designs::load_design(&game_dir, player_name, &design.id) {
        Some(e) => e,
        None => return "§cFailed to load design data.".into(),
    };

    let circuit_json = entry.circuit.to_json();
    let escaped = circuit_json.replace('\\', "\\\\").replace('"', "\\\"");
    let nbt = format!("{{YogVlsiBlueprint: \"{}\"}}", escaped);

    Player::new(srv, player_name).give(BLUEPRINT_ID, 1);
    let _ = srv.set_held_item_nbt(player_name, &nbt);
    format!("§aBlueprint exported: '{}' ({} blocks, {} ports).",
        design_name, entry.circuit.blocks.len(), entry.circuit.ports.len())
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
pub fn calculate_cost(blocks: &[CircuitBlock]) -> Vec<(String, u64)> {
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
        // Redstone components
        "minecraft:redstone_wire" => { c.push(("minecraft:redstone".into(), 1)); }
        "minecraft:redstone_torch" | "minecraft:redstone_wall_torch" => {
            c.push(("minecraft:redstone".into(), 1)); c.push(("minecraft:stick".into(), 1));
        }
        "minecraft:repeater" => {
            c.push(("minecraft:redstone".into(), 3)); c.push(("minecraft:stick".into(), 2)); c.push(("minecraft:stone".into(), 3));
        }
        "minecraft:comparator" => {
            c.push(("minecraft:redstone".into(), 3)); c.push(("minecraft:quartz".into(), 1)); c.push(("minecraft:stone".into(), 3));
        }
        "minecraft:redstone_lamp" => { c.push(("minecraft:redstone".into(), 4)); c.push(("minecraft:glowstone_dust".into(), 1)); }
        "minecraft:redstone_block" => { c.push(("minecraft:redstone".into(), 9)); }
        "minecraft:lever" => { c.push(("minecraft:stick".into(), 1)); c.push(("minecraft:cobblestone".into(), 1)); }
        "minecraft:stone_button" => { c.push(("minecraft:stone".into(), 1)); }
        "minecraft:observer" => { c.push(("minecraft:cobblestone".into(), 6)); c.push(("minecraft:redstone".into(), 2)); c.push(("minecraft:quartz".into(), 1)); }
        "minecraft:note_block" => { c.push(("minecraft:oak_planks".into(), 8)); c.push(("minecraft:redstone".into(), 1)); }
        "minecraft:target" => { c.push(("minecraft:redstone".into(), 4)); c.push(("minecraft:hay_block".into(), 1)); }
        // Pistons
        "minecraft:piston" => { c.push(("minecraft:oak_planks".into(), 3)); c.push(("minecraft:cobblestone".into(), 4)); c.push(("minecraft:iron_ingot".into(), 1)); c.push(("minecraft:redstone".into(), 1)); }
        "minecraft:sticky_piston" => { c.push(("minecraft:slime_ball".into(), 1)); c.push(("minecraft:piston".into(), 1)); }
        // Containers
        "minecraft:chest" | "minecraft:barrel" => { c.push(("minecraft:oak_planks".into(), 8)); }
        "minecraft:trapped_chest" => { c.push(("minecraft:oak_planks".into(), 8)); c.push(("minecraft:tripwire_hook".into(), 1)); }
        "minecraft:ender_chest" => { c.push(("minecraft:obsidian".into(), 8)); c.push(("minecraft:ender_eye".into(), 1)); }
        "minecraft:hopper" => { c.push(("minecraft:iron_ingot".into(), 5)); c.push(("minecraft:chest".into(), 1)); }
        "minecraft:dropper" => { c.push(("minecraft:cobblestone".into(), 7)); c.push(("minecraft:redstone".into(), 1)); }
        "minecraft:dispenser" => { c.push(("minecraft:cobblestone".into(), 7)); c.push(("minecraft:redstone".into(), 1)); c.push(("minecraft:bow".into(), 1)); }
        "minecraft:furnace" => { c.push(("minecraft:cobblestone".into(), 8)); }
        "minecraft:blast_furnace" => { c.push(("minecraft:smooth_stone".into(), 3)); c.push(("minecraft:furnace".into(), 1)); c.push(("minecraft:iron_ingot".into(), 5)); }
        "minecraft:smoker" => { c.push(("minecraft:oak_log".into(), 4)); c.push(("minecraft:furnace".into(), 1)); }
        "minecraft:brewing_stand" => { c.push(("minecraft:blaze_rod".into(), 1)); c.push(("minecraft:cobblestone".into(), 3)); }
        // Movement / utility
        "minecraft:slime_block" => { c.push(("minecraft:slime_ball".into(), 9)); }
        "minecraft:honey_block" => { c.push(("minecraft:honey_bottle".into(), 4)); }
        "minecraft:tnt" => { c.push(("minecraft:sand".into(), 4)); c.push(("minecraft:gunpowder".into(), 5)); }
        // Rails
        "minecraft:rail" => { c.push(("minecraft:iron_ingot".into(), 6)); c.push(("minecraft:stick".into(), 1)); }
        "minecraft:powered_rail" => { c.push(("minecraft:gold_ingot".into(), 6)); c.push(("minecraft:stick".into(), 1)); c.push(("minecraft:redstone".into(), 1)); }
        "minecraft:detector_rail" => { c.push(("minecraft:iron_ingot".into(), 6)); c.push(("minecraft:stone_pressure_plate".into(), 1)); c.push(("minecraft:redstone".into(), 1)); }
        "minecraft:activator_rail" => { c.push(("minecraft:iron_ingot".into(), 6)); c.push(("minecraft:stick".into(), 2)); c.push(("minecraft:redstone_torch".into(), 1)); }
        // Doors / trapdoors / gates
        "minecraft:iron_door" => { c.push(("minecraft:iron_ingot".into(), 6)); }
        "minecraft:iron_trapdoor" => { c.push(("minecraft:iron_ingot".into(), 4)); }
        // Glass
        "minecraft:glass" => { c.push(("minecraft:sand".into(), 1)); } // smelted
        "minecraft:glowstone" => { c.push(("minecraft:glowstone_dust".into(), 4)); }
        "minecraft:sea_lantern" => { c.push(("minecraft:prismarine_shard".into(), 4)); c.push(("minecraft:prismarine_crystals".into(), 5)); }
        // VLSI blocks
        "yog-vlsi:port" => { c.push(("minecraft:redstone".into(), 2)); c.push(("minecraft:stone".into(), 4)); }
        "yog-vlsi:vlsi_workbench" => { c.push(("minecraft:iron_ingot".into(), 4)); c.push(("minecraft:smooth_stone".into(), 3)); c.push(("minecraft:redstone_block".into(), 1)); }
        "yog-vlsi:alu" => { c.push(("minecraft:gold_ingot".into(), 4)); c.push(("minecraft:copper_ingot".into(), 2)); c.push(("minecraft:repeater".into(), 1)); c.push(("minecraft:diamond".into(), 1)); }
        "yog-vlsi:analog_cable" => { c.push(("minecraft:redstone".into(), 1)); }
        "yog-vlsi:digital_cable" => { c.push(("minecraft:gold_nugget".into(), 4)); c.push(("minecraft:redstone".into(), 4)); c.push(("minecraft:iron_ingot".into(), 1)); }
        "yog-vlsi:redstone_port" => { c.push(("minecraft:smooth_stone".into(), 8)); c.push(("minecraft:redstone".into(), 1)); }
        // Default: 1 stone
        _ => { c.push(("minecraft:stone".into(), 1)); }
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
        "minecraft:ender_chest" => BlockType::EnderChest,
        "minecraft:barrel" => BlockType::Barrel,
        "minecraft:hopper" => BlockType::Hopper { facing: facing(), enabled: !state_json.contains("\"enabled\":\"false\"") },
        "minecraft:dropper" => BlockType::Dropper { facing: facing(), triggered: state_json.contains("\"triggered\":\"true\"") },
        "minecraft:dispenser" => BlockType::Dispenser { facing: facing(), triggered: state_json.contains("\"triggered\":\"true\"") },
        "minecraft:furnace" => BlockType::Furnace { lit: state_json.contains("\"lit\":\"true\"") },
        "minecraft:blast_furnace" => BlockType::BlastFurnace { lit: state_json.contains("\"lit\":\"true\"") },
        "minecraft:smoker" => BlockType::Smoker { lit: state_json.contains("\"lit\":\"true\"") },
        "minecraft:brewing_stand" => BlockType::BrewingStand,
        "minecraft:white_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::White) },
        "minecraft:orange_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Orange) },
        "minecraft:magenta_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Magenta) },
        "minecraft:light_blue_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::LightBlue) },
        "minecraft:yellow_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Yellow) },
        "minecraft:lime_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Lime) },
        "minecraft:pink_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Pink) },
        "minecraft:gray_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Gray) },
        "minecraft:light_gray_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::LightGray) },
        "minecraft:cyan_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Cyan) },
        "minecraft:purple_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Purple) },
        "minecraft:blue_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Blue) },
        "minecraft:brown_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Brown) },
        "minecraft:green_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Green) },
        "minecraft:red_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Red) },
        "minecraft:black_shulker_box" => BlockType::ShulkerBox { color: Some(ShulkerColor::Black) },
        "minecraft:shulker_box" => BlockType::ShulkerBox { color: None },
        "minecraft:slime_block" => BlockType::SlimeBlock,
        "minecraft:honey_block" => BlockType::HoneyBlock,
        "minecraft:tnt" => BlockType::Tnt { unstable: state_json.contains("\"unstable\":\"true\"") },
        "minecraft:rail" => BlockType::Rail,
        "minecraft:powered_rail" => BlockType::PoweredRail { powered: state_json.contains("\"powered\":\"true\"") },
        "minecraft:detector_rail" => BlockType::DetectorRail { powered: state_json.contains("\"powered\":\"true\"") },
        "minecraft:activator_rail" => BlockType::ActivatorRail { powered: state_json.contains("\"powered\":\"true\"") },
        "minecraft:iron_door" => BlockType::IronDoor { open: state_json.contains("\"open\":\"true\""), facing: facing(), half: door_half(state_json) },
        "minecraft:oak_door" => BlockType::WoodDoor { open: state_json.contains("\"open\":\"true\""), facing: facing(), half: door_half(state_json) },
        "minecraft:iron_trapdoor" => BlockType::IronTrapdoor { open: state_json.contains("\"open\":\"true\""), facing: facing(), half: door_half(state_json) },
        "minecraft:oak_trapdoor" => BlockType::WoodTrapdoor { open: state_json.contains("\"open\":\"true\""), facing: facing(), half: door_half(state_json) },
        "minecraft:oak_fence_gate" => BlockType::FenceGate { open: state_json.contains("\"open\":\"true\""), facing: facing() },
        "minecraft:oak_button" => BlockType::WoodButton { pressed: state_json.contains("\"powered\":\"true\""), facing: facing() },
        "minecraft:stone_pressure_plate" => BlockType::StonePressurePlate { pressed: state_json.contains("\"powered\":\"true\"") },
        "minecraft:oak_pressure_plate" => BlockType::WoodPressurePlate { pressed: state_json.contains("\"powered\":\"true\"") },
        "minecraft:light_weighted_pressure_plate" => BlockType::LightWeightedPressurePlate { power: 0 },
        "minecraft:heavy_weighted_pressure_plate" => BlockType::HeavyWeightedPressurePlate { power: 0 },
        "minecraft:glass" => BlockType::Glass,
        port::PORT_INPUT => BlockType::Port(PortMode::Input),
        port::PORT_OUTPUT => BlockType::Port(PortMode::Output),
        port::PORT_BIDI => BlockType::Port(PortMode::Bidirectional),
        _ => BlockType::Solid,
    }
}

fn door_half(state_json: &str) -> crate::vm::DoorHalf {
    if state_json.contains("\"half\":\"upper\"") { crate::vm::DoorHalf::Upper } else { crate::vm::DoorHalf::Lower }
}
