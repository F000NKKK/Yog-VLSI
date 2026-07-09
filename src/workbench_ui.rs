//! Workbench GUI — Ender IO-style interface with design library, resources, fabrication.
//!
//! Built on `yog_ui`'s flexbox/dock layout engine (the same one `yog-book`
//! uses) instead of hand-rolled pixel math, so it reflows correctly at any
//! screen size / GUI scale instead of drifting.

use std::collections::HashMap;
use std::sync::Mutex;

use yog_api::{widget, Dock, FlexDir, GfxContext, Server, UiRoot};

use crate::designs::{list_designs, load_design, DesignMeta};
use crate::network;
use crate::workbench::RESOURCES;

/// Last known player position, refreshed every frame — used so button
/// actions that round-trip to the server (e.g. entering the circuit editor)
/// can tell it where to teleport the player back to afterwards.
static LAST_PLAYER_POS: Mutex<(f32, f32, f32)> = Mutex::new((0.0, 64.0, 0.0));

// ── State ────────────────────────────────────────────────────────────────────

/// Scroll offset for the design list, in whole rows.
static SCROLL: Mutex<usize> = Mutex::new(0);

/// Currently selected design name.
static SELECTED: Mutex<Option<String>> = Mutex::new(None);

/// Current player name and game dir for data access (set before each render).
static PLAYER: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));

/// True when the UI is active (tracked to allow refresh from commands).
pub static ACTIVE: Mutex<bool> = Mutex::new(false);

/// Layout tree from the last rendered frame, hit-tested on click.
static LAST_UI: Mutex<Option<UiRoot>> = Mutex::new(None);

// ── Constants ────────────────────────────────────────────────────────────────

const PAD: f32 = 8.0;
const TITLE_H: f32 = 24.0;
const ROW_H: f32 = 18.0;
const BTN_H: f32 = 20.0;
const BTN_BAR_H: f32 = BTN_H + 12.0;

// ── Colors (Ender IO palette) ────────────────────────────────────────────────

const BG: u32       = 0xFF_1A1A1A;
const BG_LIGHT: u32 = 0xFF_252525;
const ACCENT: u32   = 0xFF_1E5A99; // Ender IO blue
const TEXT: u32     = 0xFF_CCCCCC;
const TEXT_DIM: u32 = 0xFF_777777;
const TEXT_BRIGHT: u32 = 0xFF_FFFFFF;
const SLOT_BG: u32  = 0xFF_0D0D0D;
const BTN_BG: u32   = 0xFF_333333;
const SEL_BG: u32   = 0xFF_2A4A6E;

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
    let (sw_i, sh_i) = gfx.screen_size();
    let sw = sw_i as f32;
    let sh = sh_i as f32;

    let (game_dir, player_name) = PLAYER.lock().unwrap().clone();
    if player_name.is_empty() { return; }
    *LAST_PLAYER_POS.lock().unwrap() = (gfx.player_pos()[0], gfx.player_pos()[1], gfx.player_pos()[2]);

    let designs: Vec<DesignMeta> = list_designs(&game_dir, &player_name);
    let selected = SELECTED.lock().unwrap().clone();

    // Window: 80% of screen, centered.
    let pw = sw * 0.8;
    let ph = sh * 0.8;
    let px = (sw - pw) / 2.0;
    let py = (sh - ph) / 2.0;

    let root = build_tree(&game_dir, &player_name, &designs, selected.as_deref(), pw, ph);
    let screen_root = widget::panel(FlexDir::Row)
        .padding(py, 0.0, 0.0, px)
        .child(root);

    let mut ui = UiRoot::new("yog-vlsi:workbench", screen_root);
    ui.layout(sw, sh);
    ui.render(gfx);
    *LAST_UI.lock().unwrap() = Some(ui);
}

