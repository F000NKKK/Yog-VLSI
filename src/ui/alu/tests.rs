use super::draw::{Canvas, draw};
use super::layout::layout;
use super::*;
use crate::board::Wire;
use crate::board::{ChipView, SideView};
use crate::content::Tier;
use crate::vm::PortMode;

fn view() -> BoardView {
    BoardView {
        alu: board::AluId {
            dimension: String::new(),
            x: 0,
            y: 0,
            z: 0,
        },
        tier: Tier::ALL[0],
        chips: vec![ChipView {
            slot: 0,
            name: "Adder".into(),
            tier: Tier::ALL[0],
            pins: vec![
                ("A".into(), PortMode::Input),
                ("B".into(), PortMode::Input),
                ("S".into(), PortMode::Output),
            ],
            active: true,
        }],
        sides: board::SIDES
            .iter()
            .map(|s| SideView {
                side: s.to_string(),
                name: None,
                output: *s == "E",
            })
            .collect(),
        buses: vec![board::BusView {
            index: 2,
            side: "E".into(),
            channels: 32,
            rows: 3,
        }],
        wires: Vec::new(),
        layout: Vec::new(),
        levels: Vec::new(),
    }
}

#[test]
fn pins_sit_on_their_edges() {
    let l = layout(&view(), &Edits::default(), 360.0, Camera::default(), &|t| {
        t.len() as f32 * 6.0
    });
    let chip = l.nodes.iter().find(|n| n.node == Node::Chip(0)).unwrap();
    let a = l.pin(&Pin::new(Node::Chip(0), "A"), false).unwrap();
    let s = l.pin(&Pin::new(Node::Chip(0), "S"), true).unwrap();
    assert_eq!(a.x, chip.x);
    assert_eq!(s.x, chip.x + CHIP_W);
    assert!(
        l.pin(&Pin::new(Node::Chip(0), "A"), true).is_none(),
        "an input is no source"
    );
    // In:E is an output side now: drawn, not usable.
    assert!(!l.pin(&Pin::new(Node::In, "E"), true).unwrap().usable);
    assert!(l.pin(&Pin::new(Node::Out, "E"), false).unwrap().usable);
    // Nothing overlaps the panels.
    let (inp, outp) = (
        l.nodes.iter().find(|n| n.node == Node::In).unwrap(),
        l.nodes.iter().find(|n| n.node == Node::Out).unwrap(),
    );
    assert!(chip.x >= inp.x + inp.w && chip.x + CHIP_W <= outp.x);
}

#[test]
fn slots_fit_the_screen() {
    let def = yog_kit::inventory_of::<super::View>().unwrap();
    assert_eq!(def.slot_count, board::CHIP_SLOTS as usize);
    assert!(def.include_player_inventory);
    let (px, py) = def.player_inv_offset;
    assert!(px + 9.0 * 18.0 <= WIDTH && py + 76.0 <= HEIGHT, "{px},{py}");
    assert!(
        def.layout
            .iter()
            .all(|a| a.x + 16.0 <= px && a.y + 16.0 <= HEIGHT),
        "{:?}",
        def.layout
    );
    assert_eq!(def.layout[0].accepts.len(), 6);
}

#[test]
fn the_wheel_zooms_about_the_pointer_and_chips_follow_the_view() {
    let cam = Camera::default().zoomed(2.0, (100.0, 50.0));
    assert_eq!(
        cam.to_local(cam.to_world((100.0, 50.0))),
        (100.0, 50.0),
        "the point under the pointer stays"
    );
    assert_eq!(cam.zoom, 2.0);
    assert_eq!(
        Camera::default().zoomed(100.0, (0.0, 0.0)).zoom,
        Camera::MAX
    );
    let moved = Camera {
        zoom: 1.0,
        pan: (-500.0, 0.0),
    };
    let l = layout(&view(), &Edits::default(), 360.0, moved, &|t| {
        t.len() as f32 * 6.0
    });
    let chip = l.nodes.iter().find(|n| n.node == Node::Chip(0)).unwrap();
    assert!(chip.x < 0.0, "panned out of the box, not held in it");
    let side = l.nodes.iter().find(|n| n.node == Node::In).unwrap();
    assert_eq!(side.x, -498.0, "the panels move with the view");
    // A panel is as wide as its longest name.
    let mut named = view();
    named.sides[0].name = Some("A very long port name, longer than the title".into());
    let l = layout(&named, &Edits::default(), 360.0, Camera::default(), &|t| {
        t.len() as f32 * 6.0
    });
    let inp = l.nodes.iter().find(|n| n.node == Node::In).unwrap();
    assert_eq!(
        inp.w,
        "N A very long port name, longer than the title".len() as f32 * 6.0 + 8.0
    );
}

