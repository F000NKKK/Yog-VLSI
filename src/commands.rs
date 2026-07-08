//! Debug and utility commands for Yog VLSI.
//!
//! /vlsi                  — help
//! /vlsi chip <tier>      — give a blank microchip
//! /vlsi info             — show held chip metadata
//! /vlsi test <tier>      — create a programmed chip with test circuit
//! /vlsi vm step          — run one VM tick on the held chip

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::player::Player;
use yog_api::{Registry, Storage};

use crate::chip::{ChipMeta, CircuitBlock, CircuitData, Port, PortDir, PortSide};
use crate::vm::{BlockType, ComparatorMode, DoorHalf, Facing, PortMode, RedstoneVM, Tier};

/// In-memory ALU state: (x, y, z) → list of (chip_id, tier)
pub static ALU_STATE: LazyLock<Mutex<HashMap<(i32, i32, i32), Vec<(String, Tier)>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// In-memory chip-VM cache: chip_id → RedstoneVM
pub static VM_CACHE: LazyLock<Mutex<HashMap<String, RedstoneVM>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register(registry: &mut Registry) {
    // ── /vlsi ──────────────────────────────────────────────────────────────
    registry.on_command("vlsi", |_ctx, _srv| {
        Some(format!(
            "§6Yog VLSI §7— Very Large Scale Integration\n\
             §7/vlsi chip <tier>   §f— give blank chip\n\
             §7/vlsi info          §f— show held chip\n\
             §7/vlsi test <tier>   §f— create test chip\n\
             §7/vlsi vm step       §f— step VM on held chip\n\
             §7Tiers: wood, stone, gold, iron, diamond, netherite"
        ))
    });

    // ── /vlsi chip <tier> ──────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word", |ctx, _srv| {
        let sub = ctx.arg_str(0).unwrap_or("");
        if sub == "chip" {
            Some("§cUsage: /vlsi chip <tier>".into())
        } else {
            None
        }
    });

    registry.on_typed_command("vlsi", "word word", |ctx, srv| {
        let sub = ctx.arg_str(0).unwrap_or("");
        if sub != "chip" { return None; }

        let tier_name = ctx.arg_str(1).unwrap_or("wood").to_lowercase();
        let tier = match tier_name.as_str() {
            "wood" => Tier::Wood,
            "stone" => Tier::Stone,
            "gold" => Tier::Gold,
            "iron" => Tier::Iron,
            "diamond" => Tier::Diamond,
            "netherite" => Tier::Netherite,
            _ => return Some(format!("§cUnknown tier: {}. Use wood/stone/gold/iron/diamond/netherite.", tier_name)),
        };

        let item_id = format!("yog-vlsi:chip_{}", tier.id());
        let ok = Player::new(srv, &ctx.source).give(&item_id, 1);
        Some(if ok {
            format!("§aGiven 1× {} Microchip ({} ticks/s, {}×{} world)",
                tier.name(), tier.tick_rate(), tier.world_size(), tier.world_size())
        } else {
            "§cFailed to give item.".into()
        })
    });

    // ── /vlsi info ─────────────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word", |ctx, srv| {
        let sub = ctx.arg_str(0).unwrap_or("");
        if sub != "info" { return None; }

        let nbt = match srv.get_held_item_nbt(&ctx.source) {
            Some(n) => n,
            None => return Some("§cNo item in hand or no NBT data.".into()),
        };

        match ChipMeta::from_nbt(&nbt) {
            Some(meta) => {
                let port_info = if meta.ports.is_empty() {
                    "§7(no ports)".to_string()
                } else {
                    meta.ports.iter()
                        .map(|p| format!("  §e{}§7: {} side={} idx={}",
                            p.label, p.dir.name(), p.side.name(), p.index))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                Some(format!(
                    "§6Chip: §f{}\n§7ID: §f{}\n§7Tier: §e{} (§b{} ticks/s§7, §a{}×{} world§7)\n§7Ports:\n{}",
                    meta.name, meta.id, meta.tier.name(),
                    meta.tier.tick_rate(), meta.tier.world_size(), meta.tier.world_size(),
                    port_info
                ))
            }
            None => Some("§cNo VLSI chip data found on held item.".into()),
        }
    });

    // ── /vlsi test <tier> ──────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word word", |ctx, srv| {
        let sub = ctx.arg_str(0).unwrap_or("");
        if sub != "test" { return None; }

        let tier_name = ctx.arg_str(1).unwrap_or("wood").to_lowercase();
        let tier = match tier_name.as_str() {
            "wood" => Tier::Wood,
            "stone" => Tier::Stone,
            "gold" => Tier::Gold,
            "iron" => Tier::Iron,
            "diamond" => Tier::Diamond,
            "netherite" => Tier::Netherite,
            _ => return Some(format!("§cUnknown tier: {}", tier_name)),
        };

        let size = tier.world_size();
        let chip_id = crate::chip::new_chip_id();

        // Build a test circuit: redstone block → wire → lamp, plus I/O ports
        let mut blocks = Vec::new();

        // Input port at (0, 0, 0) on West side
        blocks.push(CircuitBlock {
            x: 0, y: 0, z: 0,
            block_id: "yog-vlsi:port".into(),
            state_json: r#"{"mode":"input"}"#.into(),
        });

        // Output port at (size-1, 0, 0) on East side
        blocks.push(CircuitBlock {
            x: size - 1, y: 0, z: 0,
            block_id: "yog-vlsi:port".into(),
            state_json: r#"{"mode":"output"}"#.into(),
        });

        // Redstone block at (1, 0, 0) as power source
        blocks.push(CircuitBlock {
            x: 1, y: 0, z: 0,
            block_id: "minecraft:redstone_block".into(),
            state_json: "{}".into(),
        });

        // Redstone wire from (2, 0, 0) to (size-3, 0, 0)
        for x in 2..size.saturating_sub(1) {
            if x == size - 1 { continue; }
            blocks.push(CircuitBlock {
                x, y: 0, z: 0,
                block_id: "minecraft:redstone_wire".into(),
                state_json: "{}".into(),
            });
        }

        // Redstone lamp at (size-2, 0, 0)
        if size > 2 {
            blocks.push(CircuitBlock {
                x: size - 2, y: 0, z: 0,
                block_id: "minecraft:redstone_lamp".into(),
                state_json: r#"{"lit":"false"}"#.into(),
            });
        }

        let ports = vec![
            Port { label: "IN".into(), side: PortSide::West, index: 0, dir: PortDir::Input },
            Port { label: "OUT".into(), side: PortSide::East, index: 0, dir: PortDir::Output },
        ];

        // Save circuit data to server storage
        let circuit = CircuitData {
            chip_id: chip_id.clone(),
            width: size,
            height: size,
            blocks,
            ports: ports.clone(),
        };
        save_circuit(srv, &circuit);

        // Create chip meta and set NBT
        let meta = ChipMeta {
            id: chip_id,
            tier,
            name: format!("Test {}", tier.name()),
            ports,
        };

        let item_id = format!("yog-vlsi:chip_{}", tier.id());
        let nbt = meta.to_nbt();
        let ok = Player::new(srv, &ctx.source).set_slot(36, &item_id, 1); // slot 36 = first hotbar
        if ok {
            let _ = srv.set_held_item_nbt(&ctx.source, &nbt);
        }

        Some(format!(
            "§aCreated test chip '{}' ({}). §7Right-click to inspect with /vlsi info",
            meta.name, tier.name()
        ))
    });

    // ── /vlsi vm step ──────────────────────────────────────────────────────
    registry.on_typed_command("vlsi", "word word", |ctx, srv| {
        let sub = ctx.arg_str(0).unwrap_or("");
        let action = ctx.arg_str(1).unwrap_or("");
        if sub != "vm" || action != "step" { return None; }

        let nbt = match srv.get_held_item_nbt(&ctx.source) {
            Some(n) => n,
            None => return Some("§cNo item in hand or no NBT data.".into()),
        };

        let meta = match ChipMeta::from_nbt(&nbt) {
            Some(m) => m,
            None => return Some("§cNo VLSI chip data on held item.".into()),
        };

        // Load or create VM
        let mut cache = VM_CACHE.lock().unwrap();
        let vm = cache.entry(meta.id.clone()).or_insert_with(|| {
            let mut vm = RedstoneVM::new(meta.tier);
            // Load circuit data if available
            if let Some(circuit) = load_circuit(srv, &meta.id) {
                load_circuit_into_vm(&mut vm, &circuit);
            }
            vm
        });

        let before_tick = vm.tick;
        vm.step();
        let outputs = vm.read_outputs(0);

        let output_info = if outputs.is_empty() {
            "§7(no output ports)".to_string()
        } else {
            outputs.iter()
                .map(|(x, z, p)| format!("  ({}, {}): §c{}", x, z, p))
                .collect::<Vec<_>>()
                .join("\n")
        };

        Some(format!(
            "§6VM Step §f{}→{}\n§7Output ports:\n{}",
            before_tick, vm.tick, output_info
        ))
    });
}

