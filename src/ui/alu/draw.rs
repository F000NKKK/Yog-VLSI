//! Drawing the graph: the grid, the wires (glowing where they carry a
//! signal), the nodes and their pins, the wire being drawn.

use std::collections::{HashMap, HashSet};

use yog_api::gfx_draw2d::{Draw2D, TextStyle};
use yog_api::ui::{Color, Rect, Vec2};

use super::layout::{NodeAt, PinAt, layout};
use super::{
    BG, BORDER, Camera, Drag, EDGE, GRID, GRID_STEP, GRIP, Graph, HEAD_BUS, HEAD_CHIP, HEAD_IN,
    HEAD_OUT, HEAD_SLEEPING, LABEL_INSET, MIN_GRID_PX, NODE, PIN_IN, PIN_OUT, PIN_RIM, SHADOW,
    TEXT, TEXT_DIM, TEXT_H, WIRE_GLOW, WIRE_NEW, level_color, shade,
};
use crate::board::{BoardView, Node, Pin, Wire};
use crate::res::Strings;

/// A wire's width, and its glow's.
const WIRE_W: f32 = 1.5;
const GLOW_W: f32 = 3.0;
/// A pin's dot: its rim's radius, its fill's, a free pin's hole's.
const DOT_RIM: f32 = 2.5;
const DOT_FILL: f32 = 2.0;
const DOT_HOLE: f32 = 0.75;
/// How far a wire's curve pulls out of a pin: forward (a gentle S), and
/// backward (a loop out and round) — each the least and the most.
const PULL_FORWARD: (f32, f32) = (6.0, 60.0);
const PULL_BACK: (f32, f32) = (30.0, 80.0);
/// The middle weights of a cubic Bézier's Bernstein basis.
const CUBIC_MID: f32 = 3.0;
/// The fewest and most squares a wire is drawn with (about one a pixel).
const TRACE_STEPS: (f32, f32) = (8.0, 2000.0);
/// Where a bus's pull bar runs across it: its middle third.
const GRIP_BAR: (f32, f32) = (1.0 / 3.0, 2.0 / 3.0);
/// How far outside its box a text may stick before it's cut.
const SLACK: f32 = 0.5;

/// What the graph is drawn on: the game's 2D drawing, or (in tests) a
/// recording of it.
pub(super) trait Canvas {
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, color: u32);
    fn text(&self, text: &str, x: f32, y: f32, scale: f32, color: u32);
    fn text_width(&self, text: &str) -> f32;
}

impl Canvas for Draw2D<'_> {
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, color: u32) {
        Draw2D::rect(self, Rect::from_edges(x0, y0, x1, y1), Color(color));
    }
    fn text(&self, text: &str, x: f32, y: f32, scale: f32, color: u32) {
        let style = TextStyle {
            font: "",
            scale,
            color: Color(color),
            shadow: false,
        };
        self.text_scaled(text, Vec2::new(x, y), &style);
    }
    fn text_width(&self, text: &str) -> f32 {
        Draw2D::text_width(self, text, "")
    }
}

