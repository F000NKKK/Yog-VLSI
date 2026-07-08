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

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yog_api::{GfxContext, Registry, Storage};

use crate::chip::ChipMeta;
use crate::commands::{ALU_STATE, VM_CACHE};
use crate::vm::{RedstoneVM, Tier};

pub const CHIP_LIST_CHANNEL: &str = "yog-vlsi:alu_chips";

/// Chips available in the player's inventory, as reported by the server:
/// (slot, name, tier_id).
static CHIP_LIST: LazyLock<Mutex<Vec<(u32, String, String)>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static SHOW_SELECTOR: Mutex<bool> = Mutex::new(false);

// ── State ────────────────────────────────────────────────────────────────────

/// Link graph: (source_chip_id, source_port_label) → (target_chip_id, target_port_label)
pub static LINKS: LazyLock<Mutex<HashMap<(String, String), (String, String)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// I/O mode per ALU external port: (side) → Input/Output/Bidirectional
pub static IO_MODES: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Chip info cache: chip_id → ChipMeta (set before render)
static CHIP_INFO: LazyLock<Mutex<HashMap<String, ChipMeta>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

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
const PORT_SIZE: f32 = 12.0;
const CHIP_W: f32 = 140.0;
const CHIP_HEADER_H: f32 = 18.0;
const PIN_H: f32 = 14.0;

// Colors
const BG: u32 = 0xFF_1A1A1A;
const BG_LIGHT: u32 = 0xFF_252525;
const BORDER: u32 = 0xFF_404040;
const ACCENT: u32 = 0xFF_1E5A99;
const TEXT: u32 = 0xFF_CCCCCC;
const TEXT_BRIGHT: u32 = 0xFF_FFFFFF;
const TEXT_DIM: u32 = 0xFF_777777;
const PIN_IN: u32 = 0xFF_3366CC;
const PIN_OUT: u32 = 0xFF_CC3333;
const PIN_BIDI: u32 = 0xFF_9933CC;
const SEL_HIGHLIGHT: u32 = 0xFF_FFD700;

// ── Public API ───────────────────────────────────────────────────────────────

pub fn set_alu_pos(pos: (i32, i32, i32)) {
    *ALU_POS.lock().unwrap() = Some(pos);
}

pub fn clear() {
    *ALU_POS.lock().unwrap() = None;
    *SELECTED_SRC.lock().unwrap() = None;
}

/// Update cached chip info from the ALU state.
fn refresh_chips() -> Vec<(String, Tier)> {
    let pos = ALU_POS.lock().unwrap();
    if let Some(p) = *pos {
        let state = ALU_STATE.lock().unwrap();
        state.get(&p).cloned().unwrap_or_default()
    } else {
        Vec::new()
    }
}

