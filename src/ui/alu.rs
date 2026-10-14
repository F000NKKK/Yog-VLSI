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
//! - drag empty space to move the view, turn the wheel to zoom it (the
//!   ALU's sides stay at the edges).

yog_api::ui_include!("ui/alu");

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Once};

use yog_api::gfx_draw2d::Draw2D;
use yog_api::ui::{Pointer, Rect, Surface};
use yog_api::{GfxContext, Registry};
use yog_kit::prelude::*;

use crate::board::{self, BoardView, Node, Pin, Request, Wire};
use crate::res::Strings;
use crate::vm::PortMode;

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
        let Some(v) = &self.view else { return String::new() };
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
        if let Some(v) = GRAPH.lock().unwrap().view.as_mut() {
            v.wires.clear();
        }
    }
}

// ── The graph ────────────────────────────────────────────────────────────────

const SURFACE_ID: &str = "yog-vlsi:alu_graph";
const ROW: f32 = 10.0;
const HEAD: f32 = 12.0;
const SIDE_W: f32 = 62.0;
const CHIP_W: f32 = 100.0;
const GAP: f32 = 10.0;
/// How near a pin the pointer has to be to take it.
const REACH: f32 = 6.0;

const BG: u32 = 0xFF1A2B20;
const NODE: u32 = 0xEE2C2C2C;
const HEADER: u32 = 0xFF4A4A4A;
const HEADER_SLEEPING: u32 = 0xFF3A2626;
const TEXT: u32 = 0xFFFFFFFF;
const TEXT_DIM: u32 = 0xFF8A8A8A;
const PIN_IN: u32 = 0xFF4F8FFF;
const PIN_OUT: u32 = 0xFFFF8A3A;
const PIN_BIDI: u32 = 0xFFB070FF;
const WIRE_OFF: u32 = 0xFF5A1818;
const WIRE_ON: u32 = 0xFFFF3A3A;
const WIRE_NEW: u32 = 0xFFFFE070;

/// The view of the chips: their place `(x, y)` is drawn at
/// `pan + (x, y) · zoom` (local to the graph).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Camera {
    zoom: f32,
    pan: (f32, f32),
}

impl Default for Camera {
    fn default() -> Self {
        Camera { zoom: 1.0, pan: (0.0, 0.0) }
    }
}

impl Camera {
    const MIN: f32 = 0.4;
    const MAX: f32 = 3.0;

    fn to_local(self, (x, y): (f32, f32)) -> (f32, f32) {
        (self.pan.0 + x * self.zoom, self.pan.1 + y * self.zoom)
    }

    fn to_world(self, (x, y): (f32, f32)) -> (f32, f32) {
        ((x - self.pan.0) / self.zoom, (y - self.pan.1) / self.zoom)
    }

    /// Zoomed by `factor` keeping the point under `at` (local) where it is.
    fn zoomed(self, factor: f32, at: (f32, f32)) -> Camera {
        let zoom = (self.zoom * factor).clamp(Self::MIN, Self::MAX);
        let world = self.to_world(at);
        Camera { zoom, pan: (at.0 - world.0 * zoom, at.1 - world.1 * zoom) }
    }
}

/// A pin where it's drawn (local to the graph).
#[derive(Clone)]
struct PinAt {
    pin: Pin,
    x: f32,
    y: f32,
    /// How far up and down from `y` it takes the pointer (half a row, zoomed).
    reach_y: f32,
    /// A source (a dot on a right edge); else a sink.
    source: bool,
    usable: bool,
}

struct NodeAt {
    node: Node,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// Its title bar's height and its text's size: a chip's zoom, a side's 1.
    head: f32,
    scale: f32,
    title: String,
    awake: bool,
    /// (text, x, y, color) — pin labels.
    labels: Vec<(String, f32, f32, u32)>,
}

#[derive(Default)]
struct Layout {
    nodes: Vec<NodeAt>,
    pins: Vec<PinAt>,
}

impl Layout {
    fn pin(&self, pin: &Pin, source: bool) -> Option<&PinAt> {
        self.pins.iter().find(|p| &p.pin == pin && p.source == source)
    }

    /// The pin nearest `(x, y)` within reach.
    fn hit(&self, x: f32, y: f32) -> Option<&PinAt> {
        self.pins
            .iter()
            .filter(|p| (p.y - y).abs() <= p.reach_y && (p.x - x).abs() <= REACH * 2.0)
            .min_by(|a, b| (a.x - x).abs().total_cmp(&(b.x - x).abs()))
    }

