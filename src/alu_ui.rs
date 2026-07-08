//! ALU UI — node editor flow-graph for chip linking and I/O configuration.
//!
//! Layout:
//! ┌──────────────────────────────────────────────┐
//! │ ALU (Netherite)  Chips: 2/8  Channels: 64  │
//! ├────────┬──────────────────────┬─────────────┤
//! │ Inputs │    Installed Chips   │   Outputs   │
//! │  ◉ N   │  [Adder] A0→SUM    │   ● N       │
//! │  ◉ S   │  [Clock] CLK→Q     │   ● S       │
//! │  ...   │                    │   ...       │
//! ├────────┴──────────────────────┴─────────────┤
//! │ [+Chip]  [Auto-link]  [Save]               │
//! └──────────────────────────────────────────────┘
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

use yog_api::{GfxContext, Registry};

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

/// Row rectangles for port/chip hit-testing: (label, x0, y0, x1, y1)
static HIT_RECTS: Mutex<Vec<(String, f32, f32, f32, f32)>> = Mutex::new(Vec::new());

/// Selected source port (for link creation): (chip_id, port_label)
static SELECTED_SRC: Mutex<Option<(String, String)>> = Mutex::new(None);

// ── Constants ────────────────────────────────────────────────────────────────

const PAD: f32 = 8.0;
const TITLE_H: f32 = 24.0;
const CHAR_W: f32 = 6.0;
const LINE_H: f32 = 11.0;
const CHIP_W: f32 = 140.0;
const CHIP_HEADER_H: f32 = 18.0;
const PIN_H: f32 = 14.0;

