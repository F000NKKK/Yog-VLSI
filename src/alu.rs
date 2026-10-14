//! ALU block: accepts microchips, routes signals, links chips into node
//! graphs. Right-click opens its node editor (`alu.yui`); this file is that
//! screen's code-behind (`AluVm`), plus the block's recipes, tick and
//! persistence.

yog_api::ui_include!("alu");

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::Registry;
use yog_kit::prelude::*;

use crate::commands::{ALU_STATE, CHIP_NAMES, CHIP_PORTS, EXT_VALUES, IO_MODES, LINKS, VM_CACHE, ext_chip_id};
use crate::network;
use crate::res::{Colors, Strings};
use crate::tier::{SERVER_TICKS_PER_SECOND, Tier};
use crate::vm::PortMode;

/// Server → client reply channel for the chip selector's inventory listing.
pub const CHIP_LIST_CHANNEL: &str = "yog-vlsi:alu_chiplist";
/// Links the editor lists (the rest still route).
const LINKS_SHOWN: usize = 10;

/// Fractional VM-steps owed to each chip, accumulated every server tick
/// (20/s) at the chip tier's `tick_rate()` and drained in whole-step
/// increments — this is what makes Netherite (40/s → 2 steps/tick) run
/// faster than Wood (5/s → 1 step every 4 ticks), per DESIGN.md §4.7.
static STEP_ACCUM: LazyLock<Mutex<HashMap<String, f32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub const EXT_SIDES: [&str; 6] = ["N", "S", "E", "W", "U", "D"];

pub fn is_alu_block(block_id: &str) -> bool {
    Tier::from_alu_block(block_id).is_some()
}

// ── The node editor's view model ─────────────────────────────────────────────

/// A programmed chip in the player's inventory, offered by "+ Add Chip".
#[derive(Debug, Clone)]
pub struct ChipChoice {
    pub slot: u32,
    tier: String,
    name: String,
}

impl ChipChoice {
    pub fn label(&self) -> String {
        Strings::screen_alu_chip_row(&self.tier, &self.name)
    }
}

/// One of the ALU's own external ports.
pub struct ExtPort {
    pub side: &'static str,
    pub mode: PortMode,
    selected: bool,
}

impl ExtPort {
    pub fn label(&self) -> String {
        format!("{} {}", if self.selected { "▶" } else { "◉" }, self.side)
    }
    pub fn color(&self) -> u32 {
        if self.selected { Colors::PIN_SELECTED } else { self.mode.color() }
    }
}

/// A row of the chip list: a chip's header (`port` empty) or one of its pins.
pub struct PinRow {
    pub chip: String,
    pub port: String,
    pub text: String,
    pub color: u32,
    pub bg: u32,
}

/// One chip-to-chip (or chip-to-ALU) link.
pub struct LinkRow {
    pub text: String,
    /// Where the link starts — its key in `LINKS`.
    pub chip: String,
    pub port: String,
}

#[derive(Default)]
pub struct AluVm {
    /// The ALU being edited, and its tier.
    pos: (i32, i32, i32),
    tier: Option<Tier>,
    /// The chip selector is showing (the server listed the chips).
    choosing: bool,
    choices: Vec<ChipChoice>,
    /// Where a new link starts: (chip id, port label).
    from: Option<(String, String)>,
}

impl ViewModel for AluVm {
    fn on_close(&mut self, _cx: &mut Cx<'_, Self>) {
        self.choosing = false;
        self.from = None;
    }
}

impl AluVm {
    /// The editor opens on the ALU at `pos`.
    pub fn open_at(&mut self, pos: (i32, i32, i32), tier: Tier) {
        self.pos = pos;
        self.tier = Some(tier);
        self.choosing = false;
        self.from = None;
    }
    /// The server listed the player's chips: pick one.
    pub fn show_choices(&mut self, choices: Vec<ChipChoice>) {
        self.choices = choices;
        self.choosing = true;
    }

    fn chips(&self) -> Vec<(String, Tier)> {
        ALU_STATE.lock().unwrap().get(&self.pos).cloned().unwrap_or_default()
    }
    fn chip_name(id: &str) -> String {
        if id.starts_with("__ext_") {
            return Strings::screen_alu_external();
        }
        CHIP_NAMES.lock().unwrap().get(id).cloned().unwrap_or_else(|| id[..6.min(id.len())].to_string())
    }
    /// Whether a link starting at `chip` belongs to this ALU.
    fn owns(&self, chip: &str) -> bool {
        chip == ext_chip_id(self.pos) || self.chips().iter().any(|(c, _)| c == chip)
    }
    fn is_from(&self, chip: &str, port: &str) -> bool {
        self.from.as_ref().is_some_and(|(c, p)| c == chip && p == port)
    }