    /// The chip whose title bar is at `(x, y)`.
    fn header(&self, x: f32, y: f32) -> Option<&NodeAt> {
        self.nodes.iter().rev().find(|n| {
            matches!(n.node, Node::Chip(_)) && x >= n.x && x <= n.x + n.w && y >= n.y && y <= n.y + n.head
        })
    }
}

enum Drag {
    None,
    /// A wire from a source, following the pointer; `lifted` is the sink
    /// it was taken off.
    FromSource { from: Pin, lifted: Option<Pin> },
    /// A wire from a sink, looking for its source.
    FromSink { to: Pin },
    /// A chip being moved; the pointer's offset in its title bar.
    Node { slot: u8, dx: f32, dy: f32 },
    /// The view being moved: where the pointer and the view were at the start.
    Pan { from: (f32, f32), pan: (f32, f32) },
}

struct Graph {
    view: Option<BoardView>,
    layout: Layout,
    drag: Drag,
    pointer: (f32, f32),
    /// Chip positions moved here but not yet echoed back by the server.
    moved: HashMap<u8, (f32, f32)>,
    camera: Camera,
}

static GRAPH: LazyLock<Mutex<Graph>> = LazyLock::new(|| {
    Mutex::new(Graph { view: None, layout: Layout::default(), drag: Drag::None, pointer: (0.0, 0.0), moved: HashMap::new(), camera: Camera::default() })
});

fn mode_color(mode: PortMode) -> u32 {
    match mode {
        PortMode::Input => PIN_IN,
        PortMode::Output => PIN_OUT,
        PortMode::Bidirectional => PIN_BIDI,
    }
}

fn level_color(level: u8) -> u32 {
    if level == 0 {
        return WIRE_OFF;
    }
    let t = level.min(15) as f32 / 15.0;
    let mix = |shift: u32| {
        let a = (WIRE_OFF >> shift) & 0xFF;
        let b = (WIRE_ON >> shift) & 0xFF;
        ((a as f32 + (b as f32 - a as f32) * (0.4 + 0.6 * t)) as u32) << shift
    };
    0xFF00_0000 | mix(16) | mix(8) | mix(0)
}

