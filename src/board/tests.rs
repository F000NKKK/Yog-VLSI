use super::*;
use crate::chip::Port;

fn chip(slot: u8, ports: &[(&str, PortMode)]) -> (u8, Installed) {
    let ports: Vec<Port> = ports
        .iter()
        .enumerate()
        .map(|(i, (l, d))| Port {
            label: l.to_string(),
            x: i as u32,
            y: 0,
            z: 0,
            dir: *d,
        })
        .collect();
    let circuit = CircuitData {
        size: 16,
        blocks: Vec::new(),
        ports: ports.clone(),
        alus: Vec::new(),
    };
    let meta = ChipMeta {
        id: format!("c{slot}"),
        tier: Tier::ALL[0],
        name: "Chip".into(),
        ports,
    };
    (slot, Installed::new(meta, circuit.clone(), circuit.vm()))
}

fn board() -> Board {
    let mut b = Board::new(Tier::ALL[0]);
    for (slot, c) in [
        chip(0, &[("A", PortMode::Input), ("Q", PortMode::Output)]),
        chip(1, &[("D", PortMode::Bidirectional)]),
    ] {
        b.chips[slot as usize] = Some(c);
    }
    b
}

fn connect(b: &mut Board, from: Pin, to: Pin) -> Result<(), String> {
    let alu = AluId {
        dimension: String::new(),
        x: 0,
        y: 0,
        z: 0,
    };
    b.apply(Request::Connect { alu, from, to })
}

#[test]
fn wires_run_from_sources_to_sinks() {
    let mut b = board();
    assert!(
        connect(
            &mut b,
            Pin::new(Node::Chip(0), "Q"),
            Pin::new(Node::Chip(1), "D")
        )
        .is_ok()
    );
    assert!(
        connect(
            &mut b,
            Pin::new(Node::Chip(1), "D"),
            Pin::new(Node::Chip(0), "A")
        )
        .is_ok(),
        "bidi is both"
    );
    assert!(
        connect(
            &mut b,
            Pin::new(Node::Chip(0), "A"),
            Pin::new(Node::Out, "N")
        )
        .is_err(),
        "A is an input"
    );
    assert!(
        connect(&mut b, Pin::new(Node::In, "N"), Pin::new(Node::Out, "X")).is_err(),
        "no side X"
    );
    assert_eq!(b.saved.wires.len(), 2);
}

#[test]
fn a_sink_takes_one_wire() {
    let mut b = board();
    connect(
        &mut b,
        Pin::new(Node::In, "N"),
        Pin::new(Node::Chip(0), "A"),
    )
    .unwrap();
    connect(
        &mut b,
        Pin::new(Node::In, "S"),
        Pin::new(Node::Chip(0), "A"),
    )
    .unwrap();
    assert_eq!(
        b.saved.wires,
        [Wire {
            from: Pin::new(Node::In, "S"),
            to: Pin::new(Node::Chip(0), "A")
        }]
    );
}

#[test]
fn a_side_is_an_input_or_an_output() {
    let mut b = board();
    connect(
        &mut b,
        Pin::new(Node::In, "N"),
        Pin::new(Node::Chip(0), "A"),
    )
    .unwrap();
    connect(
        &mut b,
        Pin::new(Node::Chip(0), "Q"),
        Pin::new(Node::Out, "N"),
    )
    .unwrap();
    assert_eq!(b.saved.wires.len(), 1, "In:N went dark");
    assert!(
        connect(
            &mut b,
            Pin::new(Node::In, "N"),
            Pin::new(Node::Chip(1), "D")
        )
        .is_err()
    );
}

#[test]
fn chips_past_the_tier_limit_sleep() {
    let mut b = board();
    let max = b.saved.tier.max_chips() as u8;
    let (_, c) = chip(max, &[("Z", PortMode::Output)]);
    b.chips[max as usize] = Some(c);
    assert!(
        connect(
            &mut b,
            Pin::new(Node::Chip(max), "Z"),
            Pin::new(Node::Chip(0), "A")
        )
        .is_err()
    );
}

#[test]
fn a_bus_has_as_many_channels_as_the_tier_sees_each_both_ways() {
    let mut b = Board::new(Tier::Wood);
    let ch = |n: &str| Pin::new(Node::Bus(2), n);
    assert!(!b.is_source(&ch("1")), "no digital cable on E yet");
    b.cables[2] = Some(cables::DIGITAL);
    assert_eq!(b.bus_channels(), 32, "wood: 8 × 4 colors");
    assert!(b.is_source(&ch("1")) && b.is_sink(&ch("1")));
    assert!(b.is_sink(&ch("32")) && !b.is_sink(&ch("33")) && !b.is_source(&ch("0")));
    assert_eq!(
        b.bus_channel(2, "32"),
        Some(31),
        "pins count from 1, Pipes' channels from 0"
    );
    assert_eq!(b.bus_rows(2), BUS_ROWS);
    b.apply(Request::BusRows {
        alu: AluId {
            dimension: String::new(),
            x: 0,
            y: 0,
            z: 0,
        },
        side: 2,
        rows: 500,
    })
    .unwrap();
    assert_eq!(b.bus_rows(2), 32, "no more rows than channels");
    // An In side straight onto a bus channel: a bus takes the sides' signals.
    let alu = AluId {
        dimension: String::new(),
        x: 0,
        y: 0,
        z: 0,
    };
    assert!(
        b.apply(Request::Connect {
            alu,
            from: Pin::new(Node::In, "N"),
            to: ch("5")
        })
        .is_ok()
    );
}
