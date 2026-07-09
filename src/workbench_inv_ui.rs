//! Workbench inventory UI — rendered as yog-ui overlay on the native inventory screen.
//!
//! Activated when a player opens the VLSI Workbench (right-click).
//! Shows slot widgets (from the block entity inventory) + design library +
//! fabricate/design buttons on the right side.

use std::sync::Mutex;

use yog_api::{widget, Align, Dock, FlexDir, GfxContext, Server, UiRoot};
use yog_api::ui::slot_cache;

use crate::designs::{list_designs, load_design, DesignMeta};
use crate::network;

// ── State ────────────────────────────────────────────────────────────────────

static SCROLL: Mutex<usize> = Mutex::new(0);
static SELECTED: Mutex<Option<String>> = Mutex::new(None);
static PLAYER: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));
static LAST_PLAYER_POS: Mutex<(f32, f32, f32)> = Mutex::new((0.0, 64.0, 0.0));
static LAST_UI: Mutex<Option<UiRoot>> = Mutex::new(None);

// ── Layout constants ──────────────────────────────────────────────────────────

const PAD: f32 = 8.0;
const ROW_H: f32 = 18.0;
const BTN_H: f32 = 22.0;
const SLOT_SZ: f32 = 18.0;
const RIGHT_W: f32 = 200.0;

// ── Colors ────────────────────────────────────────────────────────────────────

const BG: u32       = 0xFF_1A1A1A;
const BG_LIGHT: u32 = 0xFF_252525;
const ACCENT: u32   = 0xFF_1E5A99;
const TEXT: u32     = 0xFF_CCCCCC;
const TEXT_DIM: u32 = 0xFF_777777;
const TEXT_BRIGHT: u32 = 0xFF_FFFFFF;
const BTN_BG: u32   = 0xFF_333333;
const SEL_BG: u32   = 0xFF_2A4A6E;

// ── Public API ────────────────────────────────────────────────────────────────

/// Render callback for `registry.on_ui_render("yog:inv/yog-vlsi:workbench", ...)`.
pub fn render(gfx: &GfxContext) {
    let (sw_i, sh_i) = gfx.screen_size();
    let sw = sw_i as f32;
    let sh = sh_i as f32;

    let (game_dir, player_name) = PLAYER.lock().unwrap().clone();
    if player_name.is_empty() { return; }
    *LAST_PLAYER_POS.lock().unwrap() = (gfx.player_pos()[0], gfx.player_pos()[1], gfx.player_pos()[2]);

    let designs = list_designs(&game_dir, &player_name);
    let selected = SELECTED.lock().unwrap().clone();

    // Build the UI tree
    let slot_count = slot_cache::slot_count();
    let left_w = 8.0 + 9.0 * SLOT_SZ + 8.0; // 9-slot grid + padding
    let total_w = left_w + RIGHT_W + PAD;
    let total_h = sh * 0.85;

    let root = build_tree(&game_dir, &player_name, &designs, selected.as_deref(), total_w, total_h, slot_count);
    let screen_root = widget::panel(FlexDir::Row)
        .padding((sh - total_h) / 2.0, 0.0, 0.0, (sw - total_w) / 2.0)
        .child(root);

    let mut ui = UiRoot::new("yog-vlsi:workbench_inv", screen_root);
    ui.layout(sw, sh);
    ui.render(gfx);
    *LAST_UI.lock().unwrap() = Some(ui);
}

/// Click callback for `registry.register_ui("yog:inv/yog-vlsi:workbench", ...)`.
pub fn handle_click(_ui_id: &str, event: &str) {
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

        if let Some(idx) = action.strip_prefix("design_row:") {
            if let Ok(idx) = idx.parse::<usize>() {
                let designs = list_designs(&game_dir, &player_name);
                if let Some(d) = designs.get(idx) {
                    let mut sel = SELECTED.lock().unwrap();
                    *sel = if sel.as_deref() == Some(&d.name) { None } else { Some(d.name.clone()) };
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
                    None => hint("Select a design first."),
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
                _ => {}
            }
        }
    }

    // Scroll
    if let Some(rest) = event.strip_prefix("scroll:") {
        let dy: f32 = rest.parse().unwrap_or(0.0);
        let mut s = SCROLL.lock().unwrap();
        if dy > 0.0 { *s = s.saturating_sub(1); }
        else if dy < 0.0 { *s += 1; }
    }
}

/// Call before rendering — sets player context.
pub fn set_player_context(game_dir: &str, player_name: &str) {
    *PLAYER.lock().unwrap() = (game_dir.to_string(), player_name.to_string());
}

