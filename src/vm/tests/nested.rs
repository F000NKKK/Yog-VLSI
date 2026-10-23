use super::*;

/// A NOT gate: port IN at x 0 → wire → block → torch → port OUT at x 4.
fn not_chip(budget: u32) -> NestedChip {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
    vm.set_block(1, 0, 0, BlockType::RedstoneWire);
    vm.set_block(2, 0, 0, BlockType::Solid);
    vm.set_block(
        3,
        0,
        0,
        BlockType::RedstoneTorch {
            lit: true,
            attach: Facing::West,
        },
    );
    vm.set_block(4, 0, 0, BlockType::Port(PortMode::Output));
    vm.settle(8);
    let port = |label: &str, x, mode| NestedPort {
        label: label.into(),
        x,
        y: 0,
        z: 0,
        mode,
    };
    NestedChip::new(
        0,
        vec![
            port("IN", 0, PortMode::Input),
            port("OUT", 4, PortMode::Output),
        ],
        budget,
        vm,
        "test:not",
    )
}

/// An outer circuit: input port, the ALU (its west face in, its east face out), output port.
fn outer(budget: u32) -> RedstoneVM {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
    vm.set_block(1, 0, 0, BlockType::Alu);
    vm.set_block(2, 0, 0, BlockType::Port(PortMode::Output));
    let wires = vec![
        (
            NestedPin::In(Facing::West.index() as u8),
            NestedPin::Chip {
                slot: 0,
                label: "IN".into(),
            },
        ),
        (
            NestedPin::Chip {
                slot: 0,
                label: "OUT".into(),
            },
            NestedPin::Out(Facing::East.index() as u8),
        ),
    ];
    vm.set_nested(1, 0, 0, NestedAlu::new(vec![not_chip(budget)], wires));
    vm.new_tick();
    vm.settle(8);
    vm
}

#[test]
fn cables_and_redstone_ports_carry_an_alus_faces_inside_a_chip() {
    // IN → wire → port (input) → cable → ALU (a NOT) → cable → port (output) → wire → OUT.
    let mut vm = RedstoneVM::new(16);
    vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
    vm.set_block(1, 0, 0, BlockType::RedstoneWire);
    vm.set_block(2, 0, 0, BlockType::RedstonePort { output: false });
    vm.set_block(3, 0, 0, BlockType::Cable);
    vm.set_block(4, 0, 0, BlockType::Alu);
    vm.set_block(5, 0, 0, BlockType::Cable);
    vm.set_block(6, 0, 0, BlockType::Cable);
    vm.set_block(7, 0, 0, BlockType::RedstonePort { output: true });
    vm.set_block(8, 0, 0, BlockType::RedstoneWire);
    vm.set_block(9, 0, 0, BlockType::Port(PortMode::Output));
    let wires = vec![
        (
            NestedPin::In(Facing::West.index() as u8),
            NestedPin::Chip {
                slot: 0,
                label: "IN".into(),
            },
        ),
        (
            NestedPin::Chip {
                slot: 0,
                label: "OUT".into(),
            },
            NestedPin::Out(Facing::East.index() as u8),
        ),
    ];
    vm.set_nested(4, 0, 0, NestedAlu::new(vec![not_chip(8)], wires));
    vm.new_tick();
    vm.settle(128);
    assert_eq!(
        vm.get_port_output(9, 0, 0),
        15,
        "NOT of nothing, out through the cable"
    );
    vm.set_port_input(0, 0, 0, 15);
    for _ in 0..3 {
        vm.new_tick();
        vm.settle(128);
    }
    assert_eq!(vm.get_port_output(9, 0, 0), 0, "in through the cable too");
    vm.set_port_input(0, 0, 0, 0);
    for _ in 0..3 {
        vm.new_tick();
        vm.settle(128);
    }
    assert_eq!(
        vm.get_port_output(9, 0, 0),
        15,
        "and back: a cable holds no level of its own"
    );
}

#[test]
fn a_bus_inside_a_chip_carries_channels_between_alus() {
    // IN → wire → ALU 1 (west in, onto bus channel 3) ═ bus ═ ALU 2
    // (channel 3 into a NOT, out east) → OUT.
    let mut vm = RedstoneVM::new(16);
    vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
    vm.set_block(1, 0, 0, BlockType::RedstoneWire);
    vm.set_block(2, 0, 0, BlockType::Alu);
    vm.set_block(3, 0, 0, BlockType::Bus);
    vm.set_block(4, 0, 0, BlockType::Bus);
    vm.set_block(5, 0, 0, BlockType::Alu);
    vm.set_block(6, 0, 0, BlockType::Port(PortMode::Output));
    let (w, e) = (Facing::West.index() as u8, Facing::East.index() as u8);
    vm.set_nested(
        2,
        0,
        0,
        NestedAlu::new(
            Vec::new(),
            vec![(
                NestedPin::In(w),
                NestedPin::Bus {
                    side: e,
                    channel: 3,
                },
            )],
        ),
    );
    let wires = vec![
        (
            NestedPin::Bus {
                side: w,
                channel: 3,
            },
            NestedPin::Chip {
                slot: 0,
                label: "IN".into(),
            },
        ),
        (
            NestedPin::Chip {
                slot: 0,
                label: "OUT".into(),
            },
            NestedPin::Out(e),
        ),
    ];
    vm.set_nested(5, 0, 0, NestedAlu::new(vec![not_chip(8)], wires));
    let run = |vm: &mut RedstoneVM| {
        for _ in 0..3 {
            vm.new_tick();
            vm.settle(128);
        }
    };
    run(&mut vm);
    assert_eq!(vm.get_port_output(6, 0, 0), 15, "NOT of an idle channel");
    vm.set_port_input(0, 0, 0, 15);
    run(&mut vm);
    assert_eq!(vm.get_port_output(6, 0, 0), 0, "the channel carried it");
    vm.set_port_input(0, 0, 0, 0);
    run(&mut vm);
    assert_eq!(vm.get_port_output(6, 0, 0), 15);
}

#[test]
fn an_alu_in_a_circuit_runs_its_chips_the_same_tick() {
    let mut vm = outer(8);
    assert_eq!(vm.get_port_output(2, 0, 0), 15, "NOT of nothing");
    vm.set_port_input(0, 0, 0, 15);
    vm.new_tick();
    vm.settle(128);
    assert_eq!(vm.get_port_output(2, 0, 0), 0, "answered this tick");
}

#[test]
fn a_nested_chip_keeps_its_own_speed_however_fast_the_circuit_around_it() {
    // One step a tick for the inner chip; a NOT gate needs two.
    let mut vm = outer(1);
    vm.new_tick();
    vm.settle(128);
    vm.new_tick();
    vm.settle(128);
    let before = vm.get_port_output(2, 0, 0);
    vm.set_port_input(0, 0, 0, if before == 15 { 15 } else { 0 });
    vm.new_tick();
    vm.settle(128);
    assert_eq!(
        vm.get_port_output(2, 0, 0),
        before,
        "128 steps around it, still one of its own: not yet"
    );
    vm.new_tick();
    vm.settle(128);
    vm.new_tick();
    vm.settle(128);
    assert_ne!(
        vm.get_port_output(2, 0, 0),
        before,
        "its own steps, a tick at a time"
    );
}