    pub fn title(&self) -> String {
        let tier = self.tier.unwrap_or(Tier::ALL[0]);
        Strings::screen_alu_title(
            tier.name(),
            self.chips().len(),
            tier.max_chips(),
            LINKS.lock().unwrap().keys().filter(|(c, _)| self.owns(c)).count(),
            tier.digital_channels(),
        )
    }
    pub fn editing(&self) -> bool {
        !self.choosing
    }
    pub fn ext_ports(&self) -> Vec<ExtPort> {
        let ext = ext_chip_id(self.pos);
        let modes = IO_MODES.lock().unwrap();
        EXT_SIDES
            .iter()
            .map(|&side| ExtPort {
                side,
                mode: modes.get(&format!("{ext}:{side}")).copied().unwrap_or(PortMode::Input),
                selected: self.is_from(&ext, side),
            })
            .collect()
    }
    pub fn pins(&self) -> Vec<PinRow> {
        let ports = CHIP_PORTS.lock().unwrap();
        let mut rows = Vec::new();
        for (chip, tier) in self.chips() {
            rows.push(PinRow {
                text: Strings::screen_alu_chip_row(tier.name(), Self::chip_name(&chip)),
                chip: chip.clone(),
                port: String::new(),
                color: 0xFF_FFFFFF,
                bg: 0xFF_555555,
            });
            for p in ports.get(&chip).into_iter().flatten() {
                let selected = self.is_from(&chip, &p.label);
                rows.push(PinRow {
                    text: format!("{} {} {}", if selected { "▶" } else { " " }, p.dir.short(), p.label),
                    color: if selected { Colors::PIN_SELECTED } else { p.dir.color() },
                    bg: 0,
                    chip: chip.clone(),
                    port: p.label.clone(),
                });
            }
        }
        rows
    }
    pub fn has_links(&self) -> bool {
        LINKS.lock().unwrap().keys().any(|(c, _)| self.owns(c))
    }
    pub fn links(&self) -> Vec<LinkRow> {
        LINKS
            .lock()
            .unwrap()
            .iter()
            .filter(|((sc, _), _)| self.owns(sc))
            .take(LINKS_SHOWN)
            .map(|((sc, sp), (dc, dp))| LinkRow {
                text: Strings::screen_alu_link_row(
                    format!("{}.{sp}", Self::chip_name(sc)),
                    format!("{}.{dp}", Self::chip_name(dc)),
                ),
                chip: sc.clone(),
                port: sp.clone(),
            })
            .collect()
    }
    /// Removes the link starting at `chip`.`port`.
    pub fn unlink(&mut self, chip: String, port: String) {
        LINKS.lock().unwrap().remove(&(chip, port));
    }
    /// Removes every link of this ALU.
    pub fn clear_links(&mut self) {
        self.from = None;
        let owned: Vec<_> = LINKS.lock().unwrap().keys().filter(|(c, _)| self.owns(c)).cloned().collect();
        let mut links = LINKS.lock().unwrap();
        for key in owned {
            links.remove(&key);
        }
    }

    pub fn select_ext(&mut self, side: &'static str) {
        self.select(ext_chip_id(self.pos), side.to_string());
    }
    pub fn cycle_mode(&mut self, side: &'static str) {
        let key = format!("{}:{side}", ext_chip_id(self.pos));
        let mut modes = IO_MODES.lock().unwrap();
        let next = modes.get(&key).copied().unwrap_or(PortMode::Input).next();
        modes.insert(key, next);
    }
    pub fn select_pin(&mut self, chip: String, port: String) {
        if !port.is_empty() {
            self.select(chip, port);
        }
    }
    /// The first pick starts a link; the second ends it.
    fn select(&mut self, chip: String, port: String) {
        match self.from.take() {
            None => self.from = Some((chip, port)),
            Some(src) => {
                LINKS.lock().unwrap().insert(src, (chip, port));
            }
        }
    }
    pub fn add_chip(&mut self) {
        network::request_chip_list();
    }
    /// Links every pair of chips' same-named ports.
    pub fn auto_link(&mut self) {
        let chips = self.chips();
        let ports = CHIP_PORTS.lock().unwrap();
        let mut links = LINKS.lock().unwrap();
        for (i, (a, _)) in chips.iter().enumerate() {
            for (b, _) in &chips[i + 1..] {
                for pa in ports.get(a).into_iter().flatten() {
                    for pb in ports.get(b).into_iter().flatten() {
                        if pa.label == pb.label {
                            links.insert((a.clone(), pa.label.clone()), (b.clone(), pb.label.clone()));
                        }
                    }
                }
            }
        }
    }
    pub fn save(&mut self) {
        network::send_alu_action(&["save_links"]);
    }
    pub fn install(&mut self, slot: u32) {
        self.choosing = false;
        let (x, y, z) = self.pos;
        network::send_alu_action(&["install", &slot.to_string(), &x.to_string(), &y.to_string(), &z.to_string()]);
    }
    pub fn cancel(&mut self) {
        self.choosing = false;
    }
}

