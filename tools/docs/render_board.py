"""Renders the ALU board drawings recorded by `render_boards` (alu.rs) to PNG.

    OUT=dir cargo test --lib render_boards -- --ignored   (in obj)
    python3 render_board.py dir/*.jsonl
"""
import json, sys
from PIL import Image, ImageDraw, ImageFont

SCALE = 3
FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSansCondensed.ttf"

def argb(c):
    return ((c >> 16) & 255, (c >> 8) & 255, c & 255, (c >> 24) & 255)

for path in sys.argv[1:]:
    ops = [json.loads(l) for l in open(path) if l.strip()]
    w = int(max(o["r"][2] for o in ops if "r" in o)) + 1
    h = int(max(o["r"][3] for o in ops if "r" in o)) + 1
    img = Image.new("RGBA", (w * SCALE, h * SCALE), (0, 0, 0, 255))
    for o in ops:
        c = o["r"][4] if "r" in o else o["t"][4]
        solid = (c >> 24) & 255 == 255
        layer = img if solid else Image.new("RGBA", img.size, (0, 0, 0, 0))
        d = ImageDraw.Draw(layer)
        if "r" in o:
            x0, y0, x1, y1, _ = o["r"]
            box = [round(x0 * SCALE), round(y0 * SCALE), max(round(x0 * SCALE), round(x1 * SCALE) - 1), max(round(y0 * SCALE), round(y1 * SCALE) - 1)]
            d.rectangle(box, fill=argb(c))
        else:
            t, x, y, s, _ = o["t"]
            d.text((x * SCALE, y * SCALE - SCALE), t, fill=argb(c), font=ImageFont.truetype(FONT, int(7 * SCALE * s)))
        if not solid:
            img = Image.alpha_composite(img, layer)
    out = path.rsplit(".", 1)[0] + ".png"
    img.convert("RGB").save(out)
    print(out)
