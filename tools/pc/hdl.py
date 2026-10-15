"""A gate-level netlist: signals are numbers, gates are VLSI's ready-made
designs (one chip each). Every signal is a bus channel in the generated chip;
a signal nothing drives reads 0."""

# design name -> (input labels, output label)
KINDS = {
    "NOT": (["A"], "Q"),
    "AND": (["A", "B"], "Q"),
    "OR": (["A", "B"], "Q"),
    "XOR": (["A", "B"], "Q"),
    "NAND": (["A", "B"], "Q"),
    "NOR": (["A", "B"], "Q"),
    "D latch": (["D", "E"], "Q"),
    "D flip-flop": (["D", "C"], "Q"),
}


class Net:
    def __init__(self):
        self.n = 1  # signal 0 is never driven: constant 0
        self.gates = []  # (kind, [input signals], output signal)
        self.inputs = {}  # port label -> signal
        self.outputs = {}  # port label -> signal
        self.ZERO = 0
        self.ONE = self.gate("NOT", 0)

    def sig(self):
        self.n += 1
        return self.n - 1

    def gate(self, kind, *ins):
        assert len(ins) == len(KINDS[kind][0]), kind
        q = self.sig()
        self.gates.append((kind, list(ins), q))
        return q

    # One-bit helpers.
    def NOT(self, a): return self.gate("NOT", a)
    def AND(self, a, b): return self.gate("AND", a, b)
    def OR(self, a, b): return self.gate("OR", a, b)
    def XOR(self, a, b): return self.gate("XOR", a, b)
    def NOR(self, a, b): return self.gate("NOR", a, b)
    def NAND(self, a, b): return self.gate("NAND", a, b)
    def LATCH(self, d, e): return self.gate("D latch", d, e)
    def DFF(self, d, c): return self.gate("D flip-flop", d, c)

    def MUX(self, s, a, b):
        """s ? b : a"""
        return self.OR(self.AND(a, self.NOT(s)), self.AND(b, s))

    def tree(self, op, xs):
        xs = list(xs)
        if not xs:
            return self.ZERO
        while len(xs) > 1:
            nxt = [op(xs[i], xs[i + 1]) for i in range(0, len(xs) - 1, 2)]
            if len(xs) % 2:
                nxt.append(xs[-1])
            xs = nxt
        return xs[0]

    def any(self, xs): return self.tree(self.OR, xs)
    def all(self, xs): return self.tree(self.AND, xs)

    # Words: lists of signals, bit 0 first.
    def const(self, value, width):
        return [self.ONE if value >> i & 1 else self.ZERO for i in range(width)]

    def add(self, a, b, cin):
        out, c = [], cin
        for x, y in zip(a, b):
            t = self.XOR(x, y)
            out.append(self.XOR(t, c))
            c = self.OR(self.AND(x, y), self.AND(t, c))
        return out, c

    def inc(self, a):
        out, c = [], self.ONE
        for x in a:
            out.append(self.XOR(x, c))
            c = self.AND(x, c)
        return out

    def mux_word(self, s, a, b):
        ns = self.NOT(s)
        return [self.OR(self.AND(x, ns), self.AND(y, s)) for x, y in zip(a, b)]

    def select(self, choices, width):
        """One-hot select: OR over (enable AND word) — the enables never
        overlap."""
        return [self.any([self.AND(en, w[i]) for en, w in choices]) for i in range(width)]

    def decoder(self, addr):
        """Every value of `addr` (bits, LSB first) as a one-hot line."""
        lines = [self.ONE]
        for bit in addr:
            nb = self.NOT(bit)
            lines = [self.AND(l, nb) for l in lines] + [self.AND(l, bit) for l in lines]
        return lines

    def register(self, d, load, clk):
        """Takes d on clk's rising edge while load is on."""
        q = []
        for x in d:
            fb = self.sig()  # its own Q, filled in below
            nxt = self.MUX(load, fb, x)
            out = self.DFF(nxt, clk)
            self.alias(fb, out)
            q.append(out)
        return q

    def alias(self, placeholder, real):
        """A signal used before it was made: every use of `placeholder` is `real`."""
        for g in self.gates:
            g[1][:] = [real if s == placeholder else s for s in g[1]]

    def input(self, label):
        s = self.sig()
        self.inputs[label] = s
        return s

    def output(self, label, s):
        self.outputs[label] = s
