"""Blocks and block entities of a 1.21 save, read and written in place
(the game must be closed). Light is recomputed by the game: a changed
chunk is marked unlit."""

import math
import os

import nbt


def _state_key(name, props):
    return (name, tuple(sorted((props or {}).items())))


class World:
    def __init__(self, save):
        self.save = save
        self.regions = {}
        self.dirty = set()

    def region(self, rx, rz):
        if (rx, rz) not in self.regions:
            p = os.path.join(self.save, "region", f"r.{rx}.{rz}.mca")
            self.regions[(rx, rz)] = nbt.Region(p)
        return self.regions[(rx, rz)]

    def chunk(self, cx, cz):
        r = self.region(cx >> 5, cz >> 5)
        c = r.chunk(cx & 31, cz & 31)
        if c is None:
            raise KeyError(f"chunk {cx},{cz} not generated")
        return c[1]

    def _section(self, x, y, z, create=True):
        ch = self.chunk(x >> 4, z >> 4)
        sy = y >> 4
        for s in ch.v["sections"].v[1]:
            if s.v["Y"].v == sy:
                return ch, s
        raise KeyError(f"section {sy} missing")

    @staticmethod
    def _decode(s):
        bs = s.v["block_states"].v
        pal = bs["palette"].v[1]
        if "data" not in bs:
            return pal, [0] * 4096
        bits = max(4, math.ceil(math.log2(len(pal))))
        per = 64 // bits
        mask = (1 << bits) - 1
        out = []
        for word in bs["data"].v:
            w = word & 0xFFFFFFFFFFFFFFFF
            for i in range(per):
                if len(out) == 4096:
                    break
                out.append(w >> (i * bits) & mask)
        return pal, out

    @staticmethod
    def _encode(s, pal, idx):
        bs = s.v["block_states"].v
        bs["palette"] = nbt.Tag(nbt.LIST, (nbt.COMP, pal))
        if len(pal) == 1:
            bs.pop("data", None)
            return
        bits = max(4, math.ceil(math.log2(len(pal))))
        per = 64 // bits
        words = []
        for i in range(0, 4096, per):
            w = 0
            for j, v in enumerate(idx[i:i + per]):
                w |= v << (j * bits)
            if w >= 1 << 63:
                w -= 1 << 64
            words.append(w)
        bs["data"] = nbt.Tag(nbt.LARR, words)

    def get(self, x, y, z):
        _, s = self._section(x, y, z)
        pal, idx = self._decode(s)
        p = pal[idx[(y & 15) * 256 + (z & 15) * 16 + (x & 15)]].v
        props = {k: t.v for k, t in p["Properties"].v.items()} if "Properties" in p else {}
        return p["Name"].v, props

    def set_many(self, blocks):
        """blocks: {(x, y, z): (name, props)} — grouped by section."""
        by = {}
        for (x, y, z), st in blocks.items():
            by.setdefault((x >> 4, y >> 4, z >> 4), []).append((x, y, z, st))
        for (cx, sy, cz), items in by.items():
            ch, s = self._section(cx * 16, sy * 16, cz * 16)
            pal, idx = self._decode(s)
            keys = [_state_key(p.v["Name"].v, {k: t.v for k, t in p.v["Properties"].v.items()} if "Properties" in p.v else {}) for p in pal]
            for x, y, z, (name, props) in items:
                k = _state_key(name, props)
                if k not in keys:
                    entry = {"Name": nbt.Tag(nbt.STR, name)}
                    if props:
                        entry["Properties"] = nbt.Tag(nbt.COMP, {a: nbt.Tag(nbt.STR, str(b)) for a, b in props.items()})
                    pal.append(nbt.Tag(nbt.COMP, entry))
                    keys.append(k)
                idx[(y & 15) * 256 + (z & 15) * 16 + (x & 15)] = keys.index(k)
                self.remove_block_entity(x, y, z)
            # Drop palette entries no longer used.
            used = sorted(set(idx))
            remap = {o: n for n, o in enumerate(used)}
            self._encode(s, [pal[o] for o in used], [remap[v] for v in idx])
            ch.v["isLightOn"] = nbt.Tag(nbt.BYTE, 0)
            s.v.pop("BlockLight", None)
            s.v.pop("SkyLight", None)
            self.dirty.add((cx, cz))

    def remove_block_entity(self, x, y, z):
        ch = self.chunk(x >> 4, z >> 4)
        bes = ch.v["block_entities"].v[1]
        bes[:] = [b for b in bes if (b.v["x"].v, b.v["y"].v, b.v["z"].v) != (x, y, z)]

    def add_block_entity(self, x, y, z, tag):
        self.remove_block_entity(x, y, z)
        ch = self.chunk(x >> 4, z >> 4)
        tag.v["x"], tag.v["y"], tag.v["z"] = nbt.Tag(nbt.INT, x), nbt.Tag(nbt.INT, y), nbt.Tag(nbt.INT, z)
        lst = ch.v["block_entities"]
        if lst.v[0] in (None, nbt.END):
            lst.v = (nbt.COMP, lst.v[1])
        lst.v[1].append(tag)
        self.dirty.add((x >> 4, z >> 4))

    def save_all(self):
        for r in self.regions.values():
            r.save()