/// Lays the graph out in a `w`×`h` box — the ALU's sides at its edges, the
/// chips where `cam` shows them; `width_of` measures text.
fn layout(view: &BoardView, moved: &HashMap<u8, (f32, f32)>, w: f32, h: f32, cam: Camera, width_of: &dyn Fn(&str) -> f32) -> Layout {
    let _ = h;
    let mut out = Layout::default();
    let fit = |s: &str, room: f32| -> String {
        if width_of(s) <= room {
            return s.to_string();
        }
        let mut t: String = s.to_string();
        while !t.is_empty() && width_of(&format!("{t}…")) > room {
            t.pop();
        }
        format!("{t}…")
    };

    // The ALU's sides: In on the left, Out on the right.
    for (node, x) in [(Node::In, 2.0), (Node::Out, w - SIDE_W - 2.0)] {
        let mut labels = Vec::new();
        for (row, side) in view.sides.iter().enumerate() {
            let y = 2.0 + HEAD + row as f32 * ROW;
            let source = node == Node::In;
            let usable = !source || !side.output;
            let text = match &side.name {
                Some(name) => format!("{} {name}", side.side),
                None => side.side.clone(),
            };
            let text = fit(&text, SIDE_W - 8.0);
            let tx = if source { x + 3.0 } else { x + SIDE_W - 3.0 - width_of(&text) };
            labels.push((text, tx, y + 1.0, if usable { TEXT } else { TEXT_DIM }));
            out.pins.push(PinAt {
                pin: Pin::new(node, &side.side),
                x: if source { x + SIDE_W } else { x },
                y: y + ROW / 2.0,
                reach_y: ROW / 2.0,
                source,
                usable,
            });
        }
        let title = if node == Node::In { Strings::screen_alu_inputs() } else { Strings::screen_alu_outputs() };
        out.nodes.push(NodeAt { node, x, y: 2.0, w: SIDE_W, h: HEAD + 6.0 * ROW + 2.0, head: HEAD, scale: 1.0, title, awake: true, labels });
    }

    // The chips, in columns between them unless moved.
    let (x0, x1) = (SIDE_W + 2.0 + GAP, w - SIDE_W - 2.0 - GAP);
    let cols = (((x1 - x0 + GAP) / (CHIP_W + GAP)).floor() as usize).max(1);
    let mut col_y = vec![2.0f32; cols];
    for (i, chip) in view.chips.iter().enumerate() {
        let ins: Vec<_> = chip.pins.iter().filter(|(_, m)| m.inputs()).collect();
        let outs: Vec<_> = chip.pins.iter().filter(|(_, m)| m.outputs()).collect();
        let nh = HEAD + ins.len().max(outs.len()).max(1) as f32 * ROW + 2.0;
        let saved = view.layout.iter().find(|(s, _)| *s == chip.slot).map(|(_, p)| *p);
        let (x, y) = match moved.get(&chip.slot).copied().or(saved) {
            Some(p) => p,
            None => {
                let col = i % cols;
                let p = (x0 + col as f32 * (CHIP_W + GAP), col_y[col]);
                col_y[col] += nh + GAP;
                p
            }
        };
        // Laid out in the world, drawn where the camera shows it.
        let z = cam.zoom;
        let at = |wx: f32, wy: f32| cam.to_local((wx, wy));
        let mut labels = Vec::new();
        for (row, (name, _)) in ins.iter().enumerate() {
            let ry = y + HEAD + row as f32 * ROW;
            let text = fit(name, CHIP_W / 2.0 - 5.0);
            let (tx, ty) = at(x + 4.0, ry + 1.0);
            labels.push((text, tx, ty, if chip.active { TEXT } else { TEXT_DIM }));
            let (px, py) = at(x, ry + ROW / 2.0);
            out.pins.push(PinAt { pin: Pin::new(Node::Chip(chip.slot), name.as_str()), x: px, y: py, reach_y: ROW * z / 2.0, source: false, usable: chip.active });
        }
        for (row, (name, _)) in outs.iter().enumerate() {
            let ry = y + HEAD + row as f32 * ROW;
            let text = fit(name, CHIP_W / 2.0 - 5.0);
            let (tx, ty) = at(x + CHIP_W - 4.0 - width_of(&text), ry + 1.0);
            labels.push((text, tx, ty, if chip.active { TEXT } else { TEXT_DIM }));
            let (px, py) = at(x + CHIP_W, ry + ROW / 2.0);
            out.pins.push(PinAt {
                pin: Pin::new(Node::Chip(chip.slot), name.as_str()),
                x: px,
                y: py,
                reach_y: ROW * z / 2.0,
                source: true,
                usable: chip.active,
            });
        }
        let title = if chip.active {
            format!("{} {}", chip.slot + 1, chip.name)
        } else {
            format!("{} {} ({})", chip.slot + 1, chip.name, Strings::screen_alu_sleeping())
        };
        let (nx, ny) = at(x, y);
        out.nodes.push(NodeAt {
            node: Node::Chip(chip.slot),
            x: nx,
            y: ny,
            w: CHIP_W * z,
            h: nh * z,
            head: HEAD * z,
            scale: z,
            title: fit(&title, CHIP_W - 6.0),
            awake: chip.active,
            labels,
        });
    }
    out
}

/// The pin's mode color.
fn pin_color(view: &BoardView, p: &PinAt) -> u32 {
    if !p.usable {
        return TEXT_DIM;
    }
    match p.pin.node {
        Node::In => PIN_OUT,
        Node::Out => PIN_IN,
        Node::Chip(slot) => view
            .chips
            .iter()
            .find(|c| c.slot == slot)
            .and_then(|c| c.pins.iter().find(|(n, _)| *n == p.pin.name))
            .map_or(TEXT_DIM, |(_, m)| mode_color(*m)),
    }
}

/// A wire from `(x1, y1)` (a right edge) to `(x2, y2)` (a left edge), in
/// right-angled runs like a board's traces.
fn trace(d2: &Draw2D<'_>, (x1, y1): (f32, f32), (x2, y2): (f32, f32), color: u32) {
    let hline = |xa: f32, xb: f32, y: f32| d2.rect(xa.min(xb), y - 0.5, xa.max(xb) + 1.0, y + 0.5, color);
    let vline = |x: f32, ya: f32, yb: f32| d2.rect(x - 0.5, ya.min(yb), x + 0.5, ya.max(yb) + 1.0, color);
    if x2 >= x1 + 8.0 {
        let mx = ((x1 + x2) / 2.0).round();
        hline(x1, mx, y1);
        vline(mx, y1, y2);
        hline(mx, x2, y2);
    } else {
        // Backwards: out to the right, across, and in from the left.
        let (ra, lb) = (x1 + 5.0, x2 - 5.0);
        let my = ((y1 + y2) / 2.0).round();
        hline(x1, ra, y1);
        vline(ra, y1, my);
        hline(lb, ra, my);
        vline(lb, my, y2);
        hline(lb, x2, y2);
    }
}

