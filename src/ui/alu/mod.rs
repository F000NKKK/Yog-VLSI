//! The ALU's screen (`alu.yui`, this file is its code-behind): its chip
//! slots, the player's inventory, and the node graph (DESIGN.md §4.6) —
//! a `<Surface>` drawn and driven here. What the graph means is in
//! `board.rs`; the server owns it, the screen draws the [`BoardView`] it's
//! sent and asks for changes.
//!
//! Using the graph:
//! - drag from an output pin (a dot on a node's right edge) to an input pin
//!   (a dot on a left edge) to wire them — or the other way round;
//! - drag a wire off the input pin it ends at to move it to another pin,
//!   or into empty space to remove it;
//! - drag a chip by its title to move it;
//! - drag empty space to move the view, turn the wheel to zoom it; the In
//!   and Out panels move by their titles too, as wide as their names;
//! - a bus (a side a digital cable reaches) shows its channels: wire into a
//!   channel's left pin to drive it, out of its right pin to read it; pull
//!   the bar at its bottom down for more channels.

yog_api::ui_include!("ui/alu");

mod camera;
mod draw;
mod flow;
mod input;
mod layout;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use yog_api::sync::{Arc, LazyLock, Mutex, Once};

use yog_api::ui::{Pointer, Rect, Surface};
use yog_api::{GfxContext, Registry};
use yog_kit::prelude::*;

use crate::board::{self, BoardView, Node, Pin, Request};
use crate::res::{Colors, Strings};
use crate::vm::MAX_POWER;
use camera::Camera;
use layout::Layout;

// ── The screen's view model ──────────────────────────────────────────────────

#[derive(Default)]
pub struct AluVm {
    view: Option<BoardView>,
}

impl ViewModel for AluVm {}

impl AluVm {
    fn apply(&mut self, view: BoardView) {
        self.view = Some(view);
    }
    pub fn title(&self) -> String {
        let Some(v) = &self.view else {
            return String::new();
        };
        let working = v.chips.iter().filter(|c| c.active).count();
        Strings::screen_alu_title(v.tier.name(), working, v.tier.max_chips())
    }
    pub fn slots_text(&self) -> String {
        let max = self.view.as_ref().map_or(0, |v| v.tier.max_chips());
        Strings::screen_alu_slots(max)
    }
    pub fn has_wires(&self) -> bool {
        self.view.as_ref().is_some_and(|v| !v.wires.is_empty())
    }
    pub fn clear(&mut self) {
        if let Some(v) = &self.view {
            board::send(&Request::Clear { alu: v.alu.clone() });
        }
        if let Some(v) = GRAPH.lock().view.as_mut() {
            v.wires.clear();
        }
    }
}

// ── The graph ────────────────────────────────────────────────────────────────

const SURFACE_ID: &str = "yog-vlsi:alu_graph";
const ROW: f32 = 10.0;
const HEAD: f32 = 12.0;
/// The margin round the graph's own edge.
const EDGE: f32 = 2.0;
/// The narrowest an In/Out panel gets (it widens to its ports' names), and
/// the room beside its widest name.
const SIDE_MIN_W: f32 = 40.0;
const PANEL_PAD: f32 = 8.0;
const CHIP_W: f32 = 100.0;
const GAP: f32 = 10.0;
/// The widest a column of chips is pitched: the chip and this many gaps.
const MAX_PITCH_GAPS: f32 = 4.0;
/// Room between the In/Out panels and the chips: the wires' curves.
const SIDE_GAP: f32 = 24.0;
/// A bus node's width and the height of the bar that pulls it open.
const BUS_W: f32 = 64.0;
const GRIP: f32 = 4.0;
/// How near a pin the pointer has to be to take it.
const REACH: f32 = 6.0;
/// How far in from a node's edge its labels and title start.
const LABEL_INSET: f32 = 3.0;
const CHIP_LABEL_INSET: f32 = 4.0;
/// What a title and a chip's pin name give up of their room (the insets
/// both sides).
const TITLE_CUT: f32 = 6.0;
const PIN_NAME_CUT: f32 = 5.0;
/// The game font's line height at scale 1.
const TEXT_H: f32 = 8.0;
/// The grid's spacing, in the graph's own units, and the closest it's drawn.
const GRID_STEP: f32 = 10.0;
const MIN_GRID_PX: f32 = 6.0;
/// What a turn of the wheel zooms by.
const ZOOM_STEP: f32 = 1.15;
/// Where the pointer is while it's off the graph.
const OFF_GRAPH: (f32, f32) = (-100.0, -100.0);

const BG: u32 = Colors::ALU_BACKGROUND;
const GRID: u32 = Colors::ALU_GRID;
const NODE: u32 = Colors::ALU_NODE;
const BORDER: u32 = Colors::ALU_BORDER;
const SHADOW: u32 = Colors::ALU_SHADOW;
/// Title bars: a chip, a sleeping chip, the In and Out panels, a bus.
const HEAD_CHIP: u32 = Colors::ALU_HEAD_CHIP;
const HEAD_SLEEPING: u32 = Colors::ALU_HEAD_SLEEPING;
const HEAD_IN: u32 = Colors::ALU_HEAD_IN;
const HEAD_OUT: u32 = Colors::ALU_HEAD_OUT;
const HEAD_BUS: u32 = Colors::ALU_HEAD_BUS;
const TEXT: u32 = Colors::ALU_TEXT;
const TEXT_DIM: u32 = Colors::ALU_TEXT_DIM;
const PIN_IN: u32 = Colors::PIN_INPUT;
const PIN_OUT: u32 = Colors::PIN_OUTPUT;
const PIN_RIM: u32 = Colors::ALU_PIN_RIM;
const WIRE_OFF: u32 = Colors::ALU_WIRE_OFF;
const WIRE_ON: u32 = Colors::ALU_WIRE_ON;
const WIRE_GLOW: u32 = Colors::ALU_WIRE_GLOW;
const WIRE_NEW: u32 = Colors::ALU_WIRE_NEW;
/// How bright the weakest signal's wire is, between off and on.
const WIRE_MIN_BRIGHT: f32 = 0.4;
/// A title bar's bottom edge: its color times this.
const SHADE: (u32, u32) = (3, 4);

