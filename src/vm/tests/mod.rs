use super::*;

const LEVER_ON: BlockType = BlockType::Lever {
    on: true,
    attach: Facing::Down,
};
const TORCH: BlockType = BlockType::RedstoneTorch {
    lit: true,
    attach: Facing::Down,
};

fn line(vm: &mut RedstoneVM, z: u32, from: u32, to: u32) {
    for x in from..to {
        vm.set_block(x, 0, z, BlockType::RedstoneWire);
    }
}

/// A repeater on row z = 0 sending east (it faces its input, west).
fn repeater(delay: u8) -> BlockType {
    BlockType::Repeater {
        delay_ticks: delay,
        facing: Facing::West,
    }
}

#[test]
fn lever_powers_wire_with_decay() {
    let mut vm = RedstoneVM::new(32);
    vm.set_block(0, 0, 0, LEVER_ON);
    line(&mut vm, 0, 1, 20);
    vm.step();
    assert_eq!(vm.cell(1, 0, 0).power, 15);
    assert_eq!(vm.cell(15, 0, 0).power, 1);
    assert_eq!(vm.cell(16, 0, 0).power, 0);
}

#[test]
fn wire_climbs_and_descends_steps() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(0, 0, 0, LEVER_ON);
    vm.set_block(1, 0, 0, BlockType::RedstoneWire);
    vm.set_block(2, 0, 0, BlockType::Solid);
    vm.set_block(2, 1, 0, BlockType::RedstoneWire);
    vm.set_block(3, 0, 0, BlockType::RedstoneWire);
    vm.step();
    assert_eq!(vm.cell(2, 1, 0).power, 14, "up the step");
    assert_eq!(vm.cell(3, 0, 0).power, 13, "and down again");

    // A block over the lower wire cuts the step.
    vm.set_block(1, 1, 0, BlockType::Solid);
    vm.step();
    assert_eq!(vm.cell(2, 1, 0).power, 0);
}

#[test]
fn wire_powers_the_block_under_it() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(1, 0, 0, BlockType::Solid);
    vm.set_block(0, 1, 0, LEVER_ON);
    vm.set_block(1, 1, 0, BlockType::RedstoneWire);
    vm.step();
    assert_eq!(vm.cell(1, 0, 0).weak, 15);
    assert_eq!(vm.cell(1, 0, 0).strong, 0);
}

#[test]
fn wire_does_not_run_on_through_a_block() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(0, 0, 0, LEVER_ON);
    vm.set_block(1, 0, 0, BlockType::RedstoneWire);
    vm.set_block(2, 0, 0, BlockType::Solid);
    vm.set_block(3, 0, 0, BlockType::RedstoneWire);
    vm.step();
    assert_eq!(vm.cell(2, 0, 0).weak, 15, "the wire points into it");
    assert_eq!(vm.cell(3, 0, 0).power, 0);
}

#[test]
fn wire_only_powers_where_it_points() {
    let mut vm = RedstoneVM::new(16);
    // Wire running east–west past a lamp to its north.
    vm.set_block(0, 0, 1, LEVER_ON);
    line(&mut vm, 1, 1, 4);
    vm.set_block(2, 0, 0, BlockType::RedstoneLamp);
    vm.step();
    assert_eq!(vm.cell(2, 0, 0).power, 0, "the line passes the lamp by");
    // The line's end points on into it.
    vm.set_block(4, 0, 1, BlockType::RedstoneLamp);
    vm.step();
    assert_eq!(vm.cell(4, 0, 1).power, 15);
}

#[test]
fn input_port_drives_output_port() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
    line(&mut vm, 0, 1, 5);
    vm.set_block(5, 0, 0, BlockType::Port(PortMode::Output));
    vm.step();
    assert_eq!(vm.get_port_output(5, 0, 0), 0);

    vm.set_port_input(0, 0, 0, 15);
    vm.step();
    assert_eq!(vm.get_port_output(5, 0, 0), 12);
    assert_eq!(vm.read_outputs(0), vec![(5, 0, 12)]);

    vm.set_port_input(0, 0, 0, 0);
    vm.step();
    assert_eq!(vm.get_port_output(5, 0, 0), 0);
}

#[test]
fn torch_inverts_the_block_it_stands_on() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(1, 0, 0, BlockType::Solid);
    vm.set_block(1, 1, 0, TORCH);
    vm.step();
    assert_eq!(vm.cell(1, 1, 0).power, 15);

    // A lever on the block's side (a lever beside it on the floor wouldn't).
    vm.set_block(
        0,
        0,
        0,
        BlockType::Lever {
            on: true,
            attach: Facing::East,
        },
    );
    vm.step();
    assert_eq!(vm.cell(1, 1, 0).power, 0);
    assert!(matches!(
        vm.cell(1, 1, 0).block,
        BlockType::RedstoneTorch { lit: false, .. }
    ));
}

#[test]
fn torch_strongly_powers_the_block_above() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(1, 1, 0, TORCH);
    vm.set_block(1, 2, 0, BlockType::Solid);
    vm.set_block(2, 2, 0, BlockType::RedstoneWire);
    vm.step();
    assert_eq!(vm.cell(1, 2, 0).strong, 15);
    assert_eq!(
        vm.cell(2, 2, 0).power,
        15,
        "a strongly powered block feeds wire"
    );
}

