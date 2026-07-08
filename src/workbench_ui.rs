//! Workbench GUI — Ender IO-style interface with design library, resources, fabrication.

use std::collections::HashMap;
use std::sync::Mutex;

use yog_api::GfxContext;

use crate::designs::{list_designs, load_design, DesignMeta};
use crate::network;
use crate::workbench::RESOURCES;

/// Last known player position, refreshed every frame — used so button
/// actions that round-trip to the server (e.g. entering the circuit editor)
/// can tell it where to teleport the player back to afterwards.
static LAST_PLAYER_POS: Mutex<(f32, f32, f32)> = Mutex::new((0.0, 64.0, 0.0));

// ── State ────────────────────────────────────────────────────────────────────

/// Scroll offset for the design list.
static SCROLL: Mutex<f32> = Mutex::new(0.0);

/// Currently selected design name.
static SELECTED: Mutex<Option<String>> = Mutex::new(None);

/// Row rectangles from last frame: (design_index, y0, y1).
static ROWS: Mutex<Vec<(usize, f32, f32)>> = Mutex::new(Vec::new());

/// Button rectangles from last frame:
/// (name, x0, y0, x1, y1)
static BUTTONS: Mutex<Vec<(&'static str, f32, f32, f32, f32)>> = Mutex::new(Vec::new());

/// Current player name and game dir for data access (set before each render).
static PLAYER: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));

/// True when the UI is active (tracked to allow refresh from commands).
pub static ACTIVE: Mutex<bool> = Mutex::new(false);

// ── Constants ────────────────────────────────────────────────────────────────

const PAD: f32 = 8.0;
const TITLE_H: f32 = 24.0;
const ROW_H: f32 = 18.0;
const BTN_W: f32 = 90.0;
const BTN_H: f32 = 20.0;
const CHAR_W: f32 = 6.0;
const LINE_H: f32 = 11.0;

// ── Colors (Ender IO palette) ────────────────────────────────────────────────

const BG: u32       = 0xFF_1A1A1A;
const BG_LIGHT: u32 = 0xFF_252525;
const BORDER: u32   = 0xFF_404040;
const ACCENT: u32   = 0xFF_1E5A99; // Ender IO blue
const TEXT: u32     = 0xFF_CCCCCC;
const TEXT_DIM: u32 = 0xFF_777777;
const TEXT_BRIGHT: u32 = 0xFF_FFFFFF;
const SLOT_BG: u32  = 0xFF_0D0D0D;
const BTN_BG: u32   = 0xFF_333333;
// const BTN_HOVER: u32 = 0xFF_444444;
const SEL_BG: u32   = 0x44_1E5A99;

// ── Public API ───────────────────────────────────────────────────────────────

/// Set player context before rendering.
pub fn set_player(game_dir: &str, player_name: &str) {
    let mut p = PLAYER.lock().unwrap();
    *p = (game_dir.to_string(), player_name.to_string());
    *ACTIVE.lock().unwrap() = true;
}

/// Clear player context (on UI close).
pub fn clear() {
    *ACTIVE.lock().unwrap() = false;
}

/// Show a local-only actionbar hint (no server round-trip needed).
fn hint(message: &str) {
    if let Some(srv) = yog_api::server() {
        let (_, player_name) = PLAYER.lock().unwrap().clone();
        srv.send_actionbar(&player_name, message);
    }
}