#[test]
fn a_bus_shows_its_rows_each_a_pin_to_drive_and_one_to_read() {
    let l = layout(&view(), &Edits::default(), 360.0, Camera::default(), &|t| {
        t.len() as f32 * 6.0
    });
    let bus = l.nodes.iter().find(|n| n.node == Node::Bus(2)).unwrap();
    assert_eq!(bus.labels.len(), 3);
    let drive = l.pin(&Pin::new(Node::Bus(2), "3"), false).unwrap();
    let read = l.pin(&Pin::new(Node::Bus(2), "3"), true).unwrap();
    assert_eq!((drive.x, read.x), (bus.x, bus.x + BUS_W));
    // Pulled open: more rows.
    let l = layout(
        &view(),
        &Edits {
            moved: HashMap::new(),
            rows: HashMap::from([(2, 10)]),
        },
        360.0,
        Camera::default(),
        &|t| t.len() as f32 * 6.0,
    );
    assert!(l.pin(&Pin::new(Node::Bus(2), "10"), true).is_some());
}

#[test]
fn zooming_leaves_every_node_where_it_is_in_the_world() {
    let at = |zoom| {
        let l = layout(
            &view(),
            &Edits::default(),
            360.0,
            Camera {
                zoom,
                pan: (0.0, 0.0),
            },
            &|t| t.len() as f32 * 6.0,
        );
        l.nodes
            .iter()
            .map(|n| (n.node, n.x / zoom, n.y / zoom))
            .collect::<Vec<_>>()
    };
    assert_eq!(at(1.0), at(2.0));
    assert_eq!(at(1.0), at(0.5));
}

#[test]
fn hits_the_nearest_pin() {
    let l = layout(&view(), &Edits::default(), 360.0, Camera::default(), &|t| {
        t.len() as f32 * 6.0
    });
    let s = l.pin(&Pin::new(Node::Chip(0), "S"), true).unwrap().clone();
    assert_eq!(l.hit(s.x + 3.0, s.y - 2.0).map(|p| &p.pin), Some(&s.pin));
    assert!(l.hit(180.0, 139.0).is_none());
}

/// A recording of what's drawn: one JSON line a rect or a text. Texts
/// come in untranslated (no game in tests): they're translated here
/// from a `.yres` (`LANG`), measured as the game's font would be.
struct Recorder(std::cell::RefCell<Vec<String>>, HashMap<String, String>);

/// What follows a translation key's namespace.
const KEY_SEP: char = '.';

impl Recorder {
    fn new() -> Self {
        let mut lang = HashMap::new();
        if let Ok(path) = std::env::var("LANG_YRES")
            && let Ok(text) = std::fs::read_to_string(path)
            && let Ok(doc) = yog_api::yog_data::text::parse(&text)
            && let Some(map) = doc.value.as_map()
        {
            for (k, v) in map.iter() {
                let k = k.to_string();
                let v = v.as_str().map(str::to_string).or_else(|| {
                    v.get(yog_api::yog_ids::word::VALUE)
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                });
                if let Some(v) = v {
                    lang.insert(k, v);
                }
            }
        }
        Recorder(Default::default(), lang)
    }
    fn shown(&self, text: &str) -> String {
        // A key's name after the mod's namespace (`<mod>.status.idle`).
        let name = text.split_once(KEY_SEP).map_or(text, |(_, name)| name);
        self.1
            .get(name)
            .cloned()
            .unwrap_or_else(|| text.to_string())
    }
}

