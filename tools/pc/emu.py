"""YOG-8 at the instruction level — to check programs fast."""
from cpu import OPS


class Emu:
    def __init__(self, words):
        self.w = words; self.pc = 0; self.a = 0; self.x = 0; self.c = 0
        self.m = [0] * 32; self.px = [0] * 64; self.r = 0; self.btn = 0

    def step(self):
        word = self.w[self.pc] if self.pc < len(self.w) else 0
        op, k = OPS[word >> 8] if word >> 8 < len(OPS) else "NOP", word & 255
        a, pc = self.a, self.pc + 1
        M = self.m[(self.x if op in ("LDX", "STX") else k) & 31]
        def arith(b, sub):
            t = a + ((~b & 255) if sub else b) + sub
            self.c = t >> 8; return t & 255
        if op == "LDI": a = k
        elif op in ("LD", "LDX"): a = M
        elif op == "ST": self.m[k & 31] = a
        elif op == "STX": self.m[self.x & 31] = a
        elif op == "ADD": a = arith(M, 0)
        elif op == "ADDI": a = arith(k, 0)
        elif op == "SUB": a = arith(M, 1)
        elif op == "SUBI": a = arith(k, 1)
        elif op == "ANDI": a &= k
        elif op == "ORI": a |= k
        elif op == "XORI": a ^= k
        elif op == "JMP": pc = k
        elif op == "JZ" and a == 0: pc = k
        elif op == "JNZ" and a != 0: pc = k
        elif op == "JC" and self.c: pc = k
        elif op == "IN": a = self.btn
        elif op == "RND": a = self.r
        elif op == "PLOT": self.px[a & 63] = a >> 6 & 1
        elif op == "TEST": a = self.px[a & 63]
        elif op == "TAX": self.x = a
        elif op == "TXA": a = self.x
        r = self.r
        fb = ((r >> 7) ^ (r >> 5) ^ (r >> 4) ^ (r >> 3)) & 1 | (r == 0)
        self.r = (r << 1 | fb) & 255
        self.a, self.pc = a, pc & 255