#[test]
fn wall_torch_hangs_on_its_wall() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(
        0,
        0,
        0,
        BlockType::Lever {
            on: true,
            attach: Facing::East,
        },
    );
    vm.set_block(1, 0, 0, BlockType::Solid);
    // facing=east: the wall is to its west.
    vm.set_block(
        2,
        0,
        0,
        BlockType::RedstoneTorch {
            lit: true,
            attach: Facing::West,
        },
    );
    vm.step();
    assert_eq!(vm.cell(2, 0, 0).power, 0);
}

#[test]
fn repeater_faces_its_input() {
    // facing=east: reads from the east, sends west.
    let mut vm = RedstoneVM::new(16);
    vm.set_block(3, 0, 0, LEVER_ON);
    vm.set_block(
        2,
        0,
        0,
        BlockType::Repeater {
            delay_ticks: 1,
            facing: Facing::East,
        },
    );
    vm.set_block(1, 0, 0, BlockType::RedstoneWire);
    vm.set_block(4, 0, 0, BlockType::RedstoneWire);
    vm.step();
    vm.step();
    assert_eq!(vm.cell(1, 0, 0).power, 15);

    // Turned the other way it sends nothing west.
    vm.set_block(
        2,
        0,
        0,
        BlockType::Repeater {
            delay_ticks: 1,
            facing: Facing::West,
        },
    );
    vm.step();
    vm.step();
    assert_eq!(vm.cell(1, 0, 0).power, 0);
}

#[test]
fn repeater_waits_its_delay() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(0, 0, 0, LEVER_ON);
    vm.set_block(1, 0, 0, BlockType::RedstoneWire);
    vm.set_block(2, 0, 0, repeater(3));
    line(&mut vm, 0, 3, 6);
    let powered_at: Vec<bool> = (0..5)
        .map(|_| {
            vm.step();
            vm.cell(3, 0, 0).power > 0
        })
        .collect();
    // Input seen in step 1, switched at the end of step 3, on the wire in step 4.
    assert_eq!(powered_at, [false, false, false, true, true]);
    assert_eq!(vm.cell(3, 0, 0).power, 15);
}

#[test]
fn repeater_stretches_a_short_pulse() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(0, 0, 0, LEVER_ON);
    vm.set_block(1, 0, 0, repeater(4));
    vm.step();
    vm.set_block(
        0,
        0,
        0,
        BlockType::Lever {
            on: false,
            attach: Facing::Down,
        },
    );
    let on: Vec<bool> = (0..10)
        .map(|_| {
            vm.step();
            vm.cell(1, 0, 0).power > 0
        })
        .collect();
    assert_eq!(on.iter().filter(|o| **o).count(), 4, "{on:?}");
}

#[test]
fn repeater_locked_by_its_side() {
    let mut vm = RedstoneVM::new(16);
    // The locking repeater at (1, 0, 1) sends south into the side of
    // the one at (1, 0, 2), which sends east.
    vm.set_block(1, 0, 0, LEVER_ON);
    vm.set_block(
        1,
        0,
        1,
        BlockType::Repeater {
            delay_ticks: 1,
            facing: Facing::North,
        },
    );
    vm.set_block(1, 0, 2, repeater(1));
    vm.step();
    vm.step();
    vm.set_block(0, 0, 2, LEVER_ON);
    vm.step();
    vm.step();
    assert_eq!(vm.cell(1, 0, 2).power, 0, "locked off");
    vm.set_block(
        1,
        0,
        0,
        BlockType::Lever {
            on: false,
            attach: Facing::Down,
        },
    );
    vm.step();
    vm.step();
    vm.step();
    assert_eq!(vm.cell(1, 0, 2).power, 15, "unlocked, it takes its input");
}

#[test]
fn comparator_compares_and_subtracts() {
    let run = |mode| {
        let mut vm = RedstoneVM::new(16);
        vm.set_block(0, 0, 1, BlockType::RedstoneBlock);
        vm.set_block(
            1,
            0,
            1,
            BlockType::Comparator {
                mode,
                facing: Facing::West,
            },
        );
        vm.set_block(1, 0, 0, BlockType::RedstoneWire);
        vm.set_block(1, 0, 0, BlockType::RedstoneWire);
        vm.set_block(0, 0, 0, BlockType::Port(PortMode::Input));
        vm.set_port_input(0, 0, 0, 7);
        vm.step();
        vm.cell(1, 0, 1).power
    };
    assert_eq!(run(ComparatorMode::Compare), 15);
    assert_eq!(run(ComparatorMode::Subtract), 8);
}

#[test]
fn observer_pulses_once_per_change() {
    let mut vm = RedstoneVM::new(16);
    // Watches east, outputs west.
    vm.set_block(
        1,
        0,
        0,
        BlockType::Observer {
            facing: Facing::East,
        },
    );
    vm.set_block(0, 0, 0, BlockType::RedstoneWire);
    vm.step();
    vm.step();
    vm.set_block(2, 0, 0, BlockType::Solid);
    let out: Vec<u8> = (0..4)
        .map(|_| {
            vm.step();
            vm.cell(0, 0, 0).power
        })
        .collect();
    assert_eq!(out, [0, 15, 0, 0]);
}

mod moving;
mod nested;
