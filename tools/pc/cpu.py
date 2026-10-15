"""YOG-8: an 8-bit accumulator computer, gate by gate.

One instruction per clock (registers take their new values on the rising
edge; memory and the screen are written while the clock is low, when the
address and data are steady).

Word: 13 bits — opcode (5) and an 8-bit operand k.
A, X: 8-bit registers. C: carry. RAM: 32 bytes. Screen: 8×8 pixels.
"""

from hdl import Net

OPS = [
    "NOP",   # 0
    "LDI",   # A = k
    "LD",    # A = M[k]
    "ST",    # M[k] = A
    "ADD",   # A = A + M[k], C
    "ADDI",  # A = A + k, C
    "SUB",   # A = A - M[k], C = no borrow
    "ANDI",  # A = A & k
    "ORI",   # A = A | k
    "XORI",  # A = A ^ k
    "JMP",   # PC = k
    "JZ",    # if A == 0
    "JNZ",   # if A != 0
    "JC",    # if C
    "IN",    # A = the direction (0 up, 1 down, 2 left, 3 right): the button pressed
             # since the last IN (up first), unless it's straight back; kept as the direction
    "RND",   # A = an 8-bit LFSR, stepping every clock
    "PLOT",  # pixel A[5:0] = A[6]
    "TEST",  # A = pixel A[5:0]
    "TAX",   # X = A
    "LDX",   # A = M[X]
    "STX",   # M[X] = A
    "TXA",   # A = X
    "SUBI",  # A = A - k, C = no borrow
    "SETD",  # the direction = A[1:0]
    "MINC",  # A = A + 1 in the bits of k, the rest kept (wraps within them)
    "MDEC",  # A = A - 1 in the bits of k, the rest kept
]
OP = {n: i for i, n in enumerate(OPS)}
RAM_BYTES = 32
BUTTONS = ["UP", "DOWN", "LEFT", "RIGHT"]


