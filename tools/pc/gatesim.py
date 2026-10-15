"""Ideal gate-level simulation of a Net: gates settle, flip-flops take D on
their clock's rising edge, latches follow D while E is on."""


class Sim:
    def __init__(self, net):
        self.net = net
        self.v = [0] * net.n
        self.ff_prev_c = {}
        self.ff_q = {}
        # Settle from all zeros the way the redstone does (Q of latches/ffs start 0).
        self.settle()

    def eval(self, kind, ins, q):
        v = self.v
        if kind == "NOT": return 1 - v[ins[0]]
        a, b = v[ins[0]], v[ins[1]]
        if kind == "AND": return a & b
        if kind == "OR": return a | b
        if kind == "XOR": return a ^ b
        if kind == "NAND": return 1 - (a & b)
        if kind == "NOR": return 1 - (a | b)
        if kind == "D latch": return a if b else v[q]
        if kind == "D flip-flop":
            prev = self.ff_prev_c.get(q, 0)
            out = self.ff_q.get(q, 0)
            if b and not prev:
                out = a
            return out
        raise ValueError(kind)

    def settle(self):
        for _ in range(10000):
            changed = False
            # Flip-flops sample on the edge as seen at the start of the pass.
            for kind, ins, q in self.net.gates:
                nv = self.eval(kind, ins, q)
                if nv != self.v[q]:
                    self.v[q] = nv
                    changed = True
            for kind, ins, q in self.net.gates:
                if kind == "D flip-flop":
                    c = self.v[ins[1]]
                    if c and not self.ff_prev_c.get(q, 0):
                        self.ff_q[q] = self.v[q]
                    self.ff_prev_c[q] = c
            if not changed:
                return
        raise RuntimeError("doesn't settle")

    def set(self, label, value):
        self.v[self.net.inputs[label]] = value

    def get(self, label):
        return self.v[self.net.outputs[label]]

    def word(self, prefix):
        return sum(self.get(f"{prefix}{i}") << i for i in range(8))

    def clock(self):
        self.set("CLK", 1); self.settle()
        self.set("CLK", 0); self.settle()

    def screen(self):
        return "\n".join("".join("#" if self.get(f"PX{r}{c}") else "." for c in range(8)) for r in range(8))
