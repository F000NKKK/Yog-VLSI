"""Minimal NBT + Anvil region reader/writer (Java edition, 1.21)."""
import struct, zlib, gzip, io

END, BYTE, SHORT, INT, LONG, FLOAT, DOUBLE, BARR, STR, LIST, COMP, IARR, LARR = range(13)


class Tag:
    __slots__ = ("t", "v")

    def __init__(self, t, v):
        self.t, self.v = t, v

    def __repr__(self):
        return f"Tag({self.t},{self.v!r})"


def _rs(b, o):
    n = struct.unpack_from(">H", b, o)[0]
    return b[o + 2:o + 2 + n].decode("utf-8", "surrogatepass"), o + 2 + n


def _read(t, b, o):
    if t == BYTE: return struct.unpack_from(">b", b, o)[0], o + 1
    if t == SHORT: return struct.unpack_from(">h", b, o)[0], o + 2
    if t == INT: return struct.unpack_from(">i", b, o)[0], o + 4
    if t == LONG: return struct.unpack_from(">q", b, o)[0], o + 8
    if t == FLOAT: return struct.unpack_from(">f", b, o)[0], o + 4
    if t == DOUBLE: return struct.unpack_from(">d", b, o)[0], o + 8
    if t == BARR:
        n = struct.unpack_from(">i", b, o)[0]; o += 4
        return bytearray(b[o:o + n]), o + n
    if t == STR: return _rs(b, o)
    if t == LIST:
        et, n = struct.unpack_from(">bi", b, o); o += 5
        items = []
        for _ in range(n):
            v, o = _read(et, b, o); items.append(Tag(et, v))
        return (et, items), o
    if t == COMP:
        d = {}
        while True:
            ct = b[o]; o += 1
            if ct == END: return d, o
            name, o = _rs(b, o)
            v, o = _read(ct, b, o); d[name] = Tag(ct, v)
    if t == IARR:
        n = struct.unpack_from(">i", b, o)[0]; o += 4
        return list(struct.unpack_from(f">{n}i", b, o)), o + 4 * n
    if t == LARR:
        n = struct.unpack_from(">i", b, o)[0]; o += 4
        return list(struct.unpack_from(f">{n}q", b, o)), o + 8 * n
    raise ValueError(t)


def _ws(s, out):
    e = s.encode("utf-8", "surrogatepass")
    out += struct.pack(">H", len(e)); out += e


def _write(t, v, out):
    if t == BYTE: out += struct.pack(">b", v)
    elif t == SHORT: out += struct.pack(">h", v)
    elif t == INT: out += struct.pack(">i", v)
    elif t == LONG: out += struct.pack(">q", v)
    elif t == FLOAT: out += struct.pack(">f", v)
    elif t == DOUBLE: out += struct.pack(">d", v)
    elif t == BARR: out += struct.pack(">i", len(v)); out += bytes(v)
    elif t == STR: _ws(v, out)
    elif t == LIST:
        et, items = v
        if not items: et = END if et is None else et
        out += struct.pack(">bi", et, len(items))
        for it in items: _write(et, it.v, out)
    elif t == COMP:
        for k, tg in v.items():
            out += bytes([tg.t]); _ws(k, out); _write(tg.t, tg.v, out)
        out += b"\x00"
    elif t == IARR: out += struct.pack(f">i{len(v)}i", len(v), *v)
    elif t == LARR: out += struct.pack(f">i{len(v)}q", len(v), *v)
    else: raise ValueError(t)


def loads(b):
    """Root compound (name, Tag)."""
    t = b[0]
    name, o = _rs(b, 1)
    v, _ = _read(t, b, o)
    return name, Tag(t, v)


def dumps(name, tag):
    out = bytearray([tag.t]); _ws(name, out); _write(tag.t, tag.v, out)
    return bytes(out)


def read_gz(path):
    with gzip.open(path, "rb") as f:
        return loads(f.read())


def write_gz(path, name, tag):
    with gzip.open(path, "wb") as f:
        f.write(dumps(name, tag))


class Region:
    def __init__(self, path):
        self.path = path
        self.data = bytearray(open(path, "rb").read())
        if len(self.data) < 8192: self.data += bytes(8192 - len(self.data))
        self.chunks = {}  # (lx, lz) -> (name, Tag), loaded lazily

    def _loc(self, i):
        e = struct.unpack_from(">I", self.data, i * 4)[0]
        return e >> 8, e & 0xFF

    def has(self, lx, lz):
        return self._loc(lx + lz * 32)[0] != 0

    def chunk(self, lx, lz):
        if (lx, lz) in self.chunks: return self.chunks[(lx, lz)]
        off, cnt = self._loc(lx + lz * 32)
        if off == 0: return None
        p = off * 4096
        n, comp = struct.unpack_from(">ib", self.data, p)
        raw = bytes(self.data[p + 5:p + 4 + n])
        b = zlib.decompress(raw) if comp == 2 else gzip.decompress(raw) if comp == 1 else raw
        c = loads(b); self.chunks[(lx, lz)] = c
        return c

    def save(self):
        """Writes every loaded chunk back (appended at the end: simple and safe)."""
        for (lx, lz), (name, tag) in self.chunks.items():
            raw = zlib.compress(dumps(name, tag))
            payload = struct.pack(">ib", len(raw) + 1, 2) + raw
            sectors = (len(payload) + 4095) // 4096
            i = lx + lz * 32
            off, cnt = self._loc(i)
            if off and sectors <= cnt:
                start = off
            else:
                start = max(2, (len(self.data) + 4095) // 4096)
            end = (start + sectors) * 4096
            if len(self.data) < end: self.data += bytes(end - len(self.data))
            self.data[start * 4096:start * 4096 + len(payload)] = payload
            self.data[start * 4096 + len(payload):end] = bytes(end - start * 4096 - len(payload))
            struct.pack_into(">I", self.data, i * 4, (start << 8) | sectors)
        open(self.path, "wb").write(self.data)


def walk(tag, f, path=()):
    """Calls f(path, tag) on every tag, depth first."""
    f(path, tag)
    if tag.t == COMP:
        for k, t in tag.v.items(): walk(t, f, path + (k,))
    elif tag.t == LIST:
        for i, t in enumerate(tag.v[1]): walk(t, f, path + (i,))
