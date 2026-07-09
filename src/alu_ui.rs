//! ALU UI — node editor flow-graph for chip linking and I/O configuration.
//!
//! Built on `yog_ui`'s flexbox/dock layout engine (the same one `yog-book`
//! uses) instead of hand-rolled pixel math, so it reflows correctly at any
//! screen size / GUI scale instead of drifting.
//!
//! Reads (`ALU_STATE`, `CHIP_PORTS`, `LINKS`, `IO_MODES` — all in
//! `commands`) hit the mod's own shared statics directly: this UI and the
//! server tick loop live in the same process for the primary target
//! (integrated/singleplayer), so that's a real, live view, not a stale
//! cache. Anything that has to go through the loader's `Server` trait
//! (inventory access, item consumption) is a no-op when called from this
//! client-side click handler, so those actions round-trip through
//! `network::send_alu_action` instead.

use std::sync::{LazyLock, Mutex};

use yog_api::{widget, Align, Dock, FlexDir, GfxContext, Registry, UiRoot};

use crate::commands::{ALU_STATE, CHIP_NAMES, CHIP_PORTS, IO_MODES, LINKS};
use crate::network;
use crate::vm::Tier;

/// Server → client reply channel for the chip-selector inventory listing.
pub const CHIP_LIST_CHANNEL: &str = "yog-vlsi:alu_chiplist";

/// Chips available in the player's inventory, as reported by the server:
/// (slot, name, tier_id).
static CHIP_LIST: LazyLock<Mutex<Vec<(u32, String, String)>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static SHOW_SELECTOR: Mutex<bool> = Mutex::new(false);

/// ALU position for this UI session.
static ALU_POS: Mutex<Option<(i32, i32, i32)>> = Mutex::new(None);

/// Selected source port (for link creation): (chip_id, port_label)
static SELECTED_SRC: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Layout tree from the last rendered frame, hit-tested on click.
static LAST_UI: Mutex<Option<UiRoot>> = Mutex::new(None);

// ── Constants ────────────────────────────────────────────────────────────────

const PAD: f32 = 8.0;
const TITLE_H: f32 = 24.0;
const LEFT_W: f32 = 120.0;
const CHIP_W: f32 = 140.0;
const ROW_H: f32 = 14.0;
const BTN_H: f32 = 20.0;
const BTN_BAR_H: f32 = BTN_H + 12.0;

// Colors
const BG: u32 = 0xFF_1A1A1A;
const BG_LIGHT: u32 = 0xFF_252525;
const ACCENT: u32 = 0xFF_1E5A99;
const TEXT_BRIGHT: u32 = 0xFF_FFFFFF;
const TEXT_DIM: u32 = 0xFF_777777;
const PIN_IN: u32 = 0xFF_3366CC;
const PIN_OUT: u32 = 0xFF_CC3333;
const PIN_BIDI: u32 = 0xFF_9933CC;
const SEL_HIGHLIGHT: u32 = 0xFF_FFD700;
const SLOT_BG: u32 = 0xFF_0D0D0D;
const BTN_BG: u32 = 0xFF_333333;

// ── Public API ───────────────────────────────────────────────────────────────

pub fn set_alu_pos(pos: (i32, i32, i32)) {
    *ALU_POS.lock().unwrap() = Some(pos);
}

pub fn clear() {
    *ALU_POS.lock().unwrap() = None;
    *SELECTED_SRC.lock().unwrap() = None;
    *SHOW_SELECTOR.lock().unwrap() = false;
}

/// Register the client-side reply handler for the chip selector.
pub fn register(registry: &mut Registry) {
    registry.on_client_packet(CHIP_LIST_CHANNEL, |ev, _srv| {
        let text = String::from_utf8_lossy(&ev.payload);
        let list: Vec<(u32, String, String)> = text.lines().filter_map(|line| {
            let mut f = line.split('\t');
            let slot: u32 = f.next()?.parse().ok()?;
            let tier = f.next()?.to_string();
            let name = f.next().unwrap_or("chip").to_string();
            Some((slot, tier, name))
        }).collect();
        *CHIP_LIST.lock().unwrap() = list;
        *SHOW_SELECTOR.lock().unwrap() = true;
    });
}

fn refresh_chips() -> Vec<(String, Tier)> {
    let pos = ALU_POS.lock().unwrap();
    if let Some(p) = *pos {
        ALU_STATE.lock().unwrap().get(&p).cloned().unwrap_or_default()
    } else {
        Vec::new()
    }
}