struct GraphSurface;

impl Surface for GraphSurface {
    fn paint(&self, gfx: &GfxContext, rect: Rect) {
        let mut g = GRAPH.lock().unwrap();
        let d2 = gfx.draw2d();
        d2.rect(rect.x, rect.y, rect.x + rect.w, rect.y + rect.h, BG);
        let Some(view) = g.view.clone() else { return };
        let width_of = |t: &str| d2.text_width(t, "");
        g.layout = layout(&view, &g.moved, rect.w, rect.h, g.camera, &width_of);
        let (ox, oy) = (rect.x, rect.y);
        let at = |x: f32, y: f32| (ox + x, oy + y);
        let levels: HashMap<&Pin, u8> = view.levels.iter().map(|(p, l)| (p, *l)).collect();
        let lifted = match &g.drag {
            Drag::FromSource { lifted, .. } => lifted.clone(),
            _ => None,
        };
        // Nothing drawn past the graph's box, however the view is moved.
        gfx.set_scissor(rect.x.floor() as i32, rect.y.floor() as i32, rect.w.ceil() as i32, rect.h.ceil() as i32);

        let node = |n: &NodeAt| {
            let (x, y) = at(n.x, n.y);
            d2.rect(x, y, x + n.w, y + n.h, NODE);
            d2.rect(x, y, x + n.w, y + n.head - n.scale, if n.awake { HEADER } else { HEADER_SLEEPING });
            let color = if n.awake { TEXT } else { TEXT_DIM };
            d2.text_scaled(&n.title, "", x + 3.0 * n.scale, y + 2.0 * n.scale, n.scale, color, false);
            for (text, tx, ty, color) in &n.labels {
                d2.text_scaled(text, "", ox + tx, oy + ty, n.scale, *color, false);
            }
        };
        let pin = |p: &PinAt| {
            let (x, y) = at(p.x, p.y);
            d2.rect(x - 2.0, y - 2.0, x + 2.0, y + 2.0, pin_color(&view, p));
            if p.source
                && let Some(level) = levels.get(&p.pin).filter(|l| **l > 0)
            {
                d2.text(&level.to_string(), x + 3.0, y - 8.0, 0xFFFFD0D0, false);
            }
        };
        let is_side = |n: &Node| matches!(n, Node::In | Node::Out);

        // The chips, under everything.
        for n in g.layout.nodes.iter().filter(|n| !is_side(&n.node)) {
            node(n);
        }

        // Wires.
        for Wire { from, to } in &view.wires {
            if lifted.as_ref() == Some(to) {
                continue;
            }
            let (Some(a), Some(b)) = (g.layout.pin(from, true), g.layout.pin(to, false)) else { continue };
            let level = levels.get(from).copied().unwrap_or(0);
            trace(&d2, at(a.x, a.y), at(b.x, b.y), level_color(level));
        }
        for p in g.layout.pins.iter().filter(|p| !is_side(&p.pin.node)) {
            pin(p);
        }

        // The ALU's sides, on top: they stay at the edges.
        for n in g.layout.nodes.iter().filter(|n| is_side(&n.node)) {
            node(n);
        }
        for p in g.layout.pins.iter().filter(|p| is_side(&p.pin.node)) {
            pin(p);
        }

        // The wire being drawn.
        let (px, py) = g.pointer;
        match &g.drag {
            Drag::FromSource { from, .. } => {
                if let Some(a) = g.layout.pin(from, true) {
                    trace(&d2, at(a.x, a.y), at(px, py), WIRE_NEW);
                }
            }
            Drag::FromSink { to } => {
                if let Some(b) = g.layout.pin(to, false) {
                    trace(&d2, at(px, py), at(b.x, b.y), WIRE_NEW);
                }
            }
            _ => {}
        }
        if g.view.as_ref().is_some_and(|v| v.chips.is_empty()) {
            let hint = Strings::screen_alu_empty();
            let w = width_of(&hint);
            d2.text(&hint, ox + (rect.w - w) / 2.0, oy + rect.h / 2.0 - 4.0, TEXT_DIM, false);
        }
        gfx.clear_scissor();
    }