/// Drawing cut to the graph's box: rects are cut, text that doesn't fit
/// whole isn't drawn (as yog-ui clips; 2D drawing has no scissor).
struct Clip<'a> {
    d2: &'a dyn Canvas,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Clip<'_> {
    /// A point local to the graph, on the screen.
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x0 + x, self.y0 + y)
    }

    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, color: u32) {
        let (x0, y0, x1, y1) = (
            x0.max(self.x0),
            y0.max(self.y0),
            x1.min(self.x1),
            y1.min(self.y1),
        );
        if x1 > x0 && y1 > y0 {
            self.d2.rect(x0, y0, x1, y1, color);
        }
    }

    /// Text cut to the box: the characters that fit whole across it (a
    /// line half out keeps what shows); a line not all in it up and down
    /// isn't drawn.
    fn text(&self, text: &str, x: f32, y: f32, scale: f32, color: u32) {
        if y < self.y0 - SLACK || y + TEXT_H * scale > self.y1 + SLACK {
            return;
        }
        let width = |t: &str| self.d2.text_width(t) * scale;
        let (mut start, mut x) = (0, x);
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let byte = |k: usize| chars.get(k).map_or(text.len(), |(i, _)| *i);
        // Drop what's left of the box.
        while x < self.x0 - SLACK && start < chars.len() {
            let (i, c) = chars[start];
            x += width(&text[i..i + c.len_utf8()]);
            start += 1;
        }
        let from = byte(start);
        // And what's right of it.
        let mut end = chars.len();
        while end > start && x + width(&text[from..byte(end)]) > self.x1 + SLACK {
            end -= 1;
        }
        if end > start {
            self.d2.text(&text[from..byte(end)], x, y, scale, color);
        }
    }

    /// A box with its corners cut (rounded at this size).
    fn panel(&self, x0: f32, y0: f32, x1: f32, y1: f32, color: u32) {
        self.rect(x0 + 1.0, y0, x1 - 1.0, y1, color);
        self.rect(x0, y0 + 1.0, x0 + 1.0, y1 - 1.0, color);
        self.rect(x1 - 1.0, y0 + 1.0, x1, y1 - 1.0, color);
    }

    /// A round dot at `(x, y)`; `hollow`, a ring.
    fn dot(&self, x: f32, y: f32, color: u32, hollow: bool) {
        let (rim, fill) = ((DOT_RIM - 1.0, DOT_RIM), (DOT_FILL - 1.0, DOT_FILL));
        self.rect(x - rim.0, y - rim.1, x + rim.0, y + rim.1, PIN_RIM);
        self.rect(x - rim.1, y - rim.0, x + rim.1, y + rim.0, PIN_RIM);
        self.rect(x - fill.0, y - fill.1, x + fill.0, y + fill.1, color);
        self.rect(x - fill.1, y - fill.0, x + fill.1, y + fill.0, color);
        if hollow {
            self.rect(x - DOT_HOLE, y - DOT_HOLE, x + DOT_HOLE, y + DOT_HOLE, NODE);
        }
    }

    /// A wire from `(x1, y1)` (a source on a right edge) to `(x2, y2)` (a
    /// sink on a left edge): a smooth curve leaving and entering level —
    /// drawn as a run of small squares `width` across.
    fn trace(&self, (x1, y1): (f32, f32), (x2, y2): (f32, f32), color: u32, width: f32) {
        let pull = if x2 >= x1 {
            ((x2 - x1) / 2.0).clamp(PULL_FORWARD.0, PULL_FORWARD.1)
        } else {
            ((x1 - x2) / 2.0).clamp(PULL_BACK.0, PULL_BACK.1)
        };
        let (cx1, cx2) = (x1 + pull, x2 - pull);
        let at = |t: f32| {
            let u = 1.0 - t;
            let (a, b, c, d) = (
                u * u * u,
                CUBIC_MID * u * u * t,
                CUBIC_MID * u * t * t,
                t * t * t,
            );
            (
                a * x1 + b * cx1 + c * cx2 + d * x2,
                a * y1 + b * y1 + c * y2 + d * y2,
            )
        };
        // Roughly its length, a square every pixel.
        let len = pull * 2.0 + (x2 - x1).abs() + (y2 - y1).abs();
        let steps = len.clamp(TRACE_STEPS.0, TRACE_STEPS.1) as usize;
        let h = width / 2.0;
        for k in 0..=steps {
            let (x, y) = at(k as f32 / steps as f32);
            self.rect(x - h, y - h, x + h, y + h, color);
        }
    }
}

/// Draws the graph `g` into `rect`.
pub(super) fn draw(d2: &dyn Canvas, g: &mut Graph, rect: Rect) {
    d2.rect(rect.x, rect.y, rect.x + rect.w, rect.y + rect.h, BG);
    let Some(view) = g.view.clone() else { return };
    let width_of = |t: &str| d2.text_width(t);
    if !g.fitted {
        fit(g, &view, rect, &width_of);
    }
    g.layout = layout(&view, &g.edits, rect.w / g.camera.zoom, g.camera, &width_of);
    // Nothing drawn past the graph's box, however the view is moved.
    let c = Clip {
        d2,
        x0: rect.x,
        y0: rect.y,
        x1: rect.x + rect.w,
        y1: rect.y + rect.h,
    };
    grid(&c, g.camera);
    // Wires under the nodes (a wire behind a node doesn't cross its names).
    wires(&c, g, &view);
    for n in &g.layout.nodes {
        node(&c, n);
    }
    pins(&c, g, &view);
    new_wire(&c, g);
    if view.chips.is_empty() {
        let hint = Strings::screen_alu_empty();
        let (x, y) = c.at((rect.w - width_of(&hint)) / 2.0, (rect.h - TEXT_H) / 2.0);
        d2.text(&hint, x, y, 1.0, TEXT_DIM);
    }
}

/// First shown, the whole board in the box: zoomed out as far as needed.
fn fit(g: &mut Graph, view: &BoardView, rect: Rect, width_of: &dyn Fn(&str) -> f32) {
    let whole = layout(view, &g.edits, rect.w, Camera::default(), width_of);
    let right = whole.nodes.iter().map(|n| n.x + n.w).fold(rect.w, f32::max) + 2.0 * EDGE;
    let bottom = whole.nodes.iter().map(|n| n.y + n.h).fold(rect.h, f32::max) + 2.0 * EDGE;
    let zoom = (rect.w / right)
        .min(rect.h / bottom)
        .clamp(Camera::MIN, 1.0);
    g.camera = Camera {
        zoom,
        pan: (0.0, 0.0),
    };
    g.fitted = true;
}