// ── Storage helpers ───────────────────────────────────────────────────────────

fn circuit_key(chip_id: &str) -> String {
    format!("circuit/{}", chip_id)
}

fn save_circuit(srv: &dyn yog_api::Server, circuit: &CircuitData) {
    let game_dir = srv.game_dir();
    let mut store = Storage::open(&game_dir, "yog-vlsi");
    store.set(&circuit_key(&circuit.chip_id), circuit.to_json());
    let _ = store.flush();
}

pub fn load_circuit(srv: &dyn yog_api::Server, chip_id: &str) -> Option<CircuitData> {
    let game_dir = srv.game_dir();
    let store = Storage::open(&game_dir, "yog-vlsi");
    let json = store.get(&circuit_key(chip_id))
        .and_then(|v| v.as_str())
        .map(String::from);
    json.and_then(|j| CircuitData::from_json(&j))
}

/// Load a CircuitData into a RedstoneVM, placing all blocks.
pub fn load_circuit_into_vm(vm: &mut RedstoneVM, circuit: &CircuitData) {
    for block in &circuit.blocks {
        let bt = parse_block_type(&block.block_id, &block.state_json);
        vm.set_block(block.x, block.y, block.z, bt);
    }
}

/// Parse a Minecraft block ID + state JSON into our VM BlockType.
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
        "minecraft:redstone_torch" => {
            let lit = !state_json.contains("\"lit\":\"false\"");
            BlockType::RedstoneTorch { lit }
        }
        "minecraft:redstone_wall_torch" => {
            let lit = !state_json.contains("\"lit\":\"false\"");
            BlockType::RedstoneWallTorch { lit, facing: facing() }
        }
        "minecraft:repeater" => {
            let delay: u8 = if state_json.contains("\"delay\":\"2\"") { 2 }
            else if state_json.contains("\"delay\":\"3\"") { 3 }
            else if state_json.contains("\"delay\":\"4\"") { 4 }
            else { 1 };
            let locked = state_json.contains("\"locked\":\"true\"");
            BlockType::Repeater { delay_ticks: delay, facing: facing(), locked }
        }
        "minecraft:comparator" => {
            let mode = if state_json.contains("\"mode\":\"subtract\"") { ComparatorMode::Subtract }
            else { ComparatorMode::Compare };
            BlockType::Comparator { mode, facing: facing() }
        }
        "minecraft:redstone_lamp" => BlockType::RedstoneLamp,
        "minecraft:redstone_block" => BlockType::RedstoneBlock,
        "minecraft:lever" => {
            let on = state_json.contains("\"powered\":\"true\"");
            BlockType::Lever { on }
        }
        "minecraft:stone_button" => {
            let pressed = state_json.contains("\"powered\":\"true\"");
            BlockType::StoneButton { pressed, facing: facing() }
        }
        "minecraft:oak_button" | "minecraft:spruce_button" | "minecraft:birch_button"
        | "minecraft:jungle_button" | "minecraft:acacia_button" | "minecraft:dark_oak_button"
        | "minecraft:mangrove_button" | "minecraft:cherry_button" | "minecraft:bamboo_button"
        | "minecraft:crimson_button" | "minecraft:warped_button" => {
            let pressed = state_json.contains("\"powered\":\"true\"");
            BlockType::WoodButton { pressed, facing: facing() }
        }
        "minecraft:stone_pressure_plate" => {
            let pressed = state_json.contains("\"powered\":\"true\"");
            BlockType::StonePressurePlate { pressed }
        }
        "minecraft:oak_pressure_plate" | "minecraft:spruce_pressure_plate"
        | "minecraft:birch_pressure_plate" | "minecraft:jungle_pressure_plate"
        | "minecraft:acacia_pressure_plate" | "minecraft:dark_oak_pressure_plate"
        | "minecraft:mangrove_pressure_plate" | "minecraft:cherry_pressure_plate"
        | "minecraft:bamboo_pressure_plate" | "minecraft:crimson_pressure_plate"
        | "minecraft:warped_pressure_plate" => {
            let pressed = state_json.contains("\"powered\":\"true\"");
            BlockType::WoodPressurePlate { pressed }
        }
        "minecraft:light_weighted_pressure_plate" => {
            let power: u8 = state_json.split("\"power\":\"")
                .nth(1).and_then(|s| s.split('\"').next())
                .and_then(|s| s.parse().ok()).unwrap_or(0);
            BlockType::LightWeightedPressurePlate { power }
        }
        "minecraft:heavy_weighted_pressure_plate" => {
            let power: u8 = state_json.split("\"power\":\"")
                .nth(1).and_then(|s| s.split('\"').next())
                .and_then(|s| s.parse().ok()).unwrap_or(0);
            BlockType::HeavyWeightedPressurePlate { power }
        }
        "minecraft:observer" => {
            let powered = state_json.contains("\"powered\":\"true\"");
            BlockType::Observer { facing: facing(), powered }
        }
        "minecraft:note_block" => BlockType::NoteBlock,
        "minecraft:target" => {
            let power: u8 = state_json.split("\"power\":\"")
                .nth(1).and_then(|s| s.split('\"').next())
                .and_then(|s| s.parse().ok()).unwrap_or(0);
            BlockType::TargetBlock { power }
        }
        "minecraft:piston" => {
            let extended = state_json.contains("\"extended\":\"true\"");
            BlockType::Piston { extended, facing: facing() }
        }
        "minecraft:sticky_piston" => {
            let extended = state_json.contains("\"extended\":\"true\"");
            BlockType::StickyPiston { extended, facing: facing() }
        }
        "minecraft:chest" => BlockType::Chest,
        "minecraft:trapped_chest" => BlockType::TrappedChest,
        "minecraft:ender_chest" => BlockType::EnderChest,
        "minecraft:barrel" => BlockType::Barrel,
        "minecraft:hopper" => {
            let enabled = !state_json.contains("\"enabled\":\"false\"");
            BlockType::Hopper { facing: facing(), enabled }
        }
        "minecraft:dropper" => {
            let triggered = state_json.contains("\"triggered\":\"true\"");
            BlockType::Dropper { facing: facing(), triggered }
        }
        "minecraft:dispenser" => {
            let triggered = state_json.contains("\"triggered\":\"true\"");
            BlockType::Dispenser { facing: facing(), triggered }
        }
        "minecraft:furnace" => {
            let lit = state_json.contains("\"lit\":\"true\"");
            BlockType::Furnace { lit }
        }
        "minecraft:blast_furnace" => {
            let lit = state_json.contains("\"lit\":\"true\"");
            BlockType::BlastFurnace { lit }
        }
        "minecraft:smoker" => {
            let lit = state_json.contains("\"lit\":\"true\"");
            BlockType::Smoker { lit }
        }
        "minecraft:brewing_stand" => BlockType::BrewingStand,
        "minecraft:slime_block" => BlockType::SlimeBlock,
        "minecraft:honey_block" => BlockType::HoneyBlock,
        "minecraft:tnt" => {
            let unstable = state_json.contains("\"unstable\":\"true\"");
            BlockType::Tnt { unstable }
        }
        "minecraft:iron_door" => {
            let open = state_json.contains("\"open\":\"true\"");
            let half = if state_json.contains("\"half\":\"upper\"") { DoorHalf::Upper } else { DoorHalf::Lower };
            BlockType::IronDoor { open, facing: facing(), half }
        }
        s if s.ends_with("_door") && s != "minecraft:iron_door" => {
            let open = state_json.contains("\"open\":\"true\"");
            let half = if state_json.contains("\"half\":\"upper\"") { DoorHalf::Upper } else { DoorHalf::Lower };
            BlockType::WoodDoor { open, facing: facing(), half }
        }
        "minecraft:iron_trapdoor" => {
            let open = state_json.contains("\"open\":\"true\"");
            let half = if state_json.contains("\"half\":\"top\"") { DoorHalf::Upper } else { DoorHalf::Lower };
            BlockType::IronTrapdoor { open, facing: facing(), half }
        }
        s if s.ends_with("_trapdoor") && s != "minecraft:iron_trapdoor" => {
            let open = state_json.contains("\"open\":\"true\"");
            let half = if state_json.contains("\"half\":\"top\"") { DoorHalf::Upper } else { DoorHalf::Lower };
            BlockType::WoodTrapdoor { open, facing: facing(), half }
        }
        s if s.ends_with("_fence_gate") => {
            let open = state_json.contains("\"open\":\"true\"");
            BlockType::FenceGate { open, facing: facing() }
        }
        "minecraft:rail" => BlockType::Rail,
        "minecraft:powered_rail" => {
            let powered = state_json.contains("\"powered\":\"true\"");
            BlockType::PoweredRail { powered }
        }
        "minecraft:detector_rail" => {
            let powered = state_json.contains("\"powered\":\"true\"");
            BlockType::DetectorRail { powered }
        }
        "minecraft:activator_rail" => {
            let powered = state_json.contains("\"powered\":\"true\"");
            BlockType::ActivatorRail { powered }
        }
        s if s.contains("shulker_box") => BlockType::ShulkerBox { color: None },
        "minecraft:glass" | "minecraft:white_stained_glass" | "minecraft:orange_stained_glass"
        | "minecraft:magenta_stained_glass" | "minecraft:light_blue_stained_glass"
        | "minecraft:yellow_stained_glass" | "minecraft:lime_stained_glass"
        | "minecraft:pink_stained_glass" | "minecraft:gray_stained_glass"
        | "minecraft:light_gray_stained_glass" | "minecraft:cyan_stained_glass"
        | "minecraft:purple_stained_glass" | "minecraft:blue_stained_glass"
        | "minecraft:brown_stained_glass" | "minecraft:green_stained_glass"
        | "minecraft:red_stained_glass" | "minecraft:black_stained_glass"
        | "minecraft:tinted_glass" | "minecraft:glass_pane" | "minecraft:glowstone"
        | "minecraft:sea_lantern" => BlockType::Glass,
        "yog-vlsi:port" => {
            let mode = if state_json.contains("\"mode\":\"input\"") { PortMode::Input }
            else if state_json.contains("\"mode\":\"output\"") { PortMode::Output }
            else { PortMode::Bidirectional };
            BlockType::Port(mode)
        }
        _ => BlockType::Solid,
    }
}
