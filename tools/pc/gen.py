"""Lays a Net out as one VLSI chip: netherite ALUs along one bus (a digital
cable), eight gates each, every signal a channel of the bus; ports on the
faces of I/O ALUs; the clock a ring of repeaters through a torch.

    python3 gen.py snake.asm out.json
"""

import json
import sys

import asm
import cpu
from hdl import KINDS

SIZE = 256
Y = 2           # the bus's height
ZB = 14         # the bus's z
X0 = 7          # where the bus starts (the clock ALU)
FACE = {"N": 0, "S": 1, "E": 2, "W": 3, "U": 4, "D": 5}


def pin_chip(slot, label): return {"Chip": [slot, label]}
def pin_bus(face, ch): return {"Bus": [face, ch]}
def pin_in(face): return {"In": face}
def pin_out(face): return {"Out": face}


def layout(net, clock_repeaters=8, clock_delay=4):
    blocks, ports, alus = [], [], []

    def block(x, y, z, state):
        blocks.append({"x": x, "y": y, "z": z, "state": state})

    def alu(x, y, z, chips, wires):
        alus.append({"x": x, "y": y, "z": z, "tier": "Netherite", "chips": chips, "wires": wires})
        block(x, y, z, "yog-vlsi:alu_netherite")

    # ── The clock: a torch on a block, its signal back into the block ─────
    b, t = (5, Y, ZB - 1), (6, Y, ZB - 1)
    block(*b, "minecraft:stone")
    block(*t, "minecraft:redstone_wall_torch[facing=east,lit=true]")
    rep = lambda facing: f"minecraft:repeater[delay={clock_delay},facing={facing},locked=false,powered=false]"
    n = clock_repeaters
    for i in range(1, n + 1):
        block(6, Y, ZB - 1 - i, rep("south"))   # signal north
        block(5, Y, ZB - 1 - i, rep("north"))   # signal south, into the block
    block(6, Y, ZB - 2 - n, "minecraft:redstone_wire[east=none,north=none,power=0,south=side,west=side]")
    block(5, Y, ZB - 2 - n, "minecraft:redstone_wire[east=side,north=none,power=0,south=side,west=none]")
    assert ZB - 2 - n >= 0

    x = X0
    # The clock's ALU: the torch on its west face onto the bus (south).
    clk = net.inputs["CLK"]
    alu(x, Y, ZB - 1, [], [[pin_in(FACE["W"]), pin_bus(FACE["S"], clk)]])

    # ── I/O: ALUs every other x, three ports each, both sides of the bus ──
    io = [(label, s, "Input") for label, s in net.inputs.items() if label != "CLK"]
    io += [(label, s, "Output") for label, s in net.outputs.items()]
    x += 2
    sides = [
        (ZB - 1, "S", [((0, 0, -1), "N"), ((0, -1, 0), "D"), ((0, 1, 0), "U")]),
        (ZB + 1, "N", [((0, 0, 1), "S"), ((0, -1, 0), "D"), ((0, 1, 0), "U")]),
    ]
    while io:
        for z, bus_face, spots in sides:
            wires = []
            for (dx, dy, dz), face in spots:
                if not io:
                    break
                label, s, mode = io.pop(0)
                ports.append({"label": label, "x": x + dx, "y": Y + dy, "z": z + dz, "dir": mode})
                if mode == "Input":
                    wires.append([pin_in(FACE[face]), pin_bus(FACE[bus_face], s)])
                else:
                    wires.append([pin_bus(FACE[bus_face], s), pin_out(FACE[face])])
            if wires:
                alu(x, Y, z, [], wires)
        x += 2

    # ── Gates: eight to an ALU, four ALUs round each bus cell ─────────────
    driver = {q: i for i, (_, _, q) in enumerate(net.gates)}
    groups = [net.gates[i:i + 8] for i in range(0, len(net.gates), 8)]
    home = {}
    for gi, g in enumerate(groups):
        for slot, (_, _, q) in enumerate(g):
            home[q] = (gi, slot)
    used_outside = set(net.outputs.values())
    for gi, g in enumerate(groups):
        for _, ins, _ in g:
            for s in ins:
                if s in home and home[s][0] != gi:
                    used_outside.add(s)
    spots = [((0, 0, -1), "S"), ((0, 0, 1), "N"), ((0, -1, 0), "U"), ((0, 1, 0), "D")]
    first_gate_x = x
    for gi, g in enumerate(groups):
        (dx, dy, dz), bus_face = spots[gi % 4]
        gx = first_gate_x + gi // 4
        chips, wires = [], []
        for slot, (kind, ins, q) in enumerate(g):
            labels, out = KINDS[kind]
            chips.append([slot, {"id": f"design:{kind}", "tier": "Netherite", "name": kind, "ports": []}])
            for label, s in zip(labels, ins):
                if s == 0:
                    continue  # never driven: off
                src = pin_chip(home[s][1], "Q") if s in home and home[s][0] == gi else pin_bus(FACE[bus_face], s)
                wires.append([src, pin_chip(slot, label)])
            if q in used_outside:
                wires.append([pin_chip(slot, out), pin_bus(FACE[bus_face], q)])
        alu(gx + dx, Y + dy, ZB + dz, chips, wires)
    last_x = first_gate_x + (len(groups) - 1) // 4
    assert last_x < SIZE, last_x
    for bx in range(X0, last_x + 1):
        block(bx, Y, ZB, "yog-vlsi:digital_cable")
    return {"size": SIZE, "blocks": blocks, "ports": ports, "alus": alus}


if __name__ == "__main__":
    words = asm.assemble(open(sys.argv[1]).read())
    net = cpu.build(words)
    circuit = layout(net)
    json.dump(circuit, open(sys.argv[2], "w"))
    print(f"{len(words)} words, {len(net.gates)} gates, {len(circuit['alus'])} ALUs, {len(circuit['ports'])} ports")
