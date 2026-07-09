//! ALU block: accepts microchips, routes signals, links chips into node graphs.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::{BlockDef, ItemDef, Registry};
use crate::commands::{ALU_STATE, CHIP_PORTS, EXT_VALUES, LINKS, VM_CACHE};

/// Fractional VM-steps owed to each chip, accumulated every server tick
/// (20/s) at the chip tier's `tick_rate()` and drained in whole-step
/// increments — this is what makes Netherite (40/s → 2 steps/tick) run
/// faster than Wood (5/s → 1 step every 4 ticks), per DESIGN.md §4.7.
static STEP_ACCUM: LazyLock<Mutex<HashMap<String, f32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub const EXT_SIDES: [&str; 6] = ["N", "S", "E", "W", "U", "D"];

/// Block/item id for a given ALU tier (each tier is its own real, placeable
/// block — there is no separate generic "yog-vlsi:alu" block).
pub fn alu_id(tier: crate::vm::Tier) -> String {
    format!("yog-vlsi:alu_{}", tier.id())
}

pub fn is_alu_block(block_id: &str) -> bool {
    crate::vm::Tier::ALL.iter().any(|t| alu_id(*t) == block_id)
}

pub fn register(registry: &mut Registry) {
    for tier in crate::vm::Tier::ALL {
        let id = alu_id(*tier);
        let (max_chips, channels) = match tier {
            crate::vm::Tier::Wood => (2, 8), crate::vm::Tier::Stone => (3, 16),
            crate::vm::Tier::Gold => (4, 24), crate::vm::Tier::Iron => (5, 32),
            crate::vm::Tier::Diamond => (6, 48), crate::vm::Tier::Netherite => (8, 64),
        };

        registry.register_block(
            BlockDef::new(&id).strength(5.0, 12.0).sound("metal").requires_tool().light_level(7)
                // Accepts both cable kinds — analog via a Redstone Port on
                // one of its faces, digital cable straight into the face.
                .connect_groups(&["analog", "digital"])
        );
        registry.register_item(
            ItemDef::new(&id)
                .tooltip(&format!("§7Insert programmed microchips. Right-click to open the node editor.\n§7Max chips: {} | Digital channels/side: {}", max_chips, channels))
        );

        let (corner, center) = match tier {
            crate::vm::Tier::Wood      => ("minecraft:oak_planks", "minecraft:redstone_block"),
            crate::vm::Tier::Stone     => ("minecraft:cobblestone", "minecraft:redstone_block"),
            crate::vm::Tier::Gold      => ("minecraft:gold_ingot", "minecraft:diamond"),
            crate::vm::Tier::Iron      => ("minecraft:iron_ingot", "minecraft:diamond"),
            crate::vm::Tier::Diamond   => ("minecraft:diamond", "minecraft:diamond_block"),
            crate::vm::Tier::Netherite => ("minecraft:netherite_ingot", "minecraft:diamond_block"),
        };
        registry.add_shaped_recipe(
            yog_api::ShapedRecipe::new(&format!("yog-vlsi:alu_{}_craft", tier.id()), &id, 1)
                .row("GCG").row("CRC").row("GDG")
                .key('G', corner).key('C', "minecraft:copper_ingot")
                .key('R', "minecraft:repeater").key('D', center)
        );
    }

    // Right-click any ALU tier → open the node editor.
    registry.on_use_block(|e, phase, srv| -> bool {
        if phase != yog_api::EventPhase::Pre { return true; }
        if !is_alu_block(&e.block_id) { return true; }
        crate::alu_ui::set_alu_pos((e.pos.x, e.pos.y, e.pos.z));
        crate::network::open_ui_for(srv, &e.player_name, "yog-vlsi:alu");
        false
    });
}

