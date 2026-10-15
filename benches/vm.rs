//! Ticking circuits on every chip size: the cost of one VM step (one
//! redstone tick) of a busy one-layer circuit, and of loading it.
//!
//! The circuit fills the bottom layer: every other row is a lever-driven
//! wire line with a repeater every 14 wires (so the signal runs the whole
//! row), rows in between are solid blocks with torches on top — wires,
//! repeaters, torches and solid powering all take part in every step.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion};
use yog_vlsi::content::Tier;
use yog_vlsi::vm::{BlockType, Facing, RedstoneVM};

/// Each world size once (Gold and Iron share 64).
fn sizes() -> impl Iterator<Item = (Tier, u32)> {
    let mut seen = Vec::new();
    Tier::ALL.into_iter().map(|t| (t, t.world_size())).filter(move |&(_, size)| {
        let new = !seen.contains(&size);
        seen.push(size);
        new
    })
}

fn circuit(size: u32) -> RedstoneVM {
    let mut vm = RedstoneVM::new(size);
    for z in 0..size {
        if z % 2 == 0 {
            vm.set_block(0, 0, z, BlockType::Lever { on: true, attach: Facing::Down });
            for x in 1..size {
                let block = if x % 15 == 0 {
                    BlockType::Repeater { delay_ticks: 1, facing: Facing::West }
                } else {
                    BlockType::RedstoneWire
                };
                vm.set_block(x, 0, z, block);
            }
        } else {
            for x in 0..size {
                vm.set_block(x, 0, z, BlockType::Solid);
                if x % 4 == 0 {
                    vm.set_block(x, 1, z, BlockType::RedstoneTorch { lit: true, attach: Facing::Down });
                }
            }
        }
    }
    vm
}

#[yog_api::bench]
fn step(c: &mut Criterion) {
    let mut group = c.benchmark_group("vm_step");
    group.sample_size(20).warm_up_time(Duration::from_millis(500)).measurement_time(Duration::from_secs(3));
    for (tier, size) in sizes() {
        let mut vm = circuit(size);
        for _ in 0..4 {
            vm.step(); // past the repeaters' first switch
        }
        group.bench_function(BenchmarkId::from_parameter(format!("{}_{size}", tier.id())), |b| {
            b.iter(|| {
                vm.step();
                black_box(vm.tick)
            })
        });
    }
    group.finish();
}

#[yog_api::bench]
fn load(c: &mut Criterion) {
    let mut group = c.benchmark_group("vm_load");
    group.sample_size(10).warm_up_time(Duration::from_millis(500)).measurement_time(Duration::from_secs(3));
    for (tier, size) in sizes() {
        group.bench_function(BenchmarkId::from_parameter(format!("{}_{size}", tier.id())), |b| {
            b.iter_batched(|| (), |()| black_box(circuit(size)), BatchSize::PerIteration)
        });
    }
    group.finish();
}