fn hint(msg: &str) {
    if let Some(srv) = yog_api::server() {
        let (_, player_name) = PLAYER.lock().unwrap().clone();
        srv.send_actionbar(&player_name, msg);
    }
}

// ── Tree builder ──────────────────────────────────────────────────────────────

fn build_tree(
    game_dir: &str, player_name: &str,
    designs: &[DesignMeta], selected: Option<&str>,
    total_w: f32, total_h: f32, slot_count: usize,
) -> widget::Widget {
    let btn_bar_h = BTN_H + 12.0;
    let body_h = total_h - 28.0 - btn_bar_h; // title + buttons

    let title = widget::panel(FlexDir::Row).dock(Dock::Top).h(28.0).bg(BG_LIGHT)
        .child(widget::label("VLSI Workbench").color(TEXT_BRIGHT).flex(1.0)
            .padding(6.0, 0.0, 0.0, 8.0).no_wrap());

    // Left: slot grid
    let slot_panel = build_slot_grid(slot_count);

    // Right: design list
    let right_panel = build_design_list(game_dir, player_name, designs, selected, body_h);

    let body = widget::panel(FlexDir::Row).dock(Dock::Top).h(body_h).gap(PAD)
        .padding(6.0, PAD, 0.0, PAD)
        .child(slot_panel)
        .child(right_panel);

    let btn_bar = widget::panel(FlexDir::Row).dock(Dock::Bottom).h(btn_bar_h).gap(6.0)
        .padding(4.0, PAD, 4.0, PAD).align(Align::Center)
        .child(bar_btn("Design"))
        .child(bar_btn("Fabricate"))
        .child(bar_btn("Export BP"));

    widget::panel(FlexDir::Column).w(total_w).h(total_h).bg(BG)
        .child(title).child(body).child(btn_bar)
}

fn bar_btn(label: &str) -> widget::Widget {
    widget::button(label).on_click(format!("btn:{label}"))
        .h(BTN_H).bg(BTN_BG).color(TEXT_BRIGHT)
        .padding(4.0, 8.0, 4.0, 8.0)
}

fn build_slot_grid(slot_count: usize) -> widget::Widget {
    let columns = 9usize;
    let mut grid = widget::panel(FlexDir::Column).gap(0.0);
    for row_start in (0..slot_count).step_by(columns) {
        let mut row = widget::panel(FlexDir::Row).gap(0.0);
        for col in 0..columns {
            let idx = row_start + col;
            if idx < slot_count {
                row = row.child(widget::inv_slot(idx));
            }
        }
        grid = grid.child(row);
    }
    widget::panel(FlexDir::Column)
        .child(widget::label("Slots:").color(ACCENT).no_wrap())
        .child(grid.padding(2.0, 0.0, 0.0, 0.0))
}

fn build_design_list(
    game_dir: &str, player_name: &str,
    designs: &[DesignMeta], selected: Option<&str>,
    body_h: f32,
) -> widget::Widget {
    let label_h = 16.0;
    let list_h = body_h - label_h;
    let rows_visible = ((list_h / (ROW_H + 2.0)).floor() as usize).max(1);
    let max_scroll = designs.len().saturating_sub(rows_visible);
    {
        let mut s = SCROLL.lock().unwrap();
        *s = (*s).min(max_scroll);
    }
    let scroll = *SCROLL.lock().unwrap();

    let info = selected_info(game_dir, player_name, designs, selected);

    let mut list = widget::panel(FlexDir::Column).gap(2.0).flex(1.0);
    if designs.is_empty() {
        list = list.child(widget::label("(no designs)").color(TEXT_DIM).shadow(false));
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

    let mut col = widget::panel(FlexDir::Column).w(RIGHT_W).flex(1.0).gap(2.0)
        .child(widget::label("Designs:").color(ACCENT).no_wrap())
        .child(list);
    if let Some(i) = info { col = col.child(i); }
    col
}

fn selected_info(
    game_dir: &str, player_name: &str,
    designs: &[DesignMeta], selected: Option<&str>,
) -> Option<widget::Widget> {
    let sel_name = selected?;
    let id = designs.iter().find(|d| d.name == sel_name)?.id.clone();
    let entry = load_design(game_dir, player_name, &id)?;
    Some(widget::label(format!("{} ports, {} blocks",
        entry.circuit.ports.len(), entry.circuit.blocks.len()))
        .color(TEXT_DIM).shadow(false).no_wrap().font_scale(0.7))
}
