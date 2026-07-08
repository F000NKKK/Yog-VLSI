//! ALU block: accepts microchips, routes signals, links chips into node graphs.

use yog_api::{BlockDef, ItemDef, Registry};
use crate::commands::{ALU_STATE, CHIP_PORTS, EXT_VALUES, LINKS, VM_CACHE};

pub const ALU_ID: &str = "yog-vlsi:alu";
pub const EXT_SIDES: [&str; 6] = ["N", "S", "E", "W", "U", "D"];

pub fn register(registry: &mut Registry) {
    registry.register_block(BlockDef::new(ALU_ID).strength(5.0,12.0).sound("metal").requires_tool().light_level(7));
    registry.register_item(ItemDef::new(ALU_ID).name("Arithmetic Logic Unit").tooltip("Insert programmed microchips. Right-click to open node editor."));

    registry.add_shaped_recipe(
        yog_api::ShapedRecipe::new("yog-vlsi:alu_craft", ALU_ID, 1)
            .row("GCG").row("CRC").row("GDG")
            .key('G',"minecraft:gold_ingot").key('C',"minecraft:copper_ingot")
            .key('R',"minecraft:repeater").key('D',"minecraft:diamond")
    );

    // ALU tier items (6 tiers, same block, different NBT)
    for tier in crate::vm::Tier::ALL {
        let tier_id = format!("yog-vlsi:alu_{}", tier.id());
        let (max_chips, channels) = match tier {
            crate::vm::Tier::Wood => (2, 8), crate::vm::Tier::Stone => (3, 16),
            crate::vm::Tier::Gold => (4, 24), crate::vm::Tier::Iron => (5, 32),
            crate::vm::Tier::Diamond => (6, 48), crate::vm::Tier::Netherite => (8, 64),
        };
        registry.register_item(ItemDef::new(&tier_id)
            .name(&format!("{} ALU", tier.name()))
            .tooltip(&format!("Max chips: {} | Digital channels/side: {}", max_chips, channels))
            .max_stack(1));
    }

    // Right-click → open ALU node editor
    registry.on_use_block(|e, phase, _srv| -> bool {
        if phase != yog_api::EventPhase::Pre { return true; }
        if e.block_id != ALU_ID { return true; }
        crate::alu_ui::set_alu_pos((e.pos.x, e.pos.y, e.pos.z));
        yog_api::open_ui("yog-vlsi:alu", true, false);
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
pub fn tick_all(srv: &dyn yog_api::Server) {
    let state = ALU_STATE.lock().unwrap().clone();

    // 1. Step every chip's VM once (rate limiting by tier is left at 1:1 for
    //    now — the per-tier tick_rate multiplier is a follow-up).
    {
        let mut cache = VM_CACHE.lock().unwrap();
        for chips in state.values() {
            for (chip_id, _) in chips {
                if let Some(vm) = cache.get_mut(chip_id) { vm.step(); }
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