fn ext_id(pos: (i32, i32, i32)) -> String {
    crate::commands::ext_chip_id(pos)
}

/// Render the ALU node editor.
pub fn render(gfx: &GfxContext) {
    let (sw_i, sh_i) = gfx.screen_size();
    let sw = sw_i as f32;
    let sh = sh_i as f32;

    let pw = sw * 0.9;
    let ph = sh * 0.85;
    let px = (sw - pw) / 2.0;
    let py = (sh - ph) / 2.0;

    let root = if *SHOW_SELECTOR.lock().unwrap() {
        build_selector_tree(pw, ph)
    } else {
        build_editor_tree(pw, ph)
    };

    let screen_root = widget::panel(FlexDir::Row)
        .padding(py, 0.0, 0.0, px)
        .child(root);

    let mut ui = UiRoot::new("yog-vlsi:alu", screen_root);
    ui.layout(sw, sh);
    ui.render(gfx);
    *LAST_UI.lock().unwrap() = Some(ui);
}

fn title_bar() -> widget::Widget {
    let chips = refresh_chips();
    widget::panel(FlexDir::Row)
        .dock(Dock::Top).h(TITLE_H).bg(BG_LIGHT)
        .child(widget::label(format!("ALU  Chips: {}  |  Links: {}", chips.len(), LINKS.lock().unwrap().len()))
            .color(TEXT_BRIGHT).flex(1.0).padding(7.0, 0.0, 0.0, 8.0).no_wrap())
        .child(widget::button("X").on_click("close")
            .color(0xFF_FF4444).bg(0xFF_553333)
            .padding(4.0, 6.0, 4.0, 6.0).margin(2.0, 4.0, 2.0, 0.0))
}

fn build_selector_tree(pw: f32, ph: f32) -> widget::Widget {
    let body_h = ph - TITLE_H - BTN_BAR_H;
    let list = CHIP_LIST.lock().unwrap();

    let mut body = widget::panel(FlexDir::Column)
        .dock(Dock::Top).h(body_h).gap(4.0)
        .padding(6.0, PAD, 0.0, PAD)
        .child(widget::label("Select a chip to install:").color(ACCENT).no_wrap());

    if list.is_empty() {
        body = body.child(widget::label("(no programmed chips in your inventory)").color(TEXT_DIM).shadow(false));
    } else {
        for (slot, tier, name) in list.iter() {
            body = body.child(
                widget::button(format!("[{tier}] {name}"))
                    .on_click(format!("select_chip:{slot}"))
                    .h(ROW_H + 4.0).bg(BG_LIGHT).color(TEXT_BRIGHT).shadow(false).no_wrap()
            );
        }
    }
    drop(list);

    let button_bar = widget::panel(FlexDir::Row)
        .dock(Dock::Bottom).h(BTN_BAR_H).padding(0.0, PAD, 0.0, PAD).align(Align::Center)
        .child(widget::button("Cancel").on_click("cancel_selector")
            .h(BTN_H).bg(BTN_BG).color(TEXT_BRIGHT).padding(4.0, 8.0, 4.0, 8.0));

    widget::panel(FlexDir::Column)
        .w(pw).h(ph).bg(BG)
        .child(title_bar())
        .child(body)
        .child(button_bar)
}

fn build_editor_tree(pw: f32, ph: f32) -> widget::Widget {
    let body_h = ph - TITLE_H - BTN_BAR_H;
    let center_w = (pw - LEFT_W - PAD * 3.0).max(CHIP_W);

    let body = widget::panel(FlexDir::Row)
        .dock(Dock::Top).h(body_h).gap(PAD)
        .padding(6.0, PAD, 0.0, PAD)
        .child(build_ports_panel(body_h))
        .child(build_chips_panel(center_w, body_h));

    let button_bar = widget::panel(FlexDir::Row)
        .dock(Dock::Bottom).h(BTN_BAR_H).gap(6.0)
        .padding(0.0, PAD, 0.0, PAD).align(Align::Center)
        .child(widget::button("+ Add Chip").on_click("add_chip")
            .h(BTN_H).bg(BTN_BG).color(TEXT_BRIGHT).padding(4.0, 8.0, 4.0, 8.0))
        .child(widget::button("Auto-link").on_click("auto_link")
            .h(BTN_H).bg(BTN_BG).color(TEXT_BRIGHT).padding(4.0, 8.0, 4.0, 8.0))
        .child(widget::button("Save").on_click("save_links")
            .h(BTN_H).bg(BTN_BG).color(TEXT_BRIGHT).padding(4.0, 8.0, 4.0, 8.0));

    widget::panel(FlexDir::Column)
        .w(pw).h(ph).bg(BG)
        .child(title_bar())
        .child(body)
        .child(button_bar)
}

