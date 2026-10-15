; Snake on YOG-8: an 8×8 screen, a pixel is row*8 + col. The body is a
; ring of positions at 16..31 (HP: its head, TP: its tail).
.equ HEAD 0
.equ DIR 1      ; 0 up, 1 down, 2 left, 3 right
.equ FOOD 2
.equ HP 3
.equ TP 4
.equ GROW 5
.equ T 6

start:
    LDI 36 | ST HEAD
    LDI 3 | ST DIR
    LDI 16 | ST HP | ST TP | TAX
    LD HEAD | STX
    ORI 64 | PLOT
    LDI 2 | ST GROW
    JMP newfood

loop:
    ; Buttons: a new direction, never straight back.
    IN | ANDI 1 | JZ n1
    LD DIR | SUBI 1 | JZ move
    LDI 0 | ST DIR | JMP move
n1: IN | ANDI 2 | JZ n2
    LD DIR | JZ move
    LDI 1 | ST DIR | JMP move
n2: IN | ANDI 4 | JZ n3
    LD DIR | SUBI 3 | JZ move
    LDI 2 | ST DIR | JMP move
n3: IN | ANDI 8 | JZ move
    LD DIR | SUBI 2 | JZ move
    LDI 3 | ST DIR

move:
    LD DIR | JNZ notup
    LD HEAD | SUBI 8 | ANDI 63 | ST HEAD | JMP moved
notup:
    SUBI 1 | JNZ notdown
    LD HEAD | ADDI 8 | ANDI 63 | ST HEAD | JMP moved
notdown:
    LD HEAD | ANDI 56 | ST T
    LD DIR | SUBI 2 | JNZ right
    LD HEAD | SUBI 1 | ANDI 7 | ADD T | ST HEAD | JMP moved
right:
    LD HEAD | ADDI 1 | ANDI 7 | ADD T | ST HEAD

moved:
    LD HEAD | SUB FOOD | JNZ notfood
    LD GROW | ADDI 1 | ST GROW
    LD HEAD | ORI 64 | PLOT
    LD HP | ADDI 1 | ANDI 15 | ORI 16 | ST HP | TAX | LD HEAD | STX
    JMP newfood
notfood:
    LD HEAD | TEST | JNZ dead
    LD HEAD | ORI 64 | PLOT
    LD HP | ADDI 1 | ANDI 15 | ORI 16 | ST HP | TAX | LD HEAD | STX
    LD GROW | JZ erase
    SUBI 1 | ST GROW | JMP loop
erase:
    LD TP | TAX | LDX | PLOT
    LD TP | ADDI 1 | ANDI 15 | ORI 16 | ST TP
    JMP loop

newfood:
    RND | ANDI 63 | ST FOOD | TEST | JNZ newfood
    LD FOOD | ORI 64 | PLOT
    JMP loop

dead:
    LDI 0 | ST T
clear:
    LD T | PLOT
    LD T | ADDI 1 | ST T | SUBI 64 | JNZ clear
    JMP start
