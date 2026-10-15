"""Builds YOG-8 running snake.asm into a save (the game must be closed):

    python3 build_world.py <save dir> <yog-data dir> [x y z]

At (x, y, z) — the floor's north-west corner — a 13×13 floor; on it a
netherite ALU holding the YOG-8 chip, an 8×8 screen of lamps (each behind
a Redstone Port on a bus channel, the bus a digital cable plate), and four
buttons below it (UP, LEFT, RIGHT, DOWN), each on a Redstone Port. The
chip's circuit, the ALU's board and the ports' channels go into VLSI's
storage."""

import json
import os
import sys

import asm
import cpu
import gen
import nbt
from world import World

CHIP_ID = "yog8-snake"
# The ALU's buses, by side (N S E W U D): the screen's plate above it, the
# buttons' cable below — two buses, an ALU isn't a cable.
SCREEN_BUS = 4
BUTTON_BUS = 5


def kv_read(path):
    out = {}
    if os.path.exists(path):
        for line in open(path).read().split("\n"):
            if line and not line.startswith("#"):
                k, t, v = line.split("\t", 2)
                out[k] = (t, v)
    return out


def kv_write(path, entries):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    lines = ["# yog-storage v2"] + [f"{k}\t{t}\t{v}" for k, (t, v) in entries.items()]
    open(path, "w").write("\n".join(lines) + "\n")


def main(save, data, ox, oy, oz):
    words = asm.assemble(open(os.path.join(os.path.dirname(__file__), "snake.asm")).read())
    net = cpu.build(words, debug=False)
    circuit = gen.layout(net)
    meta = {"id": CHIP_ID, "tier": "Netherite", "name": "YOG-8", "ports": circuit["ports"]}
    print(f"YOG-8: {len(net.gates)} gates in {len(circuit['alus'])} ALUs")

    # ── Where things go ───────────────────────────────────────────────────
    W = World(save)
    floor_y = oy
    gy = oy + 1                     # ground level (stand here)
    alu = (ox + 6, gy + 2, oz + 3)
    screen_x, screen_top, plate_z = ox + 2, gy + 10, oz + 3
    port_z, lamp_z = plate_z + 1, plate_z + 2
    panel_z = oz + 9
    blocks = {}

    def put(x, y, z, name, **props):
        blocks[(x, y, z)] = (name, {k: str(v).lower() if isinstance(v, bool) else str(v) for k, v in props.items()})

    for x in range(ox, ox + 13):
        for z in range(oz, oz + 13):
            put(x, floor_y, z, "minecraft:smooth_stone")
            for y in range(gy, gy + 13):
                put(x, y, z, "minecraft:air")

    cables = set()
    # The bus: a plate behind the screen, down past the ALU to the floor, to the panel.
    for c in range(8):
        for r in range(8):
            cables.add((screen_x + c, screen_top - r, plate_z))
    ax, ay, az = alu
    cables |= {(ax, ay - 1, az), (ax, ay - 2, az)}
    for z in range(az + 1, panel_z):
        cables.add((ax, gy, z))
    pc = (ax, gy + 1, panel_z - 1)  # the panel's hub, behind its buttons
    cables |= {pc, (pc[0], pc[1] + 1, pc[2]), (pc[0], pc[1] - 1, pc[2]), (pc[0] - 1, pc[1], pc[2]), (pc[0] + 1, pc[1], pc[2])}

    ports = []  # (pos, output, channel, name)
    for r in range(8):
        for c in range(8):
            ports.append(((screen_x + c, screen_top - r, port_z), True, r * 8 + c + 1, ""))
            put(screen_x + c, screen_top - r, lamp_z, "minecraft:redstone_lamp", lit=False)
    controls = {"UP": (0, 1), "DOWN": (0, -1), "LEFT": (-1, 0), "RIGHT": (1, 0)}
    for i, (name, (dx, dy)) in enumerate(controls.items()):
        p = (pc[0] + dx, pc[1] + dy, panel_z)
        ports.append((p, False, 65 + i, name))
        put(p[0], p[1], p[2] + 1, "minecraft:stone_button", face="wall", facing="south", powered=False)

    things = cables | {p for p, *_ in ports} | {alu}
    for (x, y, z) in cables:
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

    # ── VLSI's storage: the circuit, the board, the ports ─────────────────
    kv = os.path.join(data, "yog-vlsi", "global.kv")
    e = kv_read(kv)
    e[f"circuit/{CHIP_ID}"] = ("s", json.dumps(circuit, separators=(",", ":")))
    kv_write(kv, e)

    dim = "minecraft:overworld"
    pin = lambda node, name: {"node": node, "name": name}
    wires = [{"from": pin({"Chip": 0}, f"PX{r}{c}"), "to": pin({"Bus": SCREEN_BUS}, str(r * 8 + c + 1))} for r in range(8) for c in range(8)]
    wires += [{"from": pin({"Bus": BUTTON_BUS}, str(65 + i)), "to": pin({"Chip": 0}, name)} for i, name in enumerate(controls)]
    board = {"tier": "Netherite", "wires": wires, "layout": [], "panels": [], "bus_rows": [[SCREEN_BUS, 64], [BUTTON_BUS, 68]]}
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
    old += [[[dim, *p], {"output": out, "name": name, "channel": ch}] for p, out, ch, name in ports]
    e["ports"] = ("s", json.dumps(old, separators=(",", ":")))
    kv_write(kv, e)
    print(f"built at {ox},{oy},{oz}: ALU {alu}, screen {screen_x}..{screen_x + 7} x {screen_top - 7}..{screen_top}, buttons by {pc}")


if __name__ == "__main__":
    save, data = sys.argv[1], sys.argv[2]
    x, y, z = (int(v) for v in sys.argv[3:6]) if len(sys.argv) > 5 else (30, 71, 15)
    main(save, data, x, y, z)