fn build_ports_panel(body_h: f32) -> widget::Widget {
    let alu_pos = ALU_POS.lock().unwrap().unwrap_or((0, 0, 0));
    let ext_chip = ext_id(alu_pos);
    let selected = SELECTED_SRC.lock().unwrap().clone();

    let label_h = 14.0;
    let list_h = (body_h - label_h).max(0.0);

    let mut list = widget::panel(FlexDir::Column).w(LEFT_W).h(list_h).bg(SLOT_BG).gap(4.0).padding(4.0, 4.0, 4.0, 4.0);
    for side in &crate::alu::EXT_SIDES {
        let mode_key = format!("{}:{}", ext_chip, side);
        let mode = IO_MODES.lock().unwrap().get(&mode_key).cloned().unwrap_or_else(|| "Input".to_string());
        let color = match mode.as_str() {
            "Output" => PIN_OUT, "Bidirectional" => PIN_BIDI, _ => PIN_IN,
        };
        let is_selected = selected.as_ref().map_or(false, |(cid, pl)| cid == &ext_chip && pl == side);
        let marker = if is_selected { "▶" } else { "◉" };

        let row = widget::panel(FlexDir::Row).h(ROW_H).gap(2.0)
            .child(widget::button(format!("{marker} {side}"))
                .on_click(format!("alu_pin:{side}")).flex(1.0)
                .color(if is_selected { SEL_HIGHLIGHT } else { color }).shadow(false).no_wrap())
            .child(widget::button(mode)
                .on_click(format!("alu_mode:{side}")).w(56.0)
                .color(TEXT_DIM).shadow(false).no_wrap());
        list = list.child(row);
    }

    widget::panel(FlexDir::Column)
        .child(widget::label("External ports").color(ACCENT).no_wrap())
        .child(list)
}

fn build_chips_panel(center_w: f32, body_h: f32) -> widget::Widget {
    let chips = refresh_chips();
    let selected = SELECTED_SRC.lock().unwrap().clone();
    let names = CHIP_NAMES.lock().unwrap();
    let ports_map = CHIP_PORTS.lock().unwrap();

    let cols_per_row = (((center_w + PAD) / (CHIP_W + PAD)).floor() as usize).max(1);

    let mut grid = widget::panel(FlexDir::Column).gap(8.0);
    for row_chips in chips.chunks(cols_per_row) {
        let mut row = widget::panel(FlexDir::Row).gap(12.0);
        for (chip_id, tier) in row_chips {
            let name = names.get(chip_id).cloned()
                .unwrap_or_else(|| format!("Chip {}", &chip_id[..6.min(chip_id.len())]));
            let ports = ports_map.get(chip_id).cloned().unwrap_or_default();

            let mut card = widget::panel(FlexDir::Column).w(CHIP_W)
                .child(widget::label(format!("[{}] {}", tier.name(), name))
                    .color(TEXT_BRIGHT).bg(BG_LIGHT).shadow(false).no_wrap());

            for port in &ports {
                let is_selected = selected.as_ref().map_or(false, |(cid, pl)| cid == chip_id && pl == &port.label);
                let pin_color = match port.dir {
                    crate::chip::PortDir::Input => PIN_IN,
                    crate::chip::PortDir::Output => PIN_OUT,
                    crate::chip::PortDir::Bidirectional => PIN_BIDI,
                };
                let marker = if is_selected { "▶" } else { " " };
                card = card.child(
                    widget::button(format!("{marker} {} {}", port.dir.name(), port.label))
                        .on_click(format!("pin:{chip_id}:{}", port.label))
                        .h(ROW_H)
                        .color(if is_selected { SEL_HIGHLIGHT } else { pin_color })
                        .shadow(false).no_wrap()
                );
            }
            row = row.child(card);
        }
        grid = grid.child(row);
    }
    drop(names);
    drop(ports_map);

    let mut links_col = widget::panel(FlexDir::Column).gap(1.0);
    let links = LINKS.lock().unwrap();
    if !links.is_empty() {
        let names = CHIP_NAMES.lock().unwrap();
        links_col = links_col.child(widget::label("Links:").color(ACCENT).no_wrap());
        for ((src_id, src_port), (tgt_id, tgt_port)) in links.iter().take(10) {
            let display = |id: &str| -> String {
                if id.starts_with("__ext_") { "ALU".to_string() }
                else { names.get(id).cloned().unwrap_or_else(|| id[..6.min(id.len())].to_string()) }
            };
            links_col = links_col.child(
                widget::label(format!("{}.{} → {}.{}", display(src_id), src_port, display(tgt_id), tgt_port))
                    .color(TEXT_DIM).shadow(false).no_wrap()
            );
        }
    }
    drop(links);

    let list_h = (body_h - 14.0).max(0.0);
    let scroll_area = widget::panel(FlexDir::Column).h(list_h).gap(8.0)
        .child(grid)
        .child(links_col);

    widget::panel(FlexDir::Column).flex(1.0)
        .child(widget::label("Chips").color(ACCENT).no_wrap())
        .child(scroll_area)
}