/// The ALU blocks' node editor (recipes: content/recipes/alu_*.yrecipe).
pub fn setup(registry: &mut Registry) {
    let screen = register(registry);
    // The server's answer to "+ Add Chip": `slot\ttier\tname` lines.
    let chooser = screen.clone();
    registry.on_client_packet(CHIP_LIST_CHANNEL, move |ev, _srv| {
        let text = String::from_utf8_lossy(&ev.payload);
        let choices = text
            .lines()
            .filter_map(|line| {
                let mut f = line.split('\t');
                Some(ChipChoice {
                    slot: f.next()?.parse().ok()?,
                    tier: f.next()?.to_string(),
                    name: f.next().unwrap_or("chip").to_string(),
                })
            })
            .collect();
        chooser.update(move |vm| vm.show_choices(choices));
    });

    // Right-click any ALU tier → open the node editor on it.
    registry.on_use_block(move |e, phase, srv| -> bool {
        if phase != yog_api::EventPhase::Pre || !is_alu_block(&e.block_id) {
            return true;
        }
        let pos = (e.pos.x, e.pos.y, e.pos.z);
        let Some(tier) = Tier::from_alu_block(&e.block_id) else { return true };
        crate::commands::ALU_TIERS.lock().unwrap().insert(pos, tier);
        crate::commands::ALU_DIMS.lock().unwrap().insert(pos, e.dimension.clone());
        screen.update(move |vm| vm.open_at(pos, tier));
        network::open_ui_for(srv, &e.player_name, ID);
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

    // 1. Step every chip's VM at its tier's own rate.
    {
        let mut cache = VM_CACHE.lock().unwrap();
        let mut accum = STEP_ACCUM.lock().unwrap();
        for chips in state.values() {
            for (chip_id, tier) in chips {
                let Some(vm) = cache.get_mut(chip_id) else { continue };
                let owed = accum.entry(chip_id.clone()).or_insert(0.0);
                *owed += tier.tick_rate() as f32 / SERVER_TICKS_PER_SECOND as f32;
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
                if !port.dir.outputs() { continue; }
                let value = vm.get_port_output(port.x, port.y, port.z);
                outputs.insert((chip_id.clone(), port.label.clone()), value);
            }
        }
    }
    drop(cache);

    // 2b. What arrives at the ALUs' input sides from their cables acts as an
    //     output of the ALU's external pseudo-chip.
    let dims = crate::commands::ALU_DIMS.lock().unwrap().clone();
    let modes = IO_MODES.lock().unwrap().clone();
    let side_mode = |pos: (i32, i32, i32), side: &str| {
        modes.get(&format!("{}:{side}", ext_chip_id(pos))).copied().unwrap_or(PortMode::Input)
    };
    for (pos, dim) in &dims {
        for side in EXT_SIDES {
            if side_mode(*pos, side).inputs()
                && let Some(level) = crate::cables::side_input(srv, dim, *pos, side)
            {
                outputs.insert((ext_chip_id(*pos), side.to_string()), level);
            }
        }
    }

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
                    vm.set_port_input(port.x, port.y, port.z, value);
                }
                continue;
            }
        }

        // Destination isn't a chip port — must be an ALU external pseudo-port.
        if let Some(alu_pos) = parse_ext_chip_id(dst_chip) {
            ext.entry(alu_pos).or_default().insert(dst_label.clone(), value);
        }
    }
    drop(cache);

    // 4. The ALUs' output sides drive their cables.
    for (pos, dim) in &dims {
        for side in EXT_SIDES {
            if side_mode(*pos, side).outputs() {
                let level = ext.get(pos).and_then(|m| m.get(side)).copied().unwrap_or(0);
                crate::cables::side_output(srv, dim, *pos, side, level);
            }
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
    let game_dir = srv.game_dir();
    let mut store = yog_api::Storage::open(&game_dir, crate::MOD_ID);

    // HashMaps with tuple/non-string keys don't round-trip through
    // serde_json (object keys must be strings), so persist as Vec<(K, V)>.
    let state: Vec<((i32, i32, i32), Vec<(String, Tier)>)> =
        ALU_STATE.lock().unwrap().iter().map(|(k, v)| (*k, v.clone())).collect();
    let links: Vec<((String, String), (String, String))> =
        LINKS.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let modes: Vec<(String, PortMode)> = IO_MODES.lock().unwrap().iter().map(|(k, v)| (k.clone(), *v)).collect();
    let names: Vec<(String, String)> =
        CHIP_NAMES.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();

    store.set("alu_state", serde_json::to_string(&state).unwrap_or_default());
    store.set("alu_links", serde_json::to_string(&links).unwrap_or_default());
    store.set("alu_io_modes", serde_json::to_string(&modes).unwrap_or_default());
    store.set("alu_chip_names", serde_json::to_string(&names).unwrap_or_default());
    let dims: Vec<((i32, i32, i32), String)> =
        crate::commands::ALU_DIMS.lock().unwrap().iter().map(|(k, v)| (*k, v.clone())).collect();
    store.set("alu_dims", serde_json::to_string(&dims).unwrap_or_default());
    let _ = store.flush();
}

pub fn load_state(srv: &dyn yog_api::Server) {
    use crate::commands::install_chip;
    use crate::chip::ChipMeta;
    let game_dir = srv.game_dir();
    let store = yog_api::Storage::open(&game_dir, crate::MOD_ID);

    let state: Vec<((i32, i32, i32), Vec<(String, Tier)>)> =
        store.get("alu_state").and_then(|v| v.as_str()).and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default();
    let names: std::collections::HashMap<String, String> =
        store.get("alu_chip_names").and_then(|v| v.as_str())
            .and_then(|j| serde_json::from_str::<Vec<(String, String)>>(j).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
    if let Some(dims) = store.get("alu_dims").and_then(|v| v.as_str())
        .and_then(|j| serde_json::from_str::<Vec<((i32, i32, i32), String)>>(j).ok())
    {
        *crate::commands::ALU_DIMS.lock().unwrap() = dims.into_iter().collect();
    }
    if let Some(links) = store.get("alu_links").and_then(|v| v.as_str())
        .and_then(|j| serde_json::from_str::<Vec<((String, String), (String, String))>>(j).ok())
    {
        *LINKS.lock().unwrap() = links.into_iter().collect();
    }
    if let Some(modes) = store.get("alu_io_modes").and_then(|v| v.as_str())
        .and_then(|j| serde_json::from_str::<Vec<(String, PortMode)>>(j).ok())
    {
        *IO_MODES.lock().unwrap() = modes.into_iter().collect();
    }

    for (alu_pos, chips) in state {
        for (chip_id, tier) in chips {
            let name = names.get(&chip_id).cloned().unwrap_or_else(|| chip_id.clone());
            install_chip(srv, &ChipMeta { id: chip_id, tier, name, ports: Vec::new() }, alu_pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chip::Port;

    fn port(label: &str, dir: PortMode) -> Port {
        Port { label: label.into(), x: 0, y: 0, z: 0, dir }
    }

    #[test]
    fn links_two_picks_and_auto_link() {
        let pos = (900, 64, 900);
        ALU_STATE.lock().unwrap().insert(pos, vec![("chip_a".into(), Tier::Wood), ("chip_b".into(), Tier::Wood)]);
        CHIP_PORTS.lock().unwrap().insert("chip_a".into(), vec![port("clk", PortMode::Output)]);
        CHIP_PORTS.lock().unwrap().insert("chip_b".into(), vec![port("clk", PortMode::Input), port("rst", PortMode::Input)]);

        let mut vm = AluVm::default();
        vm.open_at(pos, Tier::Wood);
        // Header, pin, header, pin, pin.
        assert_eq!(vm.pins().len(), 5);
        vm.select_pin("chip_a".into(), "clk".into());
        assert!(vm.pins()[1].text.starts_with('▶'));
        vm.select_pin("chip_b".into(), "rst".into());
        assert_eq!(LINKS.lock().unwrap().get(&("chip_a".into(), "clk".into())), Some(&("chip_b".into(), "rst".into())));

        vm.auto_link();
        assert_eq!(LINKS.lock().unwrap().get(&("chip_a".into(), "clk".into())), Some(&("chip_b".into(), "clk".into())));

        vm.cycle_mode("N");
        assert_eq!(vm.ext_ports()[0].mode, PortMode::Output);
    }
}
