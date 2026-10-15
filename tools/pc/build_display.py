"""Builds YOG-8 drawing 3D figures (figures.asm) on a Display into a save,
up in the air (the game must be closed):

    python3 build_display.py <save dir> <yog-data dir> [x y z]

At (x, y, z) — where you stand — a platform with four buttons in its floor,
a cross like a gamepad's (UP away from you, toward the screen). North of it,
past a gap, the Display's picture, 32 blocks a side; behind the screen the
ALU holding the chip, joined to the Display by one digital cable and to the
buttons by another, which runs under the platform. The chip's circuit, the
ALU's board, the ports' channels and the Display go into VLSI's storage."""

import json
import os
import sys

import asm
import cpu
import gen
import nbt
from build_world import kv_read, kv_write
from world import World

CHIP_ID = "yog8-figures"
SCREEN = 32          # the picture's side, in blocks
GAP = 26             # from the screen to the buttons
FACE = {"N": 0, "S": 1, "E": 2, "W": 3, "U": 4, "D": 5}
DISPLAY_BUS = FACE["S"]   # the ALU's side toward the Display
BUTTON_BUS = FACE["D"]
BUTTONS = {"UP": (0, -1), "DOWN": (0, 1), "LEFT": (-1, 0), "RIGHT": (1, 0)}