/// Handle click events.
pub fn handle_click(_ui_id: &str, event: &str) {
    if event == "close" { clear(); return; }

    if let Some(rest) = event.strip_prefix("click:") {
        let mut parts = rest.splitn(2, ':');
        let mx: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let my: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);

        let hit = LAST_UI.lock().unwrap().as_ref()
            .and_then(|ui| ui.hit_test(mx, my))
            .and_then(|n| n.on_click.clone());

        if let Some(action) = hit {
            handle_hit(&action);
        }
    }
}

fn handle_hit(label: &str) {
    let alu_pos = ALU_POS.lock().unwrap().unwrap_or((0, 0, 0));

    if label == "cancel_selector" {
        *SHOW_SELECTOR.lock().unwrap() = false;
        return;
    }
    if let Some(slot) = label.strip_prefix("select_chip:") {
        *SHOW_SELECTOR.lock().unwrap() = false;
        network::send_alu_action(&["install", slot, &alu_pos.0.to_string(), &alu_pos.1.to_string(), &alu_pos.2.to_string()]);
        return;
    }

    match label {
        "close" => clear(),
        "add_chip" => {
            network::request_chip_list();
        }
        "auto_link" => {
            let chips = refresh_chips();
            let mut links = LINKS.lock().unwrap();
            let ports = CHIP_PORTS.lock().unwrap();

            for i in 0..chips.len() {
                for j in i+1..chips.len() {
                    let ports_i = ports.get(&chips[i].0).cloned().unwrap_or_default();
                    let ports_j = ports.get(&chips[j].0).cloned().unwrap_or_default();
                    for pi in &ports_i {
                        for pj in &ports_j {
                            if pi.label == pj.label {
                                links.insert(
                                    (chips[i].0.clone(), pi.label.clone()),
                                    (chips[j].0.clone(), pj.label.clone()),
                                );
                            }
                        }
                    }
                }
            }
        }
        "save_links" => {
            network::send_alu_action(&["save_links"]);
        }
        _ => {
            if let Some(rest) = label.strip_prefix("pin:") {
                if let Some((chip_id, port_label)) = rest.split_once(':') {
                    select_pin(chip_id.to_string(), port_label.to_string());
                }
            } else if let Some(side) = label.strip_prefix("alu_pin:") {
                select_pin(ext_id(alu_pos), side.to_string());
            } else if let Some(side) = label.strip_prefix("alu_mode:") {
                let key = format!("{}:{}", ext_id(alu_pos), side);
                let mut modes = IO_MODES.lock().unwrap();
                let current = modes.get(&key).cloned().unwrap_or_else(|| "Input".into());
                let next = match current.as_str() {
                    "Input" => "Output", "Output" => "Bidirectional", _ => "Input",
                };
                modes.insert(key, next.to_string());
            }
        }
    }
}

fn select_pin(chip_id: String, port_label: String) {
    let mut sel = SELECTED_SRC.lock().unwrap();
    match sel.clone() {
        None => *sel = Some((chip_id, port_label)),
        Some((src_id, src_label)) => {
            LINKS.lock().unwrap().insert((src_id, src_label), (chip_id, port_label));
            *sel = None;
        }
    }
}
