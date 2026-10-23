//! Where every node and pin of the graph is drawn: where the player put
//! it, else the In panel on the left, the Out panel on the right, the
//! buses under the In panel and the chips in columns between.

use std::collections::HashMap;

use super::flow::{flow_depths, flow_layout};
use super::{
    BUS_W, CHIP_LABEL_INSET, CHIP_W, Camera, EDGE, Edits, GAP, GRIP, HEAD, LABEL_INSET,
    MAX_PITCH_GAPS, PANEL_PAD, PIN_NAME_CUT, REACH, ROW, SIDE_GAP, SIDE_MIN_W, TEXT, TEXT_DIM,
    TITLE_CUT,
};
use crate::board::{BoardView, ChipView, Node, Pin, SideView};
use crate::res::Strings;

/// A pin where it's drawn (local to the graph).
#[derive(Clone)]
pub(super) struct PinAt {
    pub(super) pin: Pin,
    pub(super) x: f32,
    pub(super) y: f32,
    /// How far up and down from `y` it takes the pointer (half a row, zoomed).
    pub(super) reach_y: f32,
    /// A source (a dot on a right edge); else a sink.
    pub(super) source: bool,
    pub(super) usable: bool,
}

/// A label on a node: its text, where (local) and its color.
pub(super) type Label = (String, f32, f32, u32);

pub(super) struct NodeAt {
    pub(super) node: Node,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) w: f32,
    pub(super) h: f32,
    /// Its title bar's height and its text's size: a chip's zoom, a side's 1.
    pub(super) head: f32,
    pub(super) scale: f32,
    pub(super) title: String,
    pub(super) awake: bool,
    pub(super) labels: Vec<Label>,
}

#[derive(Default)]
pub(super) struct Layout {
    pub(super) nodes: Vec<NodeAt>,
    pub(super) pins: Vec<PinAt>,
}

impl Layout {
    pub(super) fn pin(&self, pin: &Pin, source: bool) -> Option<&PinAt> {
        self.pins
            .iter()
            .find(|p| &p.pin == pin && p.source == source)
    }

    /// The pin nearest `(x, y)` within reach.
    pub(super) fn hit(&self, x: f32, y: f32) -> Option<&PinAt> {
        self.pins
            .iter()
            .filter(|p| (p.y - y).abs() <= p.reach_y && (p.x - x).abs() <= REACH * 2.0)
            .min_by(|a, b| (a.x - x).abs().total_cmp(&(b.x - x).abs()))
    }

    /// The node whose title bar is at `(x, y)` (the topmost).
    pub(super) fn header(&self, x: f32, y: f32) -> Option<&NodeAt> {
        self.nodes
            .iter()
            .rev()
            .find(|n| x >= n.x && x <= n.x + n.w && y >= n.y && y <= n.y + n.head)
    }
}

/// Lays the graph out `w` wide, every node where `cam` shows it, keeping
/// the player's `edits`; `width_of` measures text.
pub(super) fn layout(
    view: &BoardView,
    edits: &Edits,
    w: f32,
    cam: Camera,
    width_of: &dyn Fn(&str) -> f32,
) -> Layout {
    let mut l = Layouter {
        view,
        edits,
        cam,
        width_of,
        out: Layout::default(),
    };
    let (in_w, out_w) = (l.panel_w(Node::In), l.panel_w(Node::Out));
    let (pitch, room) = columns(view, w, in_w + out_w);
    l.panel(Node::In, EDGE, in_w);
    l.panel(Node::Out, room - out_w - EDGE, out_w);
    let bus_at = l.buses();
    let chips_x = in_w + EDGE + SIDE_GAP;
    let auto = flow_layout(view, &|n| l.placed(n), chips_x, pitch, &|pin| {
        l.source_y(pin, &bus_at)
    });
    for chip in &view.chips {
        let at = auto.get(&chip.slot).copied().unwrap_or((chips_x, EDGE));
        l.chip(chip, at);
    }
    l.out
}

/// The chips' columns' pitch — spread across the width `w`, closer when
/// it's narrow — and the width laid out in: `w` (at zoom 1: the default
/// places don't move as the view zooms), at least room for every column
/// between the panels (`panels` wide together).
fn columns(view: &BoardView, w: f32, panels: f32) -> (f32, f32) {
    let columns = flow_depths(view)
        .values()
        .copied()
        .max()
        .map_or(1, |d| d + 1) as f32;
    let around = panels + 2.0 * EDGE + 2.0 * SIDE_GAP;
    let pitch = if columns > 1.0 {
        ((w - around - CHIP_W) / (columns - 1.0)).clamp(CHIP_W + GAP, CHIP_W + MAX_PITCH_GAPS * GAP)
    } else {
        CHIP_W
    };
    (pitch, w.max(around + CHIP_W + (columns - 1.0) * pitch))
}