/// What the player changed here that the server hasn't echoed back yet:
/// nodes moved, buses pulled open or closed.
#[derive(Default)]
struct Edits {
    moved: HashMap<Node, (f32, f32)>,
    rows: HashMap<u8, u16>,
}

enum Drag {
    None,
    /// A wire from a source, following the pointer; `lifted` is the sink
    /// it was taken off.
    FromSource {
        from: Pin,
        lifted: Option<Pin>,
    },
    /// A wire from a sink, looking for its source.
    FromSink {
        to: Pin,
    },
    /// A node (a chip, the In or Out panel) being moved; the pointer's
    /// offset in its title bar.
    Node {
        node: Node,
        dx: f32,
        dy: f32,
    },
    /// The view being moved: where the pointer and the view were at the start.
    Pan {
        from: (f32, f32),
        pan: (f32, f32),
    },
    /// A bus node being pulled open or closed: its side, its top (local).
    Rows {
        side: u8,
        top: f32,
    },
}

struct Graph {
    view: Option<BoardView>,
    layout: Layout,
    drag: Drag,
    pointer: (f32, f32),
    edits: Edits,
    camera: Camera,
    /// The camera fits this board already (it's the player's from then on).
    fitted: bool,
}

impl Graph {
    fn new(view: Option<BoardView>) -> Self {
        Graph {
            view,
            layout: Layout::default(),
            drag: Drag::None,
            pointer: (0.0, 0.0),
            edits: Edits::default(),
            camera: Camera::default(),
            fitted: false,
        }
    }

    /// The server's view arrived: what it has of the edits sent is dropped.
    fn take(&mut self, view: BoardView) {
        let e = &mut self.edits;
        e.moved
            .retain(|node, p| !view.layout.iter().any(|(n, q)| n == node && q == p));
        e.rows
            .retain(|side, r| !view.buses.iter().any(|b| b.index == *side && b.rows == *r));
        if self.view.as_ref().is_none_or(|v| v.alu != view.alu) {
            e.moved.clear();
            self.fitted = false;
        }
        self.view = Some(view);
    }
}

static GRAPH: LazyLock<Mutex<Graph>> = LazyLock::new(|| Mutex::new(Graph::new(None)));

/// A wire's color carrying `level`: off, else brighter the stronger.
fn level_color(level: u8) -> u32 {
    if level == 0 {
        return WIRE_OFF;
    }
    let t = f32::from(level.min(MAX_POWER)) / f32::from(MAX_POWER);
    let k = WIRE_MIN_BRIGHT + (1.0 - WIRE_MIN_BRIGHT) * t;
    let (off, on) = (WIRE_OFF.to_be_bytes(), WIRE_ON.to_be_bytes());
    let mix = |i: usize| (f32::from(off[i]) + (f32::from(on[i]) - f32::from(off[i])) * k) as u8;
    u32::from_be_bytes([u8::MAX, mix(1), mix(2), mix(3)])
}

/// A color a little darker.
fn shade(c: u32) -> u32 {
    let [a, r, g, b] = c.to_be_bytes();
    let ch = |v: u8| (u32::from(v) * SHADE.0 / SHADE.1) as u8;
    u32::from_be_bytes([a, ch(r), ch(g), ch(b)])
}

struct GraphSurface;

impl Surface for GraphSurface {
    fn paint(&self, gfx: &GfxContext, rect: Rect) {
        let mut g = GRAPH.lock();
        draw::draw(&gfx.draw2d(), &mut g, rect);
    }

    /// The wheel zooms the view about the pointer.
    fn scroll(&self, dy: f32, _size: (f32, f32)) -> bool {
        let mut g = GRAPH.lock();
        let factor = if dy > 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
        g.camera = g.camera.zoomed(factor, g.pointer);
        true
    }

    fn pointer(&self, ev: Pointer, _size: (f32, f32)) -> bool {
        input::pointer(&mut GRAPH.lock(), ev)
    }
}

static SURFACE: Once = Once::new();

/// The ALU blocks' screen (recipes: content/recipes/alu_*.yrecipe).
pub fn setup(registry: &mut Registry) {
    // The screen's markup lays out the block's inventory
    // (content/alu/*.yblock open it).
    let screen = register(registry);
    SURFACE.call_once(|| yog_api::ui::surface::register(SURFACE_ID, Arc::new(GraphSurface)));

    registry.on_client_packet(board::VIEW_CHANNEL, move |ev| {
        let Ok(view) = yog_api::yog_data::read_bytes::<BoardView>(&ev.payload) else {
            return;
        };
        GRAPH.lock().take(view.clone());
        screen.update(move |vm| vm.apply(view));
    });
}