fn build_tree(game_dir: &str, player_name: &str, designs: &[DesignMeta], selected: Option<&str>, pw: f32, ph: f32) -> widget::Widget {
    const INFO_H: f32 = 14.0;
    let body_h = ph - TITLE_H - INFO_H - BTN_BAR_H;

    let title_bar = widget::panel(FlexDir::Row)
        .dock(Dock::Top).h(TITLE_H).bg(BG_LIGHT)
        .child(widget::label("VLSI Workbench").color(TEXT_BRIGHT).flex(1.0)
            .padding(7.0, 0.0, 0.0, 8.0).no_wrap());

    // Half of the body's inner width goes to each side (minus the gap between them).
    let side_w = ((pw - PAD * 2.0 - PAD) / 2.0).max(1.0);

    let body = widget::panel(FlexDir::Row)
        .dock(Dock::Top).h(body_h).gap(PAD)
        .padding(6.0, PAD, 0.0, PAD)
        .child(build_design_list(designs, selected, body_h))
        .child(build_resources_panel(side_w));

    let mut info_bar = widget::panel(FlexDir::Row)
        .dock(Dock::Top).h(INFO_H).padding(2.0, PAD, 0.0, PAD);
    if let Some(info) = selected_info(game_dir, player_name, designs, selected) {
        info_bar = info_bar.child(info);
    }

    let button_bar = widget::panel(FlexDir::Row)
        .dock(Dock::Bottom).h(BTN_BAR_H).gap(6.0)
        .padding(0.0, PAD, 0.0, PAD).align(yog_api::Align::Center)
        .child(bar_button("Design"))
        .child(bar_button("Fabricate"))
        .child(bar_button("Export BP"))
        .child(bar_button("Import BP"));

    widget::panel(FlexDir::Column)
        .w(pw).h(ph).bg(BG)
        .child(title_bar)
        .child(body)
        .child(info_bar)
        .child(button_bar)
}

fn bar_button(label: &str) -> widget::Widget {
    widget::button(label).on_click(format!("btn:{label}"))
        .h(BTN_H).bg(BTN_BG).color(TEXT_BRIGHT)
        .padding(4.0, 8.0, 4.0, 8.0)
}

fn build_design_list(designs: &[DesignMeta], selected: Option<&str>, body_h: f32) -> widget::Widget {
    let label_h = 16.0;
    let list_h = (body_h - label_h).max(0.0);
    let rows_visible = ((list_h / (ROW_H + 2.0)).floor() as usize).max(1);

    let max_scroll = designs.len().saturating_sub(rows_visible);
    {
        let mut s = SCROLL.lock().unwrap();
        *s = (*s).min(max_scroll);
    }
    let scroll = *SCROLL.lock().unwrap();

    // No explicit height here: `rows_visible` already limits how many rows are
    // added, so the panel naturally sizes to ~list_h. An explicit `.h()` cap
    // would clip real content below the boundary while still *drawing* it —
    // hit-testing bails out at the parent rect, making those rows unclickable.
    let mut list = widget::panel(FlexDir::Column).bg(SLOT_BG).gap(2.0).padding(2.0, 2.0, 2.0, 2.0);
    if designs.is_empty() {
        list = list.child(widget::label("(no designs — use /vlsi design)").color(TEXT_DIM).shadow(false));
    }
    for (i, d) in designs.iter().enumerate().skip(scroll).take(rows_visible) {
        let is_sel = selected == Some(d.name.as_str());
        list = list.child(
            widget::button(format!("{} [{}]", d.name, d.tier.name()))
                .on_click(format!("design_row:{i}"))
                .h(ROW_H)
                .bg(if is_sel { SEL_BG } else { BG_LIGHT })
                .color(if is_sel { TEXT_BRIGHT } else { TEXT })
                .shadow(false).no_wrap()
        );
    }

    widget::panel(FlexDir::Column).flex(1.0)
        .child(widget::label("Designs:").color(ACCENT).no_wrap())
        .child(list)
}

/// Info line describing the currently selected design, shown under the resources panel.
fn selected_info(game_dir: &str, player_name: &str, designs: &[DesignMeta], selected: Option<&str>) -> Option<widget::Widget> {
    let sel_name = selected?;
    let id = designs.iter().find(|d| d.name == sel_name)?.id.clone();
    let entry = load_design(game_dir, player_name, &id)?;
    Some(widget::label(format!("Selected: {} ({} ports, {} blocks)",
        sel_name, entry.circuit.ports.len(), entry.circuit.blocks.len()))
        .color(TEXT_BRIGHT).shadow(false).no_wrap())
}

/// Resources render as vertical "vial" meters (shell + color-filled bar,
/// filled proportionally to `qty / RESOURCE_CAP`) instead of a scrolling
/// text list, wrapping into as many columns as fit `avail_w`.
const RESOURCE_CAP: u64 = 1_000_000;
const VIAL_W: f32 = 22.0;
const VIAL_H: f32 = 48.0;

