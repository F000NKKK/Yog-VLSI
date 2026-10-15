; 3D figures on a Display: YOG-8 (built with its Display port) fills the
; GPU's vertex and index buffers with a cube, a pyramid and an octahedron,
; then turns them. Buttons: RIGHT next figure, LEFT faces or wireframe,
; UP next color, DOWN spin speed.

; GPU registers (gpu.rs)
.equ CMD 0
.equ COLOR 1
.equ BG 2
.equ VPTR 7
.equ VDATA 8
.equ IPTR 9
.equ IDATA 10
.equ MODE 11
.equ FIRST 12
.equ COUNT 13
.equ SCALE 17
.equ SPIN_YAW 18
.equ SPIN_PITCH 19

; RAM
.equ FIGURE 0
.equ LOOK 1         ; 2 faces, 1 wireframe
.equ HUE 2
.equ SPEED 3

start:
    LDI 0 | OUT VPTR
    LDI 226 | OUT VDATA
    PUT 226 | PUT 226 | PUT 30 | PUT 226 | PUT 226 | PUT 30 | PUT 30 | PUT 226
    PUT 226 | PUT 30 | PUT 226 | PUT 226 | PUT 226 | PUT 30 | PUT 30 | PUT 226
    PUT 30 | PUT 30 | PUT 30 | PUT 30 | PUT 226 | PUT 30 | PUT 30 | PUT 224
    PUT 230 | PUT 224 | PUT 32 | PUT 230 | PUT 224 | PUT 32 | PUT 230 | PUT 32
    PUT 224 | PUT 230 | PUT 32 | PUT 0 | PUT 38 | PUT 0 | PUT 38 | PUT 0
    PUT 0 | PUT 218 | PUT 0 | PUT 0 | PUT 0 | PUT 38 | PUT 0 | PUT 0
    PUT 218 | PUT 0 | PUT 0 | PUT 0 | PUT 38 | PUT 0 | PUT 0 | PUT 218
    LDI 0 | OUT IPTR
    LDI 4 | OUT IDATA
    PUT 5 | PUT 6 | PUT 4 | PUT 6 | PUT 7 | PUT 1 | PUT 0 | PUT 3
    PUT 1 | PUT 3 | PUT 2 | PUT 5 | PUT 1 | PUT 2 | PUT 5 | PUT 2
    PUT 6 | PUT 0 | PUT 4 | PUT 7 | PUT 0 | PUT 7 | PUT 3 | PUT 7
    PUT 6 | PUT 2 | PUT 7 | PUT 2 | PUT 3 | PUT 0 | PUT 1 | PUT 5
    PUT 0 | PUT 5 | PUT 4 | PUT 11 | PUT 10 | PUT 12 | PUT 10 | PUT 9
    PUT 12 | PUT 9 | PUT 8 | PUT 12 | PUT 8 | PUT 11 | PUT 12 | PUT 8
    PUT 9 | PUT 10 | PUT 8 | PUT 10 | PUT 11 | PUT 13 | PUT 15 | PUT 17
    PUT 13 | PUT 18 | PUT 15 | PUT 13 | PUT 17 | PUT 16 | PUT 13 | PUT 16
    PUT 18 | PUT 14 | PUT 17 | PUT 15 | PUT 14 | PUT 15 | PUT 18 | PUT 14
    PUT 16 | PUT 17 | PUT 14 | PUT 18 | PUT 16
    LDI 1 | OUT BG
    LDI 0 | ST FIGURE
    LDI 2 | ST LOOK
    LDI 244 | ST HUE
    LDI 1 | ST SPEED
    JMP draw

loop:
    IN | JZ loop
    TAX
    ANDI 8 | JNZ right
    TXA | ANDI 4 | JNZ left
    TXA | ANDI 1 | JNZ up
down:                       ; spin speed 0, 1, 2, 3
    LD SPEED | ADDI 1 | ANDI 3 | ST SPEED | JMP draw
up:                         ; another bright color
    LD HUE | ADDI 37 | ORI 146 | ST HUE | JMP draw
left:                       ; faces <-> wireframe
    LD LOOK | XORI 3 | ST LOOK | JMP draw
right:                      ; the next figure
    LD FIGURE | ADDI 1 | ST FIGURE | SUBI 3 | JNZ draw
    ST FIGURE

draw:
    LD FIGURE | JZ cube
    SUBI 1 | JZ pyramid
octahedron:
    LDI 54 | OUT FIRST | LDI 24 | OUT COUNT | JMP show
pyramid:
    LDI 36 | OUT FIRST | LDI 18 | OUT COUNT | JMP show
cube:
    LDI 0 | OUT FIRST | LDI 36 | OUT COUNT
show:
    LD HUE | OUT COLOR
    LD LOOK | OUT MODE
    LD SPEED | OUT SPIN_YAW
    OUT SPIN_PITCH
    LDI 0 | OUT CMD | PUT 1 | PUT 2     ; CLEAR, DRAW, SWAP
    JMP loop
