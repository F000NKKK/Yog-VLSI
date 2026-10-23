use super::*;

fn piston(facing: Facing, sticky: bool) -> BlockType {
    BlockType::Piston {
        facing,
        sticky,
        extended: false,
    }
}

#[test]
fn piston_pushes_and_breaks() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(1, 0, 0, piston(Facing::East, false));
    vm.set_block(2, 0, 0, BlockType::Solid);
    vm.set_block(3, 0, 0, BlockType::RedstoneBlock);
    vm.set_block(4, 0, 0, BlockType::RedstoneWire);
    vm.set_block(0, 0, 0, LEVER_ON);
    vm.step();
    assert!(matches!(
        vm.cell(1, 0, 0).block,
        BlockType::Piston { extended: true, .. }
    ));
    assert!(matches!(
        vm.cell(2, 0, 0).block,
        BlockType::PistonHead { .. }
    ));
    assert_eq!(vm.cell(3, 0, 0).block, BlockType::Solid);
    assert_eq!(
        vm.cell(4, 0, 0).block,
        BlockType::RedstoneBlock,
        "the wire broke"
    );

    vm.set_block(
        0,
        0,
        0,
        BlockType::Lever {
            on: false,
            attach: Facing::Down,
        },
    );
    vm.step();
    assert!(matches!(
        vm.cell(1, 0, 0).block,
        BlockType::Piston {
            extended: false,
            ..
        }
    ));
    assert_eq!(
        vm.cell(2, 0, 0).block,
        BlockType::Air,
        "a plain piston leaves it"
    );
}

#[test]
fn piston_stops_at_obsidian_and_twelve_blocks() {
    let mut vm = RedstoneVM::new(32);
    vm.set_block(1, 0, 0, piston(Facing::East, false));
    vm.set_block(2, 0, 0, BlockType::Solid);
    vm.set_block(3, 0, 0, BlockType::Immovable);
    vm.set_block(0, 0, 0, LEVER_ON);
    vm.step();
    assert!(matches!(
        vm.cell(1, 0, 0).block,
        BlockType::Piston {
            extended: false,
            ..
        }
    ));

    let mut vm = RedstoneVM::new(32);
    vm.set_block(1, 0, 0, piston(Facing::East, false));
    for x in 2..15 {
        vm.set_block(x, 0, 0, BlockType::Solid);
    }
    vm.set_block(0, 0, 0, LEVER_ON);
    vm.step();
    assert!(
        matches!(
            vm.cell(1, 0, 0).block,
            BlockType::Piston {
                extended: false,
                ..
            }
        ),
        "13 is too many"
    );
}

#[test]
fn sticky_piston_pulls_and_slime_drags() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(1, 0, 1, piston(Facing::East, true));
    vm.set_block(2, 0, 1, BlockType::Slime);
    vm.set_block(2, 0, 2, BlockType::Solid);
    vm.set_block(0, 0, 1, LEVER_ON);
    vm.step();
    assert_eq!(vm.cell(3, 0, 1).block, BlockType::Slime);
    assert_eq!(
        vm.cell(3, 0, 2).block,
        BlockType::Solid,
        "dragged by the slime"
    );
    vm.set_block(
        0,
        0,
        1,
        BlockType::Lever {
            on: false,
            attach: Facing::Down,
        },
    );
    vm.step();
    assert_eq!(vm.cell(2, 0, 1).block, BlockType::Slime, "pulled back");
    assert_eq!(vm.cell(2, 0, 2).block, BlockType::Solid);
}

#[test]
fn quasi_connectivity() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(1, 0, 0, piston(Facing::East, false));
    // Powering the space above it.
    vm.set_block(0, 1, 0, LEVER_ON);
    vm.step();
    assert!(matches!(
        vm.cell(1, 0, 0).block,
        BlockType::Piston { extended: true, .. }
    ));
}