/// Server tick: step every installed chip's VM, then route signals along the
/// link graph the ALU node editor built.
///
/// External analog ports (Redstone Port + Analog Cable) are represented
/// internally as pseudo-chips (`commands::ext_chip_id`) so the same `LINKS`
/// map can carry chip↔chip and chip↔external links. Asserting the computed
/// `EXT_VALUES` onto real-world redstone still needs a loader-level
/// redstone-power read/write hook that doesn't exist yet (`get_block` only
/// returns block identity) — this is the one remaining piece of item #2 from
/// DESIGN.md that has to land in Yog-Mod-Loader itself before it can bridge
/// to the physical world. Digital Cable channel routing has no such
/// dependency: it's purely link-graph bit routing between ALUs, so it's
/// fully wired below.
pub fn tick_all(_srv: &dyn yog_api::Server) {
    let state = ALU_STATE.lock().unwrap().clone();

    // 1. Step every chip's VM at its tier's own rate.
    {
        let mut cache = VM_CACHE.lock().unwrap();
        let mut accum = STEP_ACCUM.lock().unwrap();
        for chips in state.values() {
            for (chip_id, _) in chips {
                let Some(vm) = cache.get_mut(chip_id) else { continue };
                let owed = accum.entry(chip_id.clone()).or_insert(0.0);
                *owed += vm.tier.tick_rate() as f32 / 20.0;
                while *owed >= 1.0 {
                    vm.step();
                    *owed -= 1.0;
                }
            }
        }
    }

    // 2. Read every chip's current port outputs.
    let ports = CHIP_PORTS.lock().unwrap().clone();
    let cache = VM_CACHE.lock().unwrap();
    let mut outputs: std::collections::HashMap<(String, String), u8> = std::collections::HashMap::new();
    for chips in state.values() {
        for (chip_id, _) in chips {
            let Some(vm) = cache.get(chip_id) else { continue };
            let Some(chip_ports) = ports.get(chip_id) else { continue };
            for port in chip_ports {
                if matches!(port.dir, crate::chip::PortDir::Input) { continue; }
                let x = port.index % vm.width;
                let z = port.index / vm.width;
                let value = vm.get_port_output(x, 0, z);
                outputs.insert((chip_id.clone(), port.label.clone()), value);
            }
        }
    }
    drop(cache);

    // 3. Push linked outputs into inputs (chip inputs take effect next tick;
    //    external pseudo-ports are recorded into EXT_VALUES for bridging).
    let links = LINKS.lock().unwrap();
    let mut ext = EXT_VALUES.lock().unwrap();
    let mut cache = VM_CACHE.lock().unwrap();
    for ((src_chip, src_label), (dst_chip, dst_label)) in links.iter() {
        let Some(&value) = outputs.get(&(src_chip.clone(), src_label.clone())) else { continue };

        if let Some(dst_ports) = ports.get(dst_chip) {
            if let Some(port) = dst_ports.iter().find(|p| &p.label == dst_label) {
                if let Some(vm) = cache.get_mut(dst_chip) {
                    let x = port.index % vm.width;
                    let z = port.index / vm.width;
                    vm.set_port_input(x, 0, z, value);
                }
                continue;
            }
        }

        // Destination isn't a chip port — must be an ALU external pseudo-port.
        if let Some(alu_pos) = parse_ext_chip_id(dst_chip) {
            ext.entry(alu_pos).or_default().insert(dst_label.clone(), value);
        }
    }
}

fn parse_ext_chip_id(id: &str) -> Option<(i32, i32, i32)> {
    let rest = id.strip_prefix("__ext_")?;
    let mut it = rest.splitn(3, '_');
    let x: i32 = it.next()?.parse().ok()?;
    let y: i32 = it.next()?.parse().ok()?;
    let z: i32 = it.next()?.parse().ok()?;
    Some((x, y, z))
}

/// Persist installed-chip placement, names, and the link/IO-mode graph so
/// ALUs survive a server restart. VM working state itself isn't persisted —
/// each chip's circuit is replayed from its saved design instead, which is
/// cheap next to storing a full grid snapshot per tier.
pub fn save_state(srv: &dyn yog_api::Server) {
    use crate::commands::CHIP_NAMES;
    let game_dir = srv.game_dir();
    let mut store = yog_api::Storage::open(&game_dir, "yog-vlsi");

    // HashMaps with tuple/non-string keys don't round-trip through
    // serde_json (object keys must be strings), so persist as Vec<(K, V)>.
    let state: Vec<((i32, i32, i32), Vec<(String, crate::vm::Tier)>)> =
        ALU_STATE.lock().unwrap().iter().map(|(k, v)| (*k, v.clone())).collect();
    let links: Vec<((String, String), (String, String))> =
        LINKS.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let modes: Vec<(String, String)> =
        crate::commands::IO_MODES.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let names: Vec<(String, String)> =
        CHIP_NAMES.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();

    store.set("alu_state", serde_json::to_string(&state).unwrap_or_default());
    store.set("alu_links", serde_json::to_string(&links).unwrap_or_default());
    store.set("alu_io_modes", serde_json::to_string(&modes).unwrap_or_default());
    store.set("alu_chip_names", serde_json::to_string(&names).unwrap_or_default());
    let _ = store.flush();
}

pub fn load_state(srv: &dyn yog_api::Server) {
    use crate::commands::install_chip;
    use crate::chip::ChipMeta;
    let game_dir = srv.game_dir();
    let store = yog_api::Storage::open(&game_dir, "yog-vlsi");

    let state: Vec<((i32, i32, i32), Vec<(String, crate::vm::Tier)>)> =
        store.get("alu_state").and_then(|v| v.as_str()).and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default();
    let names: std::collections::HashMap<String, String> =
        store.get("alu_chip_names").and_then(|v| v.as_str())
            .and_then(|j| serde_json::from_str::<Vec<(String, String)>>(j).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
    if let Some(links) = store.get("alu_links").and_then(|v| v.as_str())
        .and_then(|j| serde_json::from_str::<Vec<((String, String), (String, String))>>(j).ok())
    {
        *LINKS.lock().unwrap() = links.into_iter().collect();
    }
    if let Some(modes) = store.get("alu_io_modes").and_then(|v| v.as_str())
        .and_then(|j| serde_json::from_str::<Vec<(String, String)>>(j).ok())
    {
        *crate::commands::IO_MODES.lock().unwrap() = modes.into_iter().collect();
    }

    for (alu_pos, chips) in state {
        for (chip_id, tier) in chips {
            let name = names.get(&chip_id).cloned().unwrap_or_else(|| chip_id.clone());
            install_chip(srv, &ChipMeta { id: chip_id, tier, name, ports: Vec::new() }, alu_pos);
        }
    }
}