// Colors
const BG: u32 = 0xFF_1A1A1A;
const BG_LIGHT: u32 = 0xFF_252525;
const BORDER: u32 = 0xFF_404040;
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
    let d2d = gfx.draw2d();
    let (sw_i, sh_i) = gfx.screen_size();
    let sw = sw_i as f32;
    let sh = sh_i as f32;

    let chips = refresh_chips();
    let selected = SELECTED_SRC.lock().unwrap().clone();
    let alu_pos = ALU_POS.lock().unwrap().unwrap_or((0, 0, 0));

    // Panel: 90% of screen, centered
    let pw = sw * 0.9;
    let ph = sh * 0.85;
    let px = (sw - pw) / 2.0;
    let py = (sh - ph) / 2.0;

    // Background
    d2d.rect(px, py, px + pw, py + ph, BG);
    d2d.rect(px - 1.0, py, px + pw + 1.0, py, BORDER);
    d2d.rect(px, py + ph, px + pw, py + ph + 1.0, BORDER);

    // Title bar
    d2d.rect(px, py, px + pw, py + TITLE_H, BG_LIGHT);
    d2d.text(&format!("ALU  Chips: {}  |  Links: {}",
        chips.len(), LINKS.lock().unwrap().len()),
        px + 8.0, py + 4.0, TEXT_BRIGHT, true);

    // Close
    let close_x = px + pw - 16.0;
    d2d.rect(close_x - 2.0, py + 2.0, close_x + 10.0, py + TITLE_H - 2.0, 0xFF_553333);
    d2d.text("X", close_x, py + 4.0, 0xFF_FF4444, true);

    let mut hit_rects = Vec::new();

    if *SHOW_SELECTOR.lock().unwrap() {
        render_chip_selector(&d2d, px, py, pw, ph, &mut hit_rects);
        *HIT_RECTS.lock().unwrap() = hit_rects;
        return;
    }

    // ── Left: External ALU ports (N/S/E/W/U/D) ─────────────────────────────
    let left_x = px + PAD;
    let left_w = 110.0;
    let list_y = py + TITLE_H + 8.0;
    let list_h = ph - TITLE_H - 50.0;

    d2d.rect(left_x, list_y, left_x + left_w, list_y + list_h, SLOT_BG);
    d2d.text("External ports", left_x + 4.0, list_y - LINE_H, ACCENT, true);

    let ext_chip = ext_id(alu_pos);
    let sides = crate::alu::EXT_SIDES;
    let mut py_port = list_y + 4.0;
    for side in &sides {
        let mode_key = format!("{}:{}", ext_chip, side);
        let mode = IO_MODES.lock().unwrap().get(&mode_key).cloned().unwrap_or_else(|| "Input".to_string());
        let color = match mode.as_str() {
            "Output" => PIN_OUT, "Bidirectional" => PIN_BIDI, _ => PIN_IN,
        };
        let is_selected = selected.as_ref().map_or(false, |(cid, pl)| cid == &ext_chip && pl == side);
        let marker = if is_selected { "▶" } else { "◉" };
        d2d.text(&format!("{} {}", marker, side), left_x + 4.0, py_port, if is_selected { SEL_HIGHLIGHT } else { color }, false);
        d2d.text(&mode, left_x + left_w - 56.0, py_port, TEXT_DIM, false);
        hit_rects.push((format!("alu_pin_{}", side), left_x, py_port, left_x + left_w - 58.0, py_port + LINE_H));
        hit_rects.push((format!("alu_mode_{}", side), left_x + left_w - 58.0, py_port, left_x + left_w, py_port + LINE_H));
        py_port += LINE_H + 6.0;
    }

    // ── Center: Installed chips ────────────────────────────────────────────
    let center_x = left_x + left_w + 12.0;
    let center_w = pw - left_w - 120.0;

    d2d.text("Chips", center_x, list_y - LINE_H, ACCENT, true);

    let mut cx = center_x;
    let mut cy = list_y;
    for (chip_id, tier) in chips.iter() {
        if cx + CHIP_W > center_x + center_w {
            cx = center_x;
            cy += 120.0;
        }

        let name = CHIP_NAMES.lock().unwrap().get(chip_id).cloned()
            .unwrap_or_else(|| format!("Chip {}", &chip_id[..6.min(chip_id.len())]));

        d2d.rect(cx, cy, cx + CHIP_W, cy + CHIP_HEADER_H, BG_LIGHT);
        d2d.rect(cx, cy, cx + CHIP_W, cy + 1.0, BORDER);
        d2d.text(&format!("[{}] {}", tier.name(), name), cx + 4.0, cy + 3.0, TEXT_BRIGHT, false);

        let ports = CHIP_PORTS.lock().unwrap().get(chip_id).cloned().unwrap_or_default();

        let mut pin_y = cy + CHIP_HEADER_H + 2.0;
        for port in &ports {
            let is_selected = selected.as_ref().map_or(false, |(cid, pl)| cid == chip_id && pl == &port.label);
            let pin_color = match port.dir {
                crate::chip::PortDir::Input => PIN_IN,
                crate::chip::PortDir::Output => PIN_OUT,
                crate::chip::PortDir::Bidirectional => PIN_BIDI,
            };
            let marker = if is_selected { "▶" } else { " " };
            let label = format!("{} {} {}", marker, port.dir.name(), port.label);
            d2d.text(&label, cx + 4.0, pin_y, if is_selected { SEL_HIGHLIGHT } else { pin_color }, false);
            hit_rects.push((format!("pin_{}_{}", chip_id, port.label), cx, pin_y, cx + CHIP_W, pin_y + PIN_H));
            pin_y += PIN_H + 2.0;
        }

        let chip_bottom = cy + CHIP_HEADER_H + ports.len() as f32 * (PIN_H + 2.0) + 4.0;
        d2d.rect(cx, chip_bottom, cx + CHIP_W, chip_bottom + 1.0, BORDER);

        cx += CHIP_W + 12.0;
    }

    // Links display
    let links = LINKS.lock().unwrap();
    let mut link_y = cy + 130.0;
    if !links.is_empty() {
        d2d.text("Links:", center_x, link_y, ACCENT, true);
        link_y += LINE_H;
        let names = CHIP_NAMES.lock().unwrap();
        for ((src_id, src_port), (tgt_id, tgt_port)) in links.iter().take(10) {
            let display = |id: &str| -> String {
                if id.starts_with("__ext_") { "ALU".to_string() }
                else { names.get(id).cloned().unwrap_or_else(|| id[..6.min(id.len())].to_string()) }
            };
            d2d.text(&format!("{}.{} → {}.{}", display(src_id), src_port, display(tgt_id), tgt_port),
                center_x, link_y, TEXT_DIM, false);
            link_y += LINE_H;
        }
    }
    drop(links);

    // ── Buttons ─────────────────────────────────────────────────────────────
    let btn_y = py + ph - 24.0;
    let btns = ["[+ Add Chip]", "[Auto-link]", "[Save]"];
    let mut bx = left_x;
    for label in &btns {
        let bw = label.len() as f32 * CHAR_W + 12.0;
        d2d.rect(bx, btn_y, bx + bw, btn_y + 20.0, BTN_BG);
        d2d.text(label, bx + 6.0, btn_y + 3.0, TEXT_BRIGHT, false);
        hit_rects.push((label.to_string(), bx, btn_y, bx + bw, btn_y + 20.0));
        bx += bw + 8.0;
    }

    hit_rects.push(("close".into(), close_x - 2.0, py + 2.0, close_x + 10.0, py + TITLE_H - 2.0));

    *HIT_RECTS.lock().unwrap() = hit_rects;
}