fn build_resources_panel(avail_w: f32) -> widget::Widget {
    let res = RESOURCES.lock().unwrap();
    let mut all_res: Vec<(String, u64)> = res.values()
        .flat_map(|m| m.iter())
        .fold(HashMap::new(), |mut acc, (k, v)| {
            *acc.entry(k.clone()).or_default() += *v;
            acc
        })
        .into_iter()
        .collect();
    drop(res);
    all_res.sort_by(|a, b| a.0.cmp(&b.0));

    let col = widget::panel(FlexDir::Column).flex(1.0)
        .child(widget::label("Resources:").color(ACCENT).no_wrap());

    if all_res.is_empty() {
        return col.child(widget::label("(empty — right-click workbench with items)")
            .color(TEXT_DIM).shadow(false));
    }

    let cols_per_row = (((avail_w + 8.0) / (VIAL_W + 8.0)).floor() as usize).max(1);
    let mut rows = widget::panel(FlexDir::Column).gap(8.0);
    for chunk in all_res.chunks(cols_per_row) {
        let mut row = widget::panel(FlexDir::Row).gap(8.0);
        for (item, qty) in chunk {
            row = row.child(build_vial(item, *qty));
        }
        rows = rows.child(row);
    }
    col.child(rows)
}

/// Deterministic pseudo-color derived from the item id, so each resource
/// keeps a stable, distinguishable fill color across sessions.
fn color_for(name: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in name.bytes() { h ^= b as u32; h = h.wrapping_mul(16777619); }
    let r = 90 + (h & 0x7F);
    let g = 90 + ((h >> 8) & 0x7F);
    let b = 90 + ((h >> 16) & 0x7F);
    0xFF_000000 | (r << 16) | (g << 8) | b
}

fn short_name(item: &str) -> &str {
    item.split(':').next_back().unwrap_or(item)
}

fn fmt_qty(qty: u64) -> String {
    if qty >= 1_000_000 { format!("{:.1}M", qty as f64 / 1_000_000.0) }
    else if qty >= 1_000 { format!("{:.1}k", qty as f64 / 1_000.0) }
    else { qty.to_string() }
}

fn build_vial(item: &str, qty: u64) -> widget::Widget {
    let frac = (qty as f32 / RESOURCE_CAP as f32).clamp(0.0, 1.0);
    let content_h = VIAL_H - 4.0;
    let fill = widget::panel(FlexDir::Column)
        .dock(Dock::Bottom).h(content_h * frac).bg(color_for(item));

    let shell = widget::panel(FlexDir::Column)
        .w(VIAL_W).h(VIAL_H).bg(SLOT_BG)
        .padding(2.0, 2.0, 2.0, 2.0)
        .child(fill);

    widget::panel(FlexDir::Column).w(VIAL_W).gap(1.0)
        .child(shell)
        .child(widget::label(short_name(item)).color(TEXT_DIM).shadow(false).no_wrap().font_scale(0.6))
        .child(widget::label(fmt_qty(qty)).color(TEXT_BRIGHT).shadow(false).no_wrap().font_scale(0.6))
}

/// Handle click events forwarded from the Java side.
pub fn handle_click(_ui_id: &str, event: &str) {
    if event == "close" {
        clear();
        return;
    }

    let (game_dir, player_name) = PLAYER.lock().unwrap().clone();
    if player_name.is_empty() { return; }

    if let Some(rest) = event.strip_prefix("click:") {
        let mut parts = rest.splitn(2, ':');
        let mx: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let my: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);

        let hit = LAST_UI.lock().unwrap().as_ref()
            .and_then(|ui| ui.hit_test(mx, my))
            .and_then(|n| n.on_click.clone());

        let Some(action) = hit else { return; };

        if action == "close" { clear(); return; }

        if let Some(idx) = action.strip_prefix("design_row:") {
            if let Ok(idx) = idx.parse::<usize>() {
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
            return;
        }

        if let Some(label) = action.strip_prefix("btn:") {
            let selected = SELECTED.lock().unwrap().clone();
            let (px, py, pz) = *LAST_PLAYER_POS.lock().unwrap();
            match label {
                "Design" => match selected {
                    Some(sel) => network::send_workbench_action(&["edit", &sel, &px.to_string(), &py.to_string(), &pz.to_string()]),
                    None => hint("§eSelect a design first, or run /vlsi design <name> <tier>."),
                },
                "Fabricate" => {
                    if let Some(sel) = selected {
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
        }
    }

    // Scroll events — one row per notch.
    if let Some(rest) = event.strip_prefix("scroll:") {
        let dy: f32 = rest.parse().unwrap_or(0.0);
        let mut s = SCROLL.lock().unwrap();
        if dy > 0.0 { *s = s.saturating_sub(1); }
        else if dy < 0.0 { *s += 1; }
    }
}
