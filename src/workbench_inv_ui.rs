//! Workbench inventory UI — rendered as yog-ui overlay on the native inventory screen.
//!
//! Layout: left = chip slot + resource vials, right = design library + buttons.
//! Resource slots (1–6) show vial meters with item icons and fill indicators.
//! Slot 0 = chip, slots 1–6 = redstone/iron/gold/quartz/wood/cobblestone.

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

// ── Resource definitions ──────────────────────────────────────────────────────

/// (item_id, vial_color, capacity_per_slot)
const RESOURCES: &[(&str, u32, u32)] = &[
    ("minecraft:redstone",    0xFF_AA0000, 64),
    ("minecraft:iron_ingot",  0xFF_D8D8D8, 64),
    ("minecraft:gold_ingot",  0xFF_FFD700, 64),
    ("minecraft:quartz",      0xFF_FFFAFA, 64),
    ("minecraft:oak_log",     0xFF_8B6914, 64),
    ("minecraft:cobblestone", 0xFF_808080, 64),
];

// ── Layout constants ──────────────────────────────────────────────────────────

const PAD: f32 = 8.0;
const ROW_H: f32 = 18.0;
const BTN_H: f32 = 22.0;
const SLOT_SZ: f32 = 18.0;
const VIAL_W: f32 = 6.0;
const RIGHT_W: f32 = 210.0;
const RESOURCE_CAP: u32 = 64;

// ── Colors ────────────────────────────────────────────────────────────────────

const BG: u32       = 0xFF_1A1A1A;
const BG_LIGHT: u32 = 0xFF_252525;
const ACCENT: u32   = 0xFF_1E5A99;
const TEXT: u32     = 0xFF_CCCCCC;
const TEXT_DIM: u32 = 0xFF_777777;
const TEXT_BRIGHT: u32 = 0xFF_FFFFFF;
const BTN_BG: u32   = 0xFF_333333;
const SEL_BG: u32   = 0xFF_2A4A6E;
const SLOT_BG: u32  = 0xFF_0D0D0D;

// ── Public API ────────────────────────────────────────────────────────────────

pub fn render(gfx: &GfxContext) {
    let (sw_i, sh_i) = gfx.screen_size();
    let sw = sw_i as f32;
    let sh = sh_i as f32;

    let (game_dir, player_name) = PLAYER.lock().unwrap().clone();
    if player_name.is_empty() { return; }
    *LAST_PLAYER_POS.lock().unwrap() = (gfx.player_pos()[0], gfx.player_pos()[1], gfx.player_pos()[2]);

    let designs = list_designs(&game_dir, &player_name);
    let selected = SELECTED.lock().unwrap().clone();

    let slot_count = slot_cache::slot_count();
    let left_w = SLOT_SZ + VIAL_W + 30.0; // chip slot + vials + labels
    let total_w = left_w + RIGHT_W + PAD * 2.0;
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

    if let Some(rest) = event.strip_prefix("scroll:") {
        let dy: f32 = rest.parse().unwrap_or(0.0);
        let mut s = SCROLL.lock().unwrap();
        if dy > 0.0 { *s = s.saturating_sub(1); }
        else if dy < 0.0 { *s += 1; }
    }
}

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
    let btn_bar_h = BTN_H + 14.0;
    let body_h = total_h - 28.0 - btn_bar_h;

    let title = widget::panel(FlexDir::Row).dock(Dock::Top).h(28.0).bg(BG_LIGHT)
        .child(widget::label("VLSI Workbench").color(TEXT_BRIGHT).flex(1.0)
            .padding(6.0, 0.0, 0.0, 8.0).no_wrap());

    // Left: chip slot + resource vials
    let left_panel = build_resource_panel(slot_count);

    // Right: design list
    let right_panel = build_design_list(game_dir, player_name, designs, selected, body_h);

    let body = widget::panel(FlexDir::Row).dock(Dock::Top).h(body_h).gap(PAD)
        .padding(6.0, PAD, 0.0, PAD)
        .child(left_panel)
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

// ── Resource panel ────────────────────────────────────────────────────────────

fn build_resource_panel(slot_count: usize) -> widget::Widget {
    let mut col = widget::panel(FlexDir::Column).gap(4.0);

    // Chip slot (index 0)
    col = col.child(widget::label("Chip:").color(ACCENT).no_wrap());
    if slot_count > 0 {
        col = col.child(widget::inv_slot(0));
    }

    // Resource vials (slots 1–6)
    col = col.child(widget::label("Resources:").color(ACCENT).no_wrap());
    for i in 0..RESOURCES.len() {
        let slot_idx = 1 + i;
        let (item_id, color, cap) = RESOURCES[i];
        if slot_idx < slot_count {
            col = col.child(build_vial_row(slot_idx, item_id, color, cap));
        }
    }
    col
}

fn build_vial_row(slot_idx: usize, item_id: &str, color: u32, cap: u32) -> widget::Widget {
    // Query slot data from the pre-fetched cache
    let sd = slot_cache::get_slot(slot_idx);
    let count = sd.as_ref().map(|s| s.count).unwrap_or(0);
    let frac = (count as f32 / cap as f32).clamp(0.0, 1.0);

    // Item icon (from vanilla textures, like creative inventory)
    let icon = widget::item_slot(item_id).w(SLOT_SZ).h(SLOT_SZ);

    // Vial fill bar
    let fill_h = (SLOT_SZ - 2.0) * frac;
    let fill = widget::panel(FlexDir::Column)
        .dock(Dock::Bottom).h(fill_h.max(1.0)).bg(color);

    let vial = widget::panel(FlexDir::Column)
        .w(VIAL_W).h(SLOT_SZ).bg(SLOT_BG)
        .padding(1.0, 1.0, 1.0, 1.0)
        .child(fill);

    // Count label
    let count_label = if count > 0 {
        widget::label(format!("{}", count)).color(TEXT_BRIGHT).shadow(false).font_scale(0.7)
    } else {
        widget::label("0").color(TEXT_DIM).shadow(false).font_scale(0.7)
    };

    widget::panel(FlexDir::Row).gap(4.0).align(Align::Center)
        .child(icon)
        .child(vial)
        .child(count_label)
}

// ── Design list ───────────────────────────────────────────────────────────────

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