/// A grid under it all, moving with the view.
fn grid(c: &Clip<'_>, cam: Camera) {
    let step = GRID_STEP * cam.zoom;
    if step < MIN_GRID_PX {
        return;
    }
    let mut x = c.x0 + cam.pan.0.rem_euclid(step);
    while x < c.x1 {
        let mut y = c.y0 + cam.pan.1.rem_euclid(step);
        while y < c.y1 {
            c.rect(x, y, x + 1.0, y + 1.0, GRID);
            y += step;
        }
        x += step;
    }
}

/// Every wire but the one lifted off its pin: a glow under those carrying
/// a signal.
fn wires(c: &Clip<'_>, g: &Graph, view: &BoardView) {
    let levels: HashMap<&Pin, u8> = view.levels.iter().map(|(p, l)| (p, *l)).collect();
    let lifted = match &g.drag {
        Drag::FromSource { lifted, .. } => lifted.as_ref(),
        _ => None,
    };
    for Wire { from, to } in &view.wires {
        if lifted == Some(to) {
            continue;
        }
        let (Some(a), Some(b)) = (g.layout.pin(from, true), g.layout.pin(to, false)) else {
            continue;
        };
        let (a, b) = (c.at(a.x, a.y), c.at(b.x, b.y));
        let level = levels.get(from).copied().unwrap_or(0);
        if level > 0 {
            c.trace(a, b, WIRE_GLOW, GLOW_W);
        }
        c.trace(a, b, level_color(level), WIRE_W);
    }
}

/// A node: its shadow, frame, title bar and labels; a bus's pull bar.
fn node(c: &Clip<'_>, n: &NodeAt) {
    let (x, y) = c.at(n.x, n.y);
    let s = n.scale;
    let shadow = 2.0 * s;
    c.panel(
        x + shadow,
        y + shadow,
        x + n.w + shadow,
        y + n.h + shadow,
        SHADOW,
    );
    c.panel(x, y, x + n.w, y + n.h, BORDER);
    c.rect(x + 1.0, y + 1.0, x + n.w - 1.0, y + n.h - 1.0, NODE);
    let head = match n.node {
        Node::In => HEAD_IN,
        Node::Out => HEAD_OUT,
        Node::Bus(_) => HEAD_BUS,
        Node::Chip(_) if n.awake => HEAD_CHIP,
        Node::Chip(_) => HEAD_SLEEPING,
    };
    c.rect(x + 1.0, y + 1.0, x + n.w - 1.0, y + n.head - s, head);
    c.rect(
        x + 1.0,
        y + n.head - s,
        x + n.w - 1.0,
        y + n.head,
        shade(head),
    );
    let color = if n.awake { TEXT } else { TEXT_DIM };
    c.text(&n.title, x + LABEL_INSET * s, y + 2.0 * s, s, color);
    for (text, tx, ty, color) in &n.labels {
        let (lx, ly) = c.at(*tx, *ty);
        c.text(text, lx, ly, s, *color);
    }
    if matches!(n.node, Node::Bus(_)) {
        let gy = y + n.h - GRIP * s;
        c.rect(
            x + n.w * GRIP_BAR.0,
            gy + s,
            x + n.w * GRIP_BAR.1,
            gy + 2.0 * s,
            HEAD_BUS,
        );
    }
}

/// Every pin: hollow while nothing is wired to it.
fn pins(c: &Clip<'_>, g: &Graph, view: &BoardView) {
    let wired: HashSet<(&Pin, bool)> = view
        .wires
        .iter()
        .flat_map(|w| [(&w.from, true), (&w.to, false)])
        .collect();
    for p in &g.layout.pins {
        let (x, y) = c.at(p.x, p.y);
        let hollow = p.usable && !wired.contains(&(&p.pin, p.source));
        c.dot(x, y, pin_color(view, p), hollow);
    }
}

/// The wire being drawn, from its pin to the pointer.
fn new_wire(c: &Clip<'_>, g: &Graph) {
    let (px, py) = g.pointer;
    match &g.drag {
        Drag::FromSource { from, .. } => {
            if let Some(a) = g.layout.pin(from, true) {
                c.trace(c.at(a.x, a.y), c.at(px, py), WIRE_NEW, WIRE_W);
            }
        }
        Drag::FromSink { to } => {
            if let Some(b) = g.layout.pin(to, false) {
                c.trace(c.at(px, py), c.at(b.x, b.y), WIRE_NEW, WIRE_W);
            }
        }
        _ => {}
    }
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
            .map_or(TEXT_DIM, |(_, m)| m.color()),
        // Left: driven onto the bus; right: read from it.
        Node::Bus(_) if p.source => PIN_OUT,
        Node::Bus(_) => PIN_IN,
    }
}