def build(program, debug=True):
    """The computer running `program` (13-bit words) from its ROM."""
    n = Net()
    clk = n.input("CLK")
    btn = [n.input(b) for b in BUTTONS]

    # Registers first as placeholders: their outputs are used before they're made.
    A = [n.sig() for _ in range(8)]
    X = [n.sig() for _ in range(8)]
    PC = [n.sig() for _ in range(8)]
    C = n.sig()
    R = [n.sig() for _ in range(8)]

    # ── ROM: the word at PC ───────────────────────────────────────────────
    lo, hi = n.decoder(PC[:4]), n.decoder(PC[4:])
    words = len(program)
    assert words <= 256
    line = [n.AND(lo[a & 15], hi[a >> 4]) for a in range(words)]
    word = [n.any([line[a] for a in range(words) if program[a] >> b & 1]) for b in range(13)]
    k, opbits = word[:8], word[8:13]

    dec = n.decoder(opbits)
    op = {name: dec[i] for i, name in enumerate(OPS)}

    # ── RAM: 32 bytes of latches ──────────────────────────────────────────
    indirect = n.OR(op["LDX"], op["STX"])
    addr = n.mux_word(indirect, k[:5], X[:5])
    rows = n.decoder(addr)
    write = n.AND(n.OR(op["ST"], op["STX"]), n.NOT(clk))
    ram = []
    for r in range(RAM_BYTES):
        we = n.AND(rows[r], write)
        ram.append([n.LATCH(A[b], we) for b in range(8)])
    M = n.select([(rows[r], ram[r]) for r in range(RAM_BYTES)], 8)

    # ── Screen: 64 pixel latches ──────────────────────────────────────────
    px_rows = n.decoder(A[:6])
    plot = n.AND(op["PLOT"], n.NOT(clk))
    pixels = [n.LATCH(A[6], n.AND(px_rows[p], plot)) for p in range(64)]
    test = n.select([(px_rows[p], [pixels[p]]) for p in range(64)], 1)[0]
    for p in range(64):
        n.output(f"PX{p // 8}{p % 8}", pixels[p])

    # ── Buttons: a press is kept till an IN has read it ──────────────────
    # A flip-flop per button, clocked with everything else: pressed on the
    # edge if the button is, or it was and this wasn't an IN (which A takes
    # it with on the same edge). Clocked, it races nothing.
    not_in = n.NOT(op["IN"])
    pressed = []
    for b in btn:
        q = n.sig()
        d = n.DFF(n.OR(b, n.AND(q, not_in)), clk)
        n.alias(q, d)
        pressed.append(d)
    # The direction: the first button pressed (up, down, left, right), a
    # code 0–3 — unless it's the opposite of the direction now (the pairs
    # differ in bit 0 only), then the direction stays.
    up, down, left, right = pressed
    nu = n.NOT(up)
    c0 = n.AND(nu, n.OR(down, n.AND(n.NOT(left), right)))
    c1 = n.AND(nu, n.AND(n.NOT(down), n.OR(left, right)))
    D = [n.sig(), n.sig()]
    back = n.AND(n.NOT(n.XOR(c1, D[1])), n.XOR(c0, D[0]))
    take = n.AND(n.any(pressed), n.NOT(back))
    direction = n.mux_word(take, D, [c0, c1])

    # ── ALU ───────────────────────────────────────────────────────────────
    sub = n.OR(op["SUB"], op["SUBI"])
    imm = n.OR(op["ADDI"], op["SUBI"])
    step = n.OR(op["MINC"], op["MDEC"])
    operand = n.mux_word(imm, M, k)
    # MINC adds 1, MDEC 255 (−1): an operand of their own.
    operand = n.mux_word(step, operand, [n.ONE] + [op["MDEC"]] * 7)
    operand = [n.XOR(b, sub) for b in operand]
    total, carry = n.add(A, operand, sub)
    stepped = [n.MUX(kb, ab, tb) for kb, ab, tb in zip(k, A, total)]

    z = n.NOT(n.any(A))
    arith = n.any([op["ADD"], op["ADDI"], op["SUB"], op["SUBI"]])
    result = n.select([
        (op["LDI"], k),
        (n.OR(op["LD"], op["LDX"]), M),
        (arith, total),
        (step, stepped),
        (op["ANDI"], [n.AND(a, b) for a, b in zip(A, k)]),
        (op["ORI"], [n.OR(a, b) for a, b in zip(A, k)]),
        (op["XORI"], [n.XOR(a, b) for a, b in zip(A, k)]),
        (op["IN"], direction + [n.ZERO] * 6),
        (op["RND"], R),
        (op["TEST"], [test] + [n.ZERO] * 7),
        (op["TXA"], X),
    ], 8)
    load_a = n.any([op[o] for o in ["LDI", "LD", "LDX", "ADD", "ADDI", "SUB", "SUBI", "ANDI", "ORI", "XORI", "IN", "RND", "TEST", "TXA", "MINC", "MDEC"]])

    # ── Registers ─────────────────────────────────────────────────────────
    new = n.register(result, load_a, clk)
    for ph, q in zip(A, new):
        n.alias(ph, q)
    A[:] = new
    new = n.register(A, op["TAX"], clk)
    for ph, q in zip(X, new):
        n.alias(ph, q)
    X[:] = new
    c_new = n.register([carry], arith, clk)[0]
    n.alias(C, c_new)
    # The direction register: an IN keeps what it gave, SETD what A says.
    new_d = n.register(n.mux_word(op["SETD"], direction, A[:2]), n.OR(op["IN"], op["SETD"]), clk)
    for ph, q in zip(D, new_d):
        n.alias(ph, q)

    jump = n.any([op["JMP"], n.AND(op["JZ"], z), n.AND(op["JNZ"], n.NOT(z)), n.AND(op["JC"], c_new)])
    next_pc = n.mux_word(jump, n.inc(PC), k)
    new = n.register(next_pc, n.ONE, clk)
    for ph, q in zip(PC, new):
        n.alias(ph, q)
    PC[:] = new

    # LFSR (x^8 + x^6 + x^5 + x^4 + 1), never all zero: a zero state shifts in a one.
    fb = n.XOR(n.XOR(R[7], R[5]), n.XOR(R[4], R[3]))
    fb = n.OR(fb, n.NOT(n.any(R)))
    new = n.register([fb] + R[:7], n.ONE, clk)
    for ph, q in zip(R, new):
        n.alias(ph, q)

    if debug:
        for i in range(8):
            n.output(f"PC{i}", PC[i])
            n.output(f"A{i}", A[i])
    return n