    /// The wheel zooms the view about the pointer.
    fn scroll(&self, dy: f32, _size: (f32, f32)) -> bool {
        let mut g = GRAPH.lock().unwrap();
        let factor = if dy > 0.0 { 1.15 } else { 1.0 / 1.15 };
        g.camera = g.camera.zoomed(factor, g.pointer);
        true
    }

    fn pointer(&self, ev: Pointer, _size: (f32, f32)) -> bool {
        let mut g = GRAPH.lock().unwrap();
        let Some(view) = g.view.clone() else { return false };
        match ev {
            Pointer::Down { x, y } => {
                g.pointer = (x, y);
                let pin = g.layout.hit(x, y).cloned();
                g.drag = match pin {
                    Some(p) if p.usable && p.source => Drag::FromSource { from: p.pin, lifted: None },
                    Some(p) if p.usable => match view.wires.iter().find(|w| w.to == p.pin) {
                        Some(w) => Drag::FromSource { from: w.from.clone(), lifted: Some(p.pin) },
                        None => Drag::FromSink { to: p.pin },
                    },
                    Some(_) => Drag::None,
                    None => match g.layout.header(x, y) {
                        Some(n) => match n.node {
                            Node::Chip(slot) => Drag::Node { slot, dx: x - n.x, dy: y - n.y },
                            _ => Drag::None,
                        },
                        // Empty space: the view moves.
                        None => Drag::Pan { from: (x, y), pan: g.camera.pan },
                    },
                };
                true
            }
            Pointer::Drag { x, y } | Pointer::Move { x, y } => {
                g.pointer = (x, y);
                match g.drag {
                    Drag::Node { slot, dx, dy } => {
                        let world = g.camera.to_world((x - dx, y - dy));
                        g.moved.insert(slot, world);
                    }
                    Drag::Pan { from, pan } => g.camera.pan = (pan.0 + x - from.0, pan.1 + y - from.1),
                    _ => {}
                }
                matches!(ev, Pointer::Drag { .. })
            }
            Pointer::Up { x, y } => {
                g.pointer = (x, y);
                let alu = view.alu.clone();
                let target = g.layout.hit(x, y).filter(|p| p.usable).cloned();
                let drag = std::mem::replace(&mut g.drag, Drag::None);
                let request = match drag {
                    Drag::FromSource { from, lifted } => match target.filter(|t| !t.source) {
                        Some(t) => Some(Request::Connect { alu, from, to: t.pin }),
                        None => lifted.map(|to| Request::Disconnect { alu, to }),
                    },
                    Drag::FromSink { to } => {
                        target.filter(|t| t.source).map(|t| Request::Connect { alu, from: t.pin, to })
                    }
                    Drag::Node { slot, .. } => {
                        g.moved.get(&slot).map(|&(x, y)| Request::Move { alu, slot, x: x.round(), y: y.round() })
                    }
                    Drag::Pan { .. } | Drag::None => None,
                };
                if let Some(request) = request {
                    // Show it now; the server's view follows.
                    if let Some(v) = g.view.as_mut() {
                        match &request {
                            Request::Connect { from, to, .. } => {
                                v.wires.retain(|w| &w.to != to);
                                v.wires.push(Wire { from: from.clone(), to: to.clone() });
                            }
                            Request::Disconnect { to, .. } => v.wires.retain(|w| &w.to != to),
                            _ => {}
                        }
                    }
                    board::send(&request);
                }
                true
            }
            Pointer::Leave => {
                if matches!(g.drag, Drag::None) {
                    g.pointer = (-100.0, -100.0);
                }
                false
            }
        }
    }
}

static SURFACE: Once = Once::new();

