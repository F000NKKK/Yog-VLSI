# Design templates

The circuits the workbench can write onto a blank Blueprint (`designs.rs`).
One file each, read top to bottom:

- `name <text>` — what the design is called;
- `port <char> <label> <in|out|bidi>` — the char stands for a port;
- `delay <x> <y> <z> <1-4>` — that repeater's delay (default 1);
- `unlit <x> <y> <z>` — that torch starts off;
- `layer` — a new layer, one above the last (the first is y 0);
- any other line is a row of the layer, north to south (z), its chars
  west to east (x);
- `//` starts a comment line.

| Char | Block |
|---|---|
| `.` | air |
| `#` | stone |
| `-` | redstone wire |
| `T` | redstone torch |
| `^` `>` `v` `<` | wall torch, pointing north / east / south / west |
| `↑` `→` `↓` `←` | repeater, the signal going that way |
| `△` `▷` `▽` `◁` | comparator (subtract), the signal going that way |