#[test]
fn hopper_moves_items_and_comparator_reads_them() {
    let mut vm = RedstoneVM::new(16);
    let chest = BlockType::Container {
        slots: 27,
        solid: false,
        partner: None,
    };
    vm.set_block(1, 1, 0, chest);
    vm.set_items(1, 1, 0, &[(0, "minecraft:stone".into(), 64)]);
    vm.set_block(
        1,
        0,
        0,
        BlockType::Hopper {
            facing: Facing::East,
        },
    );
    vm.set_block(
        2,
        0,
        0,
        BlockType::Hopper {
            facing: Facing::Down,
        },
    );
    // A comparator reading the second hopper, facing it (west).
    vm.set_block(
        3,
        0,
        0,
        BlockType::Comparator {
            mode: ComparatorMode::Compare,
            facing: Facing::West,
        },
    );
    // Pulled in step 1, passed on 4 steps later.
    for _ in 0..5 {
        vm.step();
    }
    assert_eq!(vm.items_at(1, 1, 0), [(0, "minecraft:stone".into(), 62)]);
    assert_eq!(
        vm.items_at(2, 0, 0),
        [(0, "minecraft:stone".into(), 1)],
        "{:?}",
        vm.items_at(1, 0, 0)
    );
    vm.step();
    assert_eq!(vm.cell(3, 0, 0).power, 1);

    // Powered, a hopper stops.
    vm.set_block(1, 0, 1, BlockType::RedstoneBlock);
    let before = vm.items_at(1, 1, 0);
    for _ in 0..10 {
        vm.step();
    }
    assert_eq!(vm.items_at(1, 1, 0), before);
}

#[test]
fn dropper_drops_on_a_rising_edge() {
    let mut vm = RedstoneVM::new(16);
    vm.set_block(
        1,
        0,
        0,
        BlockType::Dropper {
            facing: Facing::East,
            triggered: false,
        },
    );
    vm.set_items(1, 0, 0, &[(0, "minecraft:stone".into(), 2)]);
    vm.set_block(
        2,
        0,
        0,
        BlockType::Container {
            slots: 27,
            solid: false,
            partner: None,
        },
    );
    vm.set_block(0, 0, 0, LEVER_ON);
    for _ in 0..6 {
        vm.step();
    }
    assert_eq!(
        vm.items_at(2, 0, 0),
        [(0, "minecraft:stone".into(), 1)],
        "once while held on"
    );
}

#[test]
fn air_costs_no_layers() {
    let mut vm = RedstoneVM::new(256);
    assert_eq!(vm.memory_bytes(), 0);
    vm.set_block(3, 7, 3, BlockType::RedstoneLamp);
    assert_eq!(vm.layers.iter().filter(|l| l.is_some()).count(), 1);
    assert_eq!(vm.block_count(), 1);
    vm.set_block(3, 7, 3, BlockType::Air);
    assert_eq!(vm.block_count(), 0);
    vm.step();
    vm.set_block(300, 0, 0, BlockType::Solid);
    assert_eq!(vm.block_count(), 0);
}

#[test]
fn active_cells_stay_in_grid_order() {
    let mut vm = RedstoneVM::new(16);
    for (x, y, z) in [(5, 2, 1), (0, 0, 0), (3, 2, 0), (15, 15, 15), (1, 0, 9)] {
        vm.set_block(x, y, z, BlockType::Solid);
    }
    assert!(vm.active.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(vm.pos(vm.active[4]), (15, 15, 15));
}

#[test]
fn a_not_gate_answers_in_one_settle_and_a_quiet_circuit_stops_early() {
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
    assert_eq!(vm.get_port_output(4, 0, 0), 15);
    vm.set_port_input(0, 0, 0, 15);
    let steps = vm.settle(8);
    assert_eq!(vm.get_port_output(4, 0, 0), 0, "answered the same tick");
    assert!(steps < 8, "stopped once nothing changed ({steps})");
    assert_eq!(vm.settle(8), 0, "settled, inputs as they were: not a step");
    vm.set_port_input(0, 0, 0, 15);
    assert_eq!(vm.settle(8), 0, "the same input isn't a change");
    vm.set_port_input(0, 0, 0, 0);
    assert!(vm.settle(8) > 0, "a new one is");
    assert_eq!(vm.get_port_output(4, 0, 0), 15);
}