fn render_chip_selector(d2d: &yog_api::gfx_draw2d::Draw2D, px: f32, py: f32, pw: f32, ph: f32, hit_rects: &mut Vec<(String, f32, f32, f32, f32)>) {
    d2d.text("Select a chip to install:", px + PAD, py + TITLE_H + 4.0, ACCENT, true);
    let list = CHIP_LIST.lock().unwrap();
    let mut row_y = py + TITLE_H + 20.0;
    if list.is_empty() {
        d2d.text("(no programmed chips in your inventory)", px + PAD, row_y, TEXT_DIM, false);
    }
    for (slot, tier, name) in list.iter() {
        d2d.rect(px + PAD, row_y, px + pw - PAD, row_y + LINE_H + 4.0, BG_LIGHT);
        d2d.text(&format!("[{}] {}", tier, name), px + PAD + 4.0, row_y + 2.0, TEXT_BRIGHT, false);
        hit_rects.push((format!("select_chip_{}", slot), px + PAD, row_y, px + pw - PAD, row_y + LINE_H + 4.0));
        row_y += LINE_H + 6.0;
        if row_y > py + ph - 24.0 { break; }
    }
    hit_rects.push(("cancel_selector".into(), px + PAD, py + ph - 24.0, px + PAD + 80.0, py + ph - 4.0));
    d2d.rect(px + PAD, py + ph - 24.0, px + PAD + 80.0, py + ph - 4.0, BTN_BG);
    d2d.text("[Cancel]", px + PAD + 4.0, py + ph - 21.0, TEXT_BRIGHT, false);
    hit_rects.push(("close".into(), px + pw - 18.0, py + 2.0, px + pw - 6.0, py + TITLE_H - 2.0));
}

/// Handle click events.
pub fn handle_click(_ui_id: &str, event: &str) {
    if event == "close" { clear(); return; }

    if let Some(rest) = event.strip_prefix("click:") {
        let mut parts = rest.splitn(2, ':');
        let mx: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let my: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);

        let rects = HIT_RECTS.lock().unwrap();
        for (label, rx0, ry0, rx1, ry1) in rects.iter() {
            if mx >= *rx0 && mx <= *rx1 && my >= *ry0 && my <= *ry1 {
                let label = label.clone();
                drop(rects);
                handle_hit(&label);
                return;
            }
        }
    }
}

fn handle_hit(label: &str) {
    let alu_pos = ALU_POS.lock().unwrap().unwrap_or((0, 0, 0));

    if label == "cancel_selector" {
        *SHOW_SELECTOR.lock().unwrap() = false;
        return;
    }
    if let Some(slot) = label.strip_prefix("select_chip_") {
        *SHOW_SELECTOR.lock().unwrap() = false;
        network::send_alu_action(&["install", slot, &alu_pos.0.to_string(), &alu_pos.1.to_string(), &alu_pos.2.to_string()]);
        return;
    }

    match label {
        "close" => clear(),
        "[+ Add Chip]" => {
            network::request_chip_list();
        }
        "[Auto-link]" => {
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
        "[Save]" => {
            network::send_alu_action(&["save_links"]);
        }
        _ => {
            if let Some(rest) = label.strip_prefix("pin_") {
                if let Some((chip_id, port_label)) = rest.split_once('_') {
                    select_pin(chip_id.to_string(), port_label.to_string());
                }
            } else if let Some(side) = label.strip_prefix("alu_pin_") {
                select_pin(ext_id(alu_pos), side.to_string());
            } else if let Some(side) = label.strip_prefix("alu_mode_") {
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
