# YOG-8

An 8-bit computer made of VLSI chips, and the tools that make it.

| File | What it is |
|---|---|
| cpu.py | the computer, gate by gate, and its instruction set |
| hdl.py | the netlist: signals, gates (VLSI's ready-made designs), registers, decoders, adders |
| asm.py | the assembler |
| emu.py | an instruction-level emulator, to check a program quickly |
| gatesim.py | a gate-level simulator, to check the hardware against the emulator |
| gen.py | lays the gates out into netherite ALUs along one bus and writes the chip's circuit |
| build_world.py | builds the computer, its lamp screen and buttons into a save (the game must be closed) |
| snake.asm | Snake |

## The machine

One instruction per clock. Registers: A (accumulator), X (index), PC, the
carry C and an 8-bit LFSR that steps every clock. 32 bytes of RAM, an 8×8
screen and four buttons. A program word is 13 bits: a 5-bit opcode and an
8-bit operand k; the ROM holds up to 256 words.

| Instruction | Does |
|---|---|
| LDI k · LD k · ST k | A = k · A = M[k] · M[k] = A |
| LDX · STX · TAX · TXA | A = M[X] · M[X] = A · X = A · A = X |
| ADD k · ADDI k · SUB k · SUBI k | arithmetic with the carry |
| ANDI k · ORI k · XORI k | bitwise with k |
| MINC k · MDEC k | A ± 1 within the bits of k, the other bits kept |
| JMP k · JZ k · JNZ k · JC k | jumps |
| IN · SETD | read the direction the buttons chose · set it from A |
| RND | A = the random register |
| PLOT · TEST | pixel A[5:0] = A[6] · A = that pixel |

The assembler takes `label:`, `; comments`, `.equ NAME value`, and several
instructions on a line separated by `|`.

## Making it

```bash
python3 gen.py snake.asm yog8.json        # the chip's circuit
python3 build_world.py "<save>" "<yog-data>" [x y z]
```

build_world.py writes the computer next to (x, y, z): the ALU with the
chip, the lamp screen with a Redstone Port behind every lamp, the buttons and
the cables between them, and puts the circuit and the ALU's board into VLSI's
storage.