/// The ALU blocks' screen (recipes: content/recipes/alu_*.yrecipe).
pub fn setup(registry: &mut Registry) {
    // The screen's markup lays out the block's inventory
    // (content/alu/*.yblock open it).
    let screen = register(registry);
    SURFACE.call_once(|| yog_api::ui::surface::register(SURFACE_ID, Arc::new(GraphSurface)));

    registry.on_client_packet(board::VIEW_CHANNEL, move |ev, _srv| {
        let Ok(view) = serde_json::from_slice::<BoardView>(&ev.payload) else { return };
        {
            let mut g = GRAPH.lock().unwrap();
            // The server has the moves it was sent.
            g.moved.retain(|slot, p| !view.layout.iter().any(|(s, q)| s == slot && q == p));
            if g.view.as_ref().is_some_and(|v| v.alu != view.alu) {
                g.moved.clear();
            }
            g.view = Some(view.clone());
        }
        screen.update(move |vm| vm.apply(view));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{ChipView, SideView};
    use crate::tier::Tier;

    fn view() -> BoardView {
        BoardView {
            alu: board::AluId { dimension: String::new(), x: 0, y: 0, z: 0 },
            tier: Tier::ALL[0],
            chips: vec![ChipView {
                slot: 0,
                name: "Adder".into(),
                tier: Tier::ALL[0],
                pins: vec![("A".into(), PortMode::Input), ("B".into(), PortMode::Input), ("S".into(), PortMode::Output)],
                active: true,
            }],
            sides: board::SIDES.iter().map(|s| SideView { side: s.to_string(), name: None, output: *s == "E" }).collect(),
            wires: Vec::new(),
            layout: Vec::new(),
            levels: Vec::new(),
        }
    }

    #[test]
    fn pins_sit_on_their_edges() {
        let l = layout(&view(), &HashMap::new(), 360.0, 140.0, Camera::default(), &|t| t.len() as f32 * 6.0);
        let chip = l.nodes.iter().find(|n| n.node == Node::Chip(0)).unwrap();
        let a = l.pin(&Pin::new(Node::Chip(0), "A"), false).unwrap();
        let s = l.pin(&Pin::new(Node::Chip(0), "S"), true).unwrap();
        assert_eq!(a.x, chip.x);
        assert_eq!(s.x, chip.x + CHIP_W);
        assert!(l.pin(&Pin::new(Node::Chip(0), "A"), true).is_none(), "an input is no source");
        // In:E is an output side now: drawn, not usable.
        assert!(!l.pin(&Pin::new(Node::In, "E"), true).unwrap().usable);
        assert!(l.pin(&Pin::new(Node::Out, "E"), false).unwrap().usable);
        // Nothing overlaps the side columns.
        assert!(chip.x >= SIDE_W + 2.0 && chip.x + CHIP_W <= 360.0 - SIDE_W - 2.0);
    }

    #[test]
    fn slots_fit_the_screen() {
        let def = yog_kit::inventory_of::<super::View>().unwrap();
        assert_eq!(def.slot_count, board::CHIP_SLOTS as usize);
        assert!(def.include_player_inventory);
        let (px, py) = def.player_inv_offset;
        assert!(px + 9.0 * 18.0 <= WIDTH && py + 76.0 <= HEIGHT, "{px},{py}");
        assert!(def.layout.iter().all(|a| a.x + 16.0 <= px && a.y + 16.0 <= HEIGHT), "{:?}", def.layout);
        assert_eq!(def.layout[0].accepts.len(), 6);
    }

    #[test]
    fn the_wheel_zooms_about_the_pointer_and_chips_follow_the_view() {
        let cam = Camera::default().zoomed(2.0, (100.0, 50.0));
        assert_eq!(cam.to_local(cam.to_world((100.0, 50.0))), (100.0, 50.0), "the point under the pointer stays");
        assert_eq!(cam.zoom, 2.0);
        assert_eq!(Camera::default().zoomed(100.0, (0.0, 0.0)).zoom, Camera::MAX);
        let moved = Camera { zoom: 1.0, pan: (-500.0, 0.0) };
        let l = layout(&view(), &HashMap::new(), 360.0, 140.0, moved, &|t| t.len() as f32 * 6.0);
        let chip = l.nodes.iter().find(|n| n.node == Node::Chip(0)).unwrap();
        assert!(chip.x < 0.0, "panned out of the box, not held in it");
        let side = l.nodes.iter().find(|n| n.node == Node::In).unwrap();
        assert_eq!(side.x, 2.0, "the sides stay at the edges");
    }

    #[test]
    fn hits_the_nearest_pin() {
        let l = layout(&view(), &HashMap::new(), 360.0, 140.0, Camera::default(), &|t| t.len() as f32 * 6.0);
        let s = l.pin(&Pin::new(Node::Chip(0), "S"), true).unwrap().clone();
        assert_eq!(l.hit(s.x + 3.0, s.y - 2.0).map(|p| &p.pin), Some(&s.pin));
        assert!(l.hit(180.0, 139.0).is_none());
    }
}