def main(save, data, px, py, pz):
    words = asm.assemble(open(os.path.join(os.path.dirname(__file__), "figures.asm")).read())
    net = cpu.build(words, debug=False, gpu=True)
    circuit = gen.layout(net)
    meta = {"id": CHIP_ID, "tier": "Netherite", "name": "YOG-8 3D", "ports": circuit["ports"]}
    print(f"YOG-8 3D: {len(words)} words, {len(net.gates)} gates in {len(circuit['alus'])} ALUs")

    W = World(save)
    gy = py                       # where you stand
    floor = gy - 1
    pad = (px, floor, pz)         # the cross's middle, in the floor
    sz = pz - GAP                 # the screen's plane
    sx = px - SCREEN // 2         # its left edge (seen from the platform)
    sy = gy                       # its bottom
    display = (sx, sy, sz)        # the Display: the picture's bottom-left corner
    alu = (sx, sy, sz - 2)
    blocks = {}

    def put(x, y, z, name, **props):
        blocks[(x, y, z)] = (name, {k: str(v).lower() if isinstance(v, bool) else str(v) for k, v in props.items()})

    # The platform.
    for x in range(px - 7, px + 8):
        for z in range(pz - 4, pz + 8):
            put(x, floor, z, "minecraft:smooth_stone")
            for y in range(gy, gy + 4):
                put(x, y, z, "minecraft:air")
    # The screen's back: a wall the picture lies on.
    for x in range(sx, sx + SCREEN):
        for y in range(sy, sy + SCREEN):
            put(x, y, sz, "minecraft:black_concrete")
    put(*display, "yog-vlsi:display", facing="south")

    # Cables: ALU → Display; ALU down, under the gap and the platform, to the cross.
    display_bus = {(sx, sy, sz - 1)}
    button_bus = {(alu[0], alu[1] - 1, alu[2]), (alu[0], alu[1] - 2, alu[2])}
    under = floor - 1
    button_bus |= {(alu[0], under, z) for z in range(alu[2], pz + 1)}
    button_bus |= {(x, under, pz) for x in range(alu[0], px + 2)}
    button_bus |= {(px, under, pz - 1), (px, under, pz + 1)}
    ports = []  # (pos, channel, name)
    for i, (name, (dx, dz)) in enumerate(BUTTONS.items()):
        p = (px + dx, floor, pz + dz)
        ports.append((p, i + 1, name))
        put(p[0], p[1] + 1, p[2], "minecraft:stone_button", face="floor", facing="north", powered=False)
    put(*pad, "minecraft:polished_blackstone")

    for bus, joins in ((display_bus, {display, alu}), (button_bus, {p for p, *_ in ports} | {alu})):
        things = bus | joins
        for (x, y, z) in bus:
            near = lambda dx, dy, dz: (x + dx, y + dy, z + dz) in things
            put(x, y, z, "yog-vlsi:digital_cable", north=near(0, 0, -1), south=near(0, 0, 1), east=near(1, 0, 0),
                west=near(-1, 0, 0), up=near(0, 1, 0), down=near(0, -1, 0))
    for p, *_ in ports:
        put(*p, "yog-vlsi:redstone_port")
    put(*alu, "yog-vlsi:alu_netherite")

    W.set_many(blocks)
    item = nbt.Tag(nbt.COMP, {
        "Slot": nbt.Tag(nbt.BYTE, 0),
        "id": nbt.Tag(nbt.STR, "yog-vlsi:chip_netherite"),
        "count": nbt.Tag(nbt.INT, 1),
        "components": nbt.Tag(nbt.COMP, {"minecraft:custom_data": nbt.Tag(nbt.COMP, {
            "YogVlsiChip": nbt.Tag(nbt.STR, json.dumps(meta, separators=(",", ":")))})}),
    })
    W.add_block_entity(*alu, nbt.Tag(nbt.COMP, {
        "id": nbt.Tag(nbt.STR, "yog:inventory"),
        "keepPacked": nbt.Tag(nbt.BYTE, 0),
        "Items": nbt.Tag(nbt.LIST, (nbt.COMP, [item])),
    }))
    W.save_all()

    # ── VLSI's storage ────────────────────────────────────────────────────
    dim = "minecraft:overworld"
    kv = os.path.join(data, "yog-vlsi", "global.kv")
    e = kv_read(kv)
    e[f"circuit/{CHIP_ID}"] = ("s", json.dumps(circuit, separators=(",", ":")))
    kv_write(kv, e)

    # The board: the Display port onto channels 1–18 (display.rs), the buttons off channels 1–4.
    pin = lambda node, name: {"node": node, "name": name}
    wires = [{"from": pin({"Chip": 0}, f"D{i}"), "to": pin({"Bus": DISPLAY_BUS}, str(1 + i))} for i in range(8)]
    wires += [{"from": pin({"Chip": 0}, f"R{i}"), "to": pin({"Bus": DISPLAY_BUS}, str(9 + i))} for i in range(8)]
    wires += [{"from": pin({"Chip": 0}, "T"), "to": pin({"Bus": DISPLAY_BUS}, "17")},
              {"from": pin({"Bus": DISPLAY_BUS}, "18"), "to": pin({"Chip": 0}, "ACK")}]
    wires += [{"from": pin({"Bus": BUTTON_BUS}, str(ch)), "to": pin({"Chip": 0}, name)} for _, ch, name in ports]
    board = {"tier": "Netherite", "wires": wires, "layout": [], "panels": [], "bus_rows": [[DISPLAY_BUS, 18], [BUTTON_BUS, 4]]}
    kv = os.path.join(data, "yog-vlsi_boards", "global.kv")
    e = kv_read(kv)
    boards = json.loads(e["boards"][1]) if "boards" in e else []
    here = {"dimension": dim, "x": alu[0], "y": alu[1], "z": alu[2]}
    boards = [b for b in boards if b[0] != here] + [[here, board]]
    e["boards"] = ("s", json.dumps(boards, separators=(",", ":")))
    kv_write(kv, e)

    kv = os.path.join(data, "yog-vlsi_redstone_ports", "global.kv")
    e = kv_read(kv)
    old = json.loads(e["ports"][1]) if "ports" in e else []
    mine = {(dim, *p) for p, *_ in ports}
    old = [o for o in old if tuple(o[0]) not in mine]
    old += [[[dim, *p], {"output": False, "name": name, "channel": ch}] for p, ch, name in ports]
    e["ports"] = ("s", json.dumps(old, separators=(",", ":")))
    kv_write(kv, e)

    kv = os.path.join(data, "yog-vlsi_displays", "global.kv")
    e = kv_read(kv)
    old = json.loads(e["displays"][1]) if "displays" in e else []
    old = [o for o in old if tuple(o[0]) != (dim, *display)]
    old.append([[dim, *display], {"facing": "S", "size": SCREEN, "channel": 1}])
    e["displays"] = ("s", json.dumps(old, separators=(",", ":")))
    kv_write(kv, e)
    print(f"built: stand at {px},{gy},{pz}; screen {sx}..{sx + SCREEN - 1} x {sy}..{sy + SCREEN - 1} at z {sz}; ALU {alu}")


if __name__ == "__main__":
    save, data = sys.argv[1], sys.argv[2]
    x, y, z = (int(v) for v in sys.argv[3:6]) if len(sys.argv) > 5 else (64, 110, 60)
    main(save, data, x, y, z)
