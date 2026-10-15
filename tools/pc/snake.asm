; Snake on YOG-8: an 8×8 screen, a pixel is row*8 + col. The body is a
; ring of positions at 16..31 (HP: its head, TP: its tail). The direction
; is the hardware's: IN gives it, the buttons already taken into account.
.equ HEAD 0
.equ FOOD 2
.equ HP 3
.equ TP 4
.equ GROW 5

start:
    LDI 3 | SETD                ; right
    LDI 36 | ST HEAD
    LDI 16 | ST HP | ST TP | TAX
    LD HEAD | STX
    ORI 64 | PLOT
    LDI 2 | ST GROW
    JMP newfood

loop:
    IN | JNZ notup
    LD HEAD | SUBI 8 | ANDI 63 | ST HEAD | JMP moved
notup:
    SUBI 1 | JNZ notdown
    LD HEAD | ADDI 8 | ANDI 63 | ST HEAD | JMP moved
notdown:
    SUBI 1 | JNZ right
    LD HEAD | MDEC 7 | ST HEAD | JMP moved
right:
    LD HEAD | MINC 7 | ST HEAD

moved:
    LD HEAD | SUB FOOD | JNZ notfood
    ; Eaten: the head goes on, the tail stays — one longer, while the
    ; ring of 16 has room.
    LD HEAD | ORI 64 | PLOT
    LD HP | MINC 15 | ST HP | TAX | LD HEAD | STX
    LD HP | SUB TP | ANDI 15 | SUBI 15 | JNZ newfood
    LD TP | TAX | LDX | PLOT
    LD TP | MINC 15 | ST TP
    JMP newfood
notfood:
    LD HEAD | TEST | JNZ dead
    LD HEAD | ORI 64 | PLOT
    LD HP | MINC 15 | ST HP | TAX | LD HEAD | STX
    LD GROW | JZ erase
    SUBI 1 | ST GROW | JMP loop
erase:
    LD TP | TAX | LDX | PLOT
    LD TP | MINC 15 | ST TP
    JMP loop

newfood:
    RND | ANDI 63 | ST FOOD | TEST | JNZ newfood
    LD FOOD | ORI 64 | PLOT
    JMP loop

dead:
    LDI 0 | ST HEAD
clear:
    LD HEAD | PLOT
    LD HEAD | ADDI 1 | ST HEAD | SUBI 64 | JNZ clear
    JMP start