/// Render the workbench GUI.
pub fn render(gfx: &GfxContext) {
    let d2d = gfx.draw2d();
    let (sw_i, sh_i) = gfx.screen_size();
    let sw = sw_i as f32;
    let sh = sh_i as f32;

    let (game_dir, player_name) = PLAYER.lock().unwrap().clone();
    if player_name.is_empty() { return; }
    *LAST_PLAYER_POS.lock().unwrap() = (gfx.player_pos()[0], gfx.player_pos()[1], gfx.player_pos()[2]);

    let designs: Vec<DesignMeta> = list_designs(&game_dir, &player_name);
    let selected = SELECTED.lock().unwrap().clone();

    // ── Layout ──────────────────────────────────────────────────────────────
    // Panel: 80% of screen, centered
    let pw = sw * 0.8;
    let ph = sh * 0.8;
    let px = (sw - pw) / 2.0;
    let py = (sh - ph) / 2.0;

    // Panel background
    d2d.rect(px, py, px + pw, py + ph, BG);
    d2d.rect(px - 1.0, py - 1.0, px + pw + 1.0, py, BORDER); // top border
    d2d.rect(px, py + ph, px + pw, py + ph + 1.0, BORDER);    // bottom border
    d2d.rect(px - 1.0, py, px, py + ph, BORDER);              // left
    d2d.rect(px + pw, py, px + pw + 1.0, py + ph, BORDER);    // right

    // Title bar
    let title_y = py + 4.0;
    d2d.rect(px, py, px + pw, py + TITLE_H, BG_LIGHT);
    d2d.text("VLSI Workbench", px + 8.0, title_y, TEXT_BRIGHT, true);

    // Close button (top-right)
    let close_x = px + pw - 16.0;
    d2d.rect(close_x - 2.0, py + 2.0, close_x + 10.0, py + TITLE_H - 2.0, 0xFF_553333);
    d2d.text("X", close_x, title_y, 0xFF_FF4444, true);

    // ── Left panel: Design library ──────────────────────────────────────────
    let left_x = px + PAD;
    let left_w = pw * 0.45;
    let list_y = py + TITLE_H + 4.0;
    let list_h = ph - TITLE_H - 50.0;

    d2d.text("Designs:", left_x, list_y - LINE_H, ACCENT, true);
    d2d.rect(left_x, list_y, left_x + left_w, list_y + list_h, SLOT_BG);

    // Scrollable design rows
    let content_h = designs.len() as f32 * (ROW_H + 2.0);
    let max_scroll = (content_h - list_h).max(0.0);
    {
        let mut s = SCROLL.lock().unwrap();
        *s = s.clamp(0.0, max_scroll);
    }
    let scroll = *SCROLL.lock().unwrap();

    let mut rows = Vec::new();
    let mut row_y = list_y + 2.0 - scroll;
    for (i, d) in designs.iter().enumerate() {
        let ry = row_y;
        row_y += ROW_H + 2.0;
        rows.push((i, ry, ry + ROW_H));

        if ry + ROW_H < list_y || ry > list_y + list_h { continue; }

        let is_sel = selected.as_deref() == Some(&d.name);
        let row_bg = if is_sel { SEL_BG } else { BG_LIGHT };
        d2d.rect(left_x + 1.0, ry.max(list_y), left_x + left_w - 1.0, (ry + ROW_H).min(list_y + list_h), row_bg);

        if ry + 8.0 >= list_y && ry + 8.0 <= list_y + list_h {
            let label = format!("{} [{}]", d.name, d.tier.name());
            let lbl = if label.len() as f32 * CHAR_W > left_w - 10.0 {
                format!("{}...", &label[..((left_w - 20.0) / CHAR_W) as usize])
            } else { label };
            d2d.text(&lbl, left_x + 4.0, ry + 4.0, if is_sel { TEXT_BRIGHT } else { TEXT }, false);
        }
    }
    *ROWS.lock().unwrap() = rows;

    // ── Right panel: Resources ──────────────────────────────────────────────
    let right_x = px + pw * 0.5;
    let right_w = pw * 0.5 - PAD;

    d2d.text("Resources:", right_x, list_y - LINE_H, ACCENT, true);
    d2d.rect(right_x, list_y, right_x + right_w, list_y + list_h, SLOT_BG);

    let res = RESOURCES.lock().unwrap();
    // Find resources for nearby workbench (simplified: show all)
    let all_res: Vec<(String, u64)> = res.values()
        .flat_map(|m| m.iter())
        .fold(HashMap::new(), |mut acc, (k, v)| {
            *acc.entry(k.clone()).or_default() += *v;
            acc
        })
        .into_iter()
        .collect();

    let mut res_y = list_y + 4.0;
    for (item, qty) in all_res.iter().take(12) {
        if res_y + LINE_H > list_y + list_h { break; }
        d2d.text(&format!("{}: {}", item, qty), right_x + 4.0, res_y, TEXT_DIM, false);
        res_y += LINE_H;
    }
    drop(res);

    if all_res.is_empty() {
        d2d.text("(empty — right-click workbench", right_x + 4.0, list_y + 4.0, TEXT_DIM, false);
        d2d.text(" with items to add resources)", right_x + 4.0, list_y + 16.0, TEXT_DIM, false);
    }

    // ── Selected design info ────────────────────────────────────────────────
    if let Some(ref sel_name) = selected {
        if let Some(entry) = load_design(&game_dir, &player_name,
            &designs.iter().find(|d| d.name == *sel_name).map(|d| d.id.clone()).unwrap_or_default())
        {
            let info_y = list_y + list_h + 4.0;
            d2d.text(&format!("Selected: {} ({} ports, {} blocks)",
                sel_name, entry.circuit.ports.len(), entry.circuit.blocks.len()),
                left_x, info_y, TEXT_BRIGHT, false);
        }
    }

    // ── Buttons ─────────────────────────────────────────────────────────────
    let btn_y = py + ph - BTN_H - 8.0;
    let mut buttons = Vec::new();
    let btn_labels = ["Design", "Fabricate", "Export BP", "Import BP"];
    let mut bx = left_x;
    for label in &btn_labels {
        buttons.push((*label, bx, btn_y, bx + BTN_W, btn_y + BTN_H));
        d2d.rect(bx, btn_y, bx + BTN_W, btn_y + BTN_H, BTN_BG);
        d2d.rect(bx, btn_y, bx + BTN_W, btn_y + 1.0, BORDER);
        d2d.rect(bx, btn_y + BTN_H - 1.0, bx + BTN_W, btn_y + BTN_H, BORDER);
        let tx = bx + (BTN_W - label.len() as f32 * CHAR_W) / 2.0;
        d2d.text(label, tx, btn_y + 4.0, TEXT_BRIGHT, false);
        bx += BTN_W + 6.0;
    }
    *BUTTONS.lock().unwrap() = buttons;

    // Close button rect
    BUTTONS.lock().unwrap().push(("close", close_x - 2.0, py + 2.0, close_x + 10.0, py + TITLE_H - 2.0));
}

