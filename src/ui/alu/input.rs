//! The pointer on the graph: wiring pins, moving nodes and the view,
//! pulling buses open — each change asked of the server, and shown now.

use yog_api::ui::Pointer;

use super::layout::PinAt;
use super::{Drag, GRIP, Graph, HEAD, OFF_GRAPH, ROW};
use crate::board::{self, AluId, BoardView, Node, Request, Wire};

/// What the pointer did on the graph `g`; whether the graph took it.
pub(super) fn pointer(g: &mut Graph, ev: Pointer) -> bool {
    let Some(view) = g.view.clone() else {
        return false;
    };
    match ev {
        Pointer::Down { x, y } => {
            g.pointer = (x, y);
            g.drag = pressed(g, &view, x, y);
            true
        }
        Pointer::Drag { x, y } | Pointer::Move { x, y } => {
            g.pointer = (x, y);
            dragged(g, &view, x, y);
            matches!(ev, Pointer::Drag { .. })
        }
        Pointer::Up { x, y } => {
            g.pointer = (x, y);
            let target = g.layout.hit(x, y).filter(|p| p.usable).cloned();
            let drag = std::mem::replace(&mut g.drag, Drag::None);
            if let Some(request) = request(g, view.alu.clone(), drag, target) {
                show_now(g, &request);
                board::send(&request);
            }
            true
        }
        Pointer::Leave => {
            if matches!(g.drag, Drag::None) {
                g.pointer = OFF_GRAPH;
            }
            false
        }
    }
}

/// What pressing at `(x, y)` starts: pulling a bus's bar, a wire from a
/// pin (or one lifted off it), moving a node by its title, else the view.
fn pressed(g: &Graph, view: &BoardView, x: f32, y: f32) -> Drag {
    let grip = g.layout.nodes.iter().rev().find_map(|n| match n.node {
        Node::Bus(side)
            if x >= n.x
                && x <= n.x + n.w
                && y >= n.y + n.h - GRIP * n.scale - 1.0
                && y <= n.y + n.h + 1.0 =>
        {
            Some((side, n.y))
        }
        _ => None,
    });
    if let Some((side, top)) = grip {
        return Drag::Rows { side, top };
    }
    match g.layout.hit(x, y).cloned() {
        Some(p) if p.usable && p.source => Drag::FromSource {
            from: p.pin,
            lifted: None,
        },
        Some(p) if p.usable => match view.wires.iter().find(|w| w.to == p.pin) {
            Some(w) => Drag::FromSource {
                from: w.from.clone(),
                lifted: Some(p.pin),
            },
            None => Drag::FromSink { to: p.pin },
        },
        Some(_) => Drag::None,
        None => match g.layout.header(x, y) {
            Some(n) => Drag::Node {
                node: n.node,
                dx: x - n.x,
                dy: y - n.y,
            },
            // Empty space: the view moves.
            None => Drag::Pan {
                from: (x, y),
                pan: g.camera.pan,
            },
        },
    }
}

/// The pointer moved to `(x, y)` while dragging.
fn dragged(g: &mut Graph, view: &BoardView, x: f32, y: f32) {
    match g.drag {
        Drag::Node { node, dx, dy } => {
            let world = g.camera.to_world((x - dx, y - dy));
            g.edits.moved.insert(node, world);
        }
        Drag::Pan { from, pan } => g.camera.pan = (pan.0 + x - from.0, pan.1 + y - from.1),
        Drag::Rows { side, top } => {
            let z = g.camera.zoom;
            let channels = view
                .buses
                .iter()
                .find(|b| b.index == side)
                .map_or(1, |b| b.channels);
            let rows = ((y - top - (HEAD + 2.0 + GRIP) * z) / (ROW * z))
                .round()
                .clamp(1.0, f32::from(channels)) as u16;
            g.edits.rows.insert(side, rows);
        }
        _ => {}
    }
}

/// What `drag` ended on `target` asks of the server.
fn request(g: &Graph, alu: AluId, drag: Drag, target: Option<PinAt>) -> Option<Request> {
    match drag {
        Drag::FromSource { from, lifted } => match target.filter(|t| !t.source) {
            Some(t) => Some(Request::Connect {
                alu,
                from,
                to: t.pin,
            }),
            None => lifted.map(|to| Request::Disconnect { alu, to }),
        },
        Drag::FromSink { to } => target.filter(|t| t.source).map(|t| Request::Connect {
            alu,
            from: t.pin,
            to,
        }),
        Drag::Node { node, .. } => g.edits.moved.get(&node).map(|&(x, y)| Request::Move {
            alu,
            node,
            x: x.round(),
            y: y.round(),
        }),
        Drag::Rows { side, .. } => {
            g.edits
                .rows
                .get(&side)
                .map(|&rows| Request::BusRows { alu, side, rows })
        }
        Drag::Pan { .. } | Drag::None => None,
    }
}

/// A wire change shown now; the server's view follows.
fn show_now(g: &mut Graph, request: &Request) {
    let Some(v) = g.view.as_mut() else {
        return;
    };
    match request {
        Request::Connect { from, to, .. } => {
            v.wires.retain(|w| &w.to != to);
            v.wires.push(Wire {
                from: from.clone(),
                to: to.clone(),
            });
        }
        Request::Disconnect { to, .. } => v.wires.retain(|w| &w.to != to),
        _ => {}
    }
}
