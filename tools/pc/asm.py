"""YOG-8 assembler: `label:`, `; comment`, `.equ NAME value`, one
instruction per line or several split by `|`; operands are numbers, labels
or names."""

import re
from cpu import OP


LABELS = {}


def assemble(text):
    names, lines = {}, []
    for raw in text.splitlines():
        line = raw.split(";")[0].strip()
        for part in filter(None, (p.strip() for p in line.split("|"))):
            if m := re.match(r"^\.equ\s+(\w+)\s+(\S+)$", part):
                names[m[1]] = int(m[2], 0)
                continue
            while m := re.match(r"^(\w+):\s*(.*)$", part):
                names[m[1]] = len(lines)
                part = m[2].strip()
            if part:
                lines.append(part)
    words = []
    for i, line in enumerate(lines):
        op, *arg = line.split()
        if op.upper() not in OP:
            raise SyntaxError(f"{i}: {line}: no such instruction")
        k = 0
        if arg:
            a = arg[0]
            k = names[a] if a in names else int(a, 0)
        words.append(OP[op.upper()] << 8 | (k & 0xFF))
    LABELS.clear()
    LABELS.update(names)
    return words