/// Render the ALU node editor.
pub fn render(gfx: &GfxContext) {
    let d2d = gfx.draw2d();
    let (sw_i, sh_i) = gfx.screen_size();
    let sw = sw_i as f32;
    let sh = sh_i as f32;

    let chips = refresh_chips();
    let selected = SELECTED_SRC.lock().unwrap().clone();

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

    // ── Left: External inputs ──────────────────────────────────────────────
    let left_x = px + PAD;
    let left_w = 100.0;
    let list_y = py + TITLE_H + 8.0;
    let list_h = ph - TITLE_H - 50.0;

    d2d.rect(left_x, list_y, left_x + left_w, list_y + list_h, SLOT_BG);
    d2d.text("Inputs", left_x + 4.0, list_y - LINE_H, ACCENT, true);

    let mut hit_rects = Vec::new();
    let sides = ["N", "S", "E", "W", "U", "D"];
    let mut py_port = list_y + 4.0;
    for side in &sides {
        let mode = IO_MODES.lock().unwrap()
            .get(*side).cloned().unwrap_or_else(|| "Input".to_string());
        let color = match mode.as_str() {
            "Output" => PIN_OUT, "Bidirectional" => PIN_BIDI, _ => PIN_IN,
        };
        let label = format!("◉ {}", side);
        d2d.text(&label, left_x + 4.0, py_port, color, false);
        d2d.text(&mode, left_x + left_w - 50.0, py_port, TEXT_DIM, false);
        hit_rects.push((format!("alu_in_{}", side), left_x, py_port, left_x + left_w, py_port + LINE_H));
        py_port += LINE_H + 6.0;
    }

    // ── Center: Installed chips ────────────────────────────────────────────
    let center_x = left_x + left_w + 12.0;
    let center_w = pw - left_w - 120.0;

    d2d.text("Chips", center_x, list_y - LINE_H, ACCENT, true);

    let mut cx = center_x;
    let mut cy = list_y;
    for (i, (chip_id, tier)) in chips.iter().enumerate() {
        if cx + CHIP_W > center_x + center_w {
            cx = center_x;
            cy += 120.0;
        }

        let info = CHIP_INFO.lock().unwrap();
        let name = info.get(chip_id).map(|m| m.name.clone()).unwrap_or_else(|| format!("Chip {}", &chip_id[..6]));
        drop(info);

        // Chip background
        d2d.rect(cx, cy, cx + CHIP_W, cy + CHIP_HEADER_H, BG_LIGHT);
        d2d.rect(cx, cy, cx + CHIP_W, cy + 1.0, BORDER);
        d2d.text(&format!("[{}] {}", tier.name(), name), cx + 4.0, cy + 3.0, TEXT_BRIGHT, false);

        // Port pins (simplified: show from cached info)
        let info = CHIP_INFO.lock().unwrap();
        let ports: Vec<_> = info.get(chip_id)
            .map(|m| m.ports.clone()).unwrap_or_default();
        drop(info);

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

        // Bottom border
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
        for ((src_id, src_port), (tgt_id, tgt_port)) in links.iter().take(10) {
            let src_name = CHIP_INFO.lock().unwrap().get(src_id)
                .map(|m| m.name.clone()).unwrap_or_else(|| src_id[..6].to_string());
            let tgt_name = CHIP_INFO.lock().unwrap().get(tgt_id)
                .map(|m| m.name.clone()).unwrap_or_else(|| tgt_id[..6].to_string());
            d2d.text(&format!("{}.{} → {}.{}", src_name, src_port, tgt_name, tgt_port),
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

    // Close button
    hit_rects.push(("close".into(), close_x - 2.0, py + 2.0, close_x + 10.0, py + TITLE_H - 2.0));

    *HIT_RECTS.lock().unwrap() = hit_rects;
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
                handle_hit(label);
                return;
            }
        }
    }
}

fn handle_hit(label: &str) {
    match label {
        "close" => clear(),
        "[+ Add Chip]" => { /* TODO: open chip selection */ }
        "[Auto-link]" => {
            // Auto-link ports with matching labels
            let chips = refresh_chips();
            let mut links = LINKS.lock().unwrap();
            let info = CHIP_INFO.lock().unwrap();

            for i in 0..chips.len() {
                for j in i+1..chips.len() {
                    let ports_i = info.get(&chips[i].0).map(|m| m.ports.clone()).unwrap_or_default();
                    let ports_j = info.get(&chips[j].0).map(|m| m.ports.clone()).unwrap_or_default();
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
        "[Save]" => { /* TODO: persist link graph */ }
        _ => {
            // Handle pin clicks for link creation
            if label.starts_with("pin_") {
                let rest = &label[4..];
                if let Some((chip_id, port_label)) = rest.split_once('_') {
                    let (cid, pl) = (chip_id.to_string(), port_label.to_string());
                    let mut sel = SELECTED_SRC.lock().unwrap();
                    match sel.clone() {
                        None => *sel = Some((cid, pl)),
                        Some((src_id, src_label)) => {
                            // Create link: src → target
                            LINKS.lock().unwrap().insert(
                                (src_id.clone(), src_label.clone()),
                                (cid.clone(), pl.clone()),
                            );
                            *sel = None;
                        }
                    }
                }
            }
            // ALU input port click — toggle mode
            if label.starts_with("alu_in_") {
                let side = &label[7..];
                let mut modes = IO_MODES.lock().unwrap();
                let current = modes.get(side).cloned().unwrap_or_else(|| "Input".into());
                let next = match current.as_str() {
                    "Input" => "Output", "Output" => "Bidirectional", _ => "Input",
                };
                modes.insert(side.to_string(), next.to_string());
            }
        }
    }
}

// ── Colors ───────────────────────────────────────────────────────────────────

const SLOT_BG: u32 = 0xFF_0D0D0D;
const BTN_BG: u32 = 0xFF_333333;