impl Canvas for Recorder {
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, color: u32) {
        self.0
            .borrow_mut()
            .push(format!("{{\"r\":[{x0},{y0},{x1},{y1},{color}]}}"));
    }
    fn text(&self, text: &str, x: f32, y: f32, scale: f32, color: u32) {
        self.0.borrow_mut().push(format!(
            "{{\"t\":[{:?},{x},{y},{scale},{color}]}}",
            self.shown(text)
        ));
    }
    fn text_width(&self, text: &str) -> f32 {
        self.shown(text).chars().count() as f32 * 6.0
    }
}

/// The board of a ready-made design's ALU, as the screen gets it.
fn design_board(name: &str) -> BoardView {
    let d = crate::designs::named(name).unwrap();
    let c = d.circuit(d.tier());
    let alu = &c.alus[0];
    let pin = |p: &crate::vm::NestedPin| match p {
        crate::vm::NestedPin::In(i) => Pin::new(Node::In, board::SIDES[*i as usize]),
        crate::vm::NestedPin::Out(i) => Pin::new(Node::Out, board::SIDES[*i as usize]),
        crate::vm::NestedPin::Chip { slot, label } => Pin::new(Node::Chip(*slot), label.as_str()),
        crate::vm::NestedPin::Bus { side, channel } => {
            Pin::new(Node::Bus(*side), (channel + 1).to_string())
        }
    };
    let outs: Vec<&str> = alu
        .wires
        .iter()
        .filter_map(|(_, t)| match t {
            crate::vm::NestedPin::Out(i) => Some(board::SIDES[*i as usize]),
            _ => None,
        })
        .collect();
    BoardView {
        alu: board::AluId { dimension: String::new(), x: 0, y: 0, z: 0 },
        tier: alu.tier,
        chips: alu
            .chips
            .iter()
            .map(|(slot, m)| ChipView { slot: *slot, name: m.name.clone(), tier: m.tier, pins: m.ports.iter().map(|p| (p.label.clone(), p.dir)).collect(), active: true })
            .collect(),
        sides: board::SIDES
            .iter()
            .filter(|s| alu.wires.iter().any(|(f, t)| [f, t].iter().any(|p| matches!(p, crate::vm::NestedPin::In(i) | crate::vm::NestedPin::Out(i) if board::SIDES[*i as usize] == **s))))
            .map(|s| SideView { side: s.to_string(), name: None, output: outs.contains(s) })
            .collect(),
        buses: Vec::new(),
        wires: alu.wires.iter().map(|(f, t)| Wire { from: pin(f), to: pin(t) }).collect(),
        layout: Vec::new(),
        levels: alu.wires.iter().enumerate().filter(|(k, _)| k % 3 == 0).map(|(_, (f, _))| (pin(f), 15)).collect(),
    }
}

/// Draws a few boards into `OUT` (a directory), a JSON-lines file each,
/// for `tools/render_board.py` to make pictures of.
#[test]
#[ignore]
fn render_boards() {
    let Ok(dir) = std::env::var("OUT") else {
        return;
    };
    let boards = [
        ("simple", view()),
        ("full_adder", design_board("Full adder")),
        ("decoder", design_board("Decoder 2:4")),
    ];
    for (name, v) in boards {
        let mut g = Graph::new(Some(v));
        let rec = Recorder::new();
        let size = |k: &str, d: f32| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d)
        };
        draw(
            &rec,
            &mut g,
            Rect {
                x: 0.0,
                y: 0.0,
                w: size("BOARD_W", 460.0),
                h: size("BOARD_H", 170.0),
            },
        );
        std::fs::write(format!("{dir}/{name}.jsonl"), rec.0.borrow().join("\n")).unwrap();
    }
}