/// A side's row text: its name, and its port's.
fn side_text(side: &SideView) -> String {
    match &side.name {
        Some(name) => format!("{} {name}", side.side),
        None => side.side.clone(),
    }
}

/// The In or Out panel's title.
fn panel_title(node: Node) -> String {
    if node == Node::In {
        Strings::screen_alu_inputs()
    } else {
        Strings::screen_alu_outputs()
    }
}

/// A row's pin's height in the world: row `row` of a node whose top is `y`.
fn row_y(y: f32, row: usize) -> f32 {
    y + HEAD + row as f32 * ROW + ROW / 2.0
}

/// A chip's height: its title and its longer side of pins.
pub(super) fn chip_height(chip: &ChipView) -> f32 {
    let ins = chip.pins.iter().filter(|(_, m)| m.inputs()).count();
    let outs = chip.pins.iter().filter(|(_, m)| m.outputs()).count();
    HEAD + ins.max(outs).max(1) as f32 * ROW + 2.0
}

/// One layout being made: laid out in the world, drawn where the camera
/// shows it.
struct Layouter<'a> {
    view: &'a BoardView,
    edits: &'a Edits,
    cam: Camera,
    width_of: &'a dyn Fn(&str) -> f32,
    out: Layout,
}

impl Layouter<'_> {
    /// Where the player put `node`: moved here, else as the server has it.
    fn placed(&self, node: Node) -> Option<(f32, f32)> {
        self.edits.moved.get(&node).copied().or_else(|| {
            self.view
                .layout
                .iter()
                .find(|(n, _)| *n == node)
                .map(|(_, p)| *p)
        })
    }

    /// `s` cut to `room` wide, with an ellipsis if it had to be.
    fn fit(&self, s: &str, room: f32) -> String {
        if (self.width_of)(s) <= room {
            return s.to_string();
        }
        let mut t: String = s.to_string();
        while !t.is_empty() && (self.width_of)(&format!("{t}…")) > room {
            t.pop();
        }
        format!("{t}…")
    }

    /// A label at world `(x, y)`.
    fn label(&self, text: String, (x, y): (f32, f32), color: u32) -> Label {
        let (tx, ty) = self.cam.to_local((x, y));
        (text, tx, ty, color)
    }

    /// A pin at world `at`.
    fn pin(&mut self, pin: Pin, at: (f32, f32), source: bool, usable: bool) {
        let (x, y) = self.cam.to_local(at);
        self.out.pins.push(PinAt {
            pin,
            x,
            y,
            reach_y: ROW * self.cam.zoom / 2.0,
            source,
            usable,
        });
    }

    /// Node `n`, laid out in the world, as the camera shows it.
    fn place(&mut self, mut n: NodeAt) {
        let z = self.cam.zoom;
        (n.x, n.y) = self.cam.to_local((n.x, n.y));
        (n.w, n.h, n.head, n.scale) = (n.w * z, n.h * z, n.head * z, z);
        self.out.nodes.push(n);
    }

    /// The In or Out panel's width: its title's and its rows' widest.
    fn panel_w(&self, node: Node) -> f32 {
        self.view
            .sides
            .iter()
            .map(|s| (self.width_of)(&side_text(s)))
            .chain([(self.width_of)(&panel_title(node))])
            .fold(SIDE_MIN_W, |a, b| a.max(b + PANEL_PAD))
    }

    /// The ALU's sides: In (its sources) or Out (its sinks), `pw` wide, at
    /// `default_x` unless moved.
    fn panel(&mut self, node: Node, default_x: f32, pw: f32) {
        let (x, y) = self.placed(node).unwrap_or((default_x, EDGE));
        let source = node == Node::In;
        let mut labels = Vec::new();
        for (row, side) in self.view.sides.iter().enumerate() {
            let ry = y + HEAD + row as f32 * ROW;
            let usable = !source || !side.output;
            let text = side_text(side);
            let tx = if source {
                x + LABEL_INSET
            } else {
                x + pw - LABEL_INSET - (self.width_of)(&text)
            };
            labels.push(self.label(text, (tx, ry + 1.0), if usable { TEXT } else { TEXT_DIM }));
            let px = if source { x + pw } else { x };
            self.pin(
                Pin::new(node, &side.side),
                (px, row_y(y, row)),
                source,
                usable,
            );
        }
        self.place(NodeAt {
            node,
            x,
            y,
            w: pw,
            h: HEAD + self.view.sides.len() as f32 * ROW + 2.0,
            head: HEAD,
            scale: 1.0,
            title: panel_title(node),
            awake: true,
            labels,
        });
    }

    /// The buses, under the In panel unless moved. Where each is.
    fn buses(&mut self) -> HashMap<u8, (f32, f32)> {
        let mut bus_y = EDGE + HEAD + self.view.sides.len() as f32 * ROW + 2.0 + GAP;
        let mut bus_at = HashMap::new();
        for bus in &self.view.buses {
            let node = Node::Bus(bus.index);
            let n_rows = self
                .edits
                .rows
                .get(&bus.index)
                .copied()
                .unwrap_or(bus.rows)
                .clamp(1, bus.channels.max(1));
            let nh = HEAD + n_rows as f32 * ROW + 2.0 + GRIP;
            let (x, y) = self.placed(node).unwrap_or((EDGE, bus_y));
            bus_at.insert(bus.index, (x, y));
            bus_y += nh + GAP;
            let mut labels = Vec::new();
            for n in 1..=n_rows {
                let row = usize::from(n - 1);
                let name = n.to_string();
                let tx = x + (BUS_W - (self.width_of)(&name)) / 2.0;
                labels.push(self.label(
                    name.clone(),
                    (tx, y + HEAD + row as f32 * ROW + 1.0),
                    TEXT,
                ));
                for source in [false, true] {
                    let px = if source { x + BUS_W } else { x };
                    self.pin(
                        Pin::new(node, name.as_str()),
                        (px, row_y(y, row)),
                        source,
                        true,
                    );
                }
            }
            let title = self.fit(&Strings::screen_alu_bus(&bus.side), BUS_W - TITLE_CUT);
            self.place(NodeAt {
                node,
                x,
                y,
                w: BUS_W,
                h: nh,
                head: HEAD,
                scale: 1.0,
                title,
                awake: true,
                labels,
            });
        }
        bus_at
    }

    /// Where a source pin of the board's sides or buses is (world height).
    fn source_y(&self, pin: &Pin, bus_at: &HashMap<u8, (f32, f32)>) -> Option<f32> {
        match pin.node {
            Node::In => self
                .view
                .sides
                .iter()
                .position(|s| s.side == pin.name)
                .map(|r| row_y(self.placed(Node::In).unwrap_or((EDGE, EDGE)).1, r)),
            Node::Bus(i) => {
                let (_, y) = *bus_at.get(&i)?;
                let n: usize = pin.name.parse().ok()?;
                Some(row_y(y, n.checked_sub(1)?))
            }
            _ => None,
        }
    }

    /// A chip, where the player put it, else at `auto`.
    fn chip(&mut self, chip: &ChipView, auto: (f32, f32)) {
        let node = Node::Chip(chip.slot);
        let (x, y) = self.placed(node).unwrap_or(auto);
        let color = if chip.active { TEXT } else { TEXT_DIM };
        let mut labels = Vec::new();
        for source in [false, true] {
            let names = chip
                .pins
                .iter()
                .filter(|(_, m)| if source { m.outputs() } else { m.inputs() });
            for (row, (name, _)) in names.enumerate() {
                let text = self.fit(name, CHIP_W / 2.0 - PIN_NAME_CUT);
                let tx = if source {
                    x + CHIP_W - CHIP_LABEL_INSET - (self.width_of)(&text)
                } else {
                    x + CHIP_LABEL_INSET
                };
                labels.push(self.label(text, (tx, y + HEAD + row as f32 * ROW + 1.0), color));
                let px = if source { x + CHIP_W } else { x };
                self.pin(
                    Pin::new(node, name.as_str()),
                    (px, row_y(y, row)),
                    source,
                    chip.active,
                );
            }
        }
        let title = if chip.active {
            format!("{} {}", chip.slot + 1, chip.name)
        } else {
            let sleeping = Strings::screen_alu_sleeping();
            format!("{} {} ({sleeping})", chip.slot + 1, chip.name)
        };
        let title = self.fit(&title, CHIP_W - TITLE_CUT);
        self.place(NodeAt {
            node,
            x,
            y,
            w: CHIP_W,
            h: chip_height(chip),
            head: HEAD,
            scale: 1.0,
            title,
            awake: chip.active,
            labels,
        });
    }
}