/// Handle click events forwarded from the Java side.
pub fn handle_click(ui_id: &str, event: &str) {
    if event == "close" {
        clear();
        return;
    }

    let (game_dir, player_name) = PLAYER.lock().unwrap().clone();
    if player_name.is_empty() { return; }

    // Parse "click:X:Y"
    if let Some(rest) = event.strip_prefix("click:") {
        let mut parts = rest.splitn(2, ':');
        let mx: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let my: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);

        // Check buttons first
        let buttons = BUTTONS.lock().unwrap();
        let mut hit = None;
        for (name, bx0, by0, bx1, by1) in buttons.iter() {
            if mx >= *bx0 && mx <= *bx1 && my >= *by0 && my <= *by1 {
                hit = Some(*name);
                break;
            }
        }
        drop(buttons);
        if let Some(name) = hit {
            let selected = SELECTED.lock().unwrap().clone();
            let (px, py, pz) = *LAST_PLAYER_POS.lock().unwrap();
            match name {
                "close" => clear(),
                "Design" => match selected {
                    Some(sel) => network::send_workbench_action(&["edit", &sel, &px.to_string(), &py.to_string(), &pz.to_string()]),
                    None => hint("§eSelect a design first, or run /vlsi design <name> <tier>."),
                },
                "Fabricate" => {
                    if let Some(sel) = selected {
                        let (game_dir, player_name) = PLAYER.lock().unwrap().clone();
                        if let Some(meta) = list_designs(&game_dir, &player_name).into_iter().find(|d| d.name == sel) {
                            network::send_workbench_action(&["fabricate", &sel, meta.tier.id()]);
                        }
                    }
                }
                "Export BP" => {
                    if let Some(sel) = selected {
                        network::send_workbench_action(&["export_bp", &sel]);
                    }
                }
                "Import BP" => hint("§eHold a filled Blueprint and right-click any VLSI Workbench to import it."),
                _ => {}
            }
            return;
        }

        // Check design list rows
        let rows = ROWS.lock().unwrap();
        if let Some(&(idx, _, _)) = rows.iter().find(|&&(_, ry0, ry1)| my >= ry0 && my <= ry1) {
            let designs = list_designs(&game_dir, &player_name);
            if let Some(d) = designs.get(idx) {
                let mut sel = SELECTED.lock().unwrap();
                if sel.as_deref() == Some(&d.name) {
                    *sel = None; // deselect
                } else {
                    *sel = Some(d.name.clone());
                }
            }
        }
    }

    // Scroll events
    if let Some(rest) = event.strip_prefix("scroll:") {
        let dy: f32 = rest.parse().unwrap_or(0.0);
        let mut s = SCROLL.lock().unwrap();
        *s = (*s - dy * 20.0).max(0.0);
    }
}
