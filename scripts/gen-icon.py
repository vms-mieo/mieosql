#!/usr/bin/env python3
"""Generate MieoSQL iOS-style app icon (squircle, Apple blue + white DB cylinder).

NOTE: `icons/icon.png` is managed by `cargo tauri icon` (512px + .icns/.ico
set). Regenerating the 1024px master with this script and running
`cargo tauri icon icons/icon-1024.png` refreshes the whole set.
"""
from PIL import Image, ImageDraw, ImageFilter

S = 2048          # render scale, downscaled to 1024 for AA
OUT = 1024
RADIUS_RATIO = 0.225  # iOS squircle corner radius ~22.5%

TOP = (58, 161, 255)     # #3AA1FF
MID = (0, 122, 255)      # #007AFF
BOT = (0, 82, 204)       # #0052CC

def lerp(a, b, t):
    return tuple(int(x + (y - x) * t) for x, y in zip(a, b))

def bg_color(y01):
    if y01 < 0.45:
        return lerp(TOP, MID, y01 / 0.45)
    return lerp(MID, BOT, (y01 - 0.45) / 0.55)

# --- 1. squircle mask ---
mask = Image.new("L", (S, S), 0)
md = ImageDraw.Draw(mask)
md.rounded_rectangle([0, 0, S - 1, S - 1], radius=int(S * RADIUS_RATIO), fill=255)
mask = mask.filter(ImageFilter.GaussianBlur(2))

# --- 2. background vertical gradient ---
bg = Image.new("RGB", (S, S))
bg_draw_rows = ImageDraw.Draw(bg)
for y in range(S):
    bg_draw_rows.line([(0, y), (S, y)], fill=bg_color(y / (S - 1)))

# subtle top gloss highlight (radial white, alpha fades with distance)
gloss = Image.new("L", (S, S), 0)
gd = ImageDraw.Draw(gloss)
cx, cy, r = S * 0.5, S * 0.22, S * 0.75
gd.ellipse([cx - r, cy - r, cx + r, cy + r], fill=90)
gloss = gloss.filter(ImageFilter.GaussianBlur(S // 8))
white = Image.new("RGB", (S, S), (255, 255, 255))
bg = Image.composite(
    Image.blend(bg, white, 0.16),
    bg,
    gloss.point(lambda v: int(v * 0.55)),
)

bg = bg.convert("RGBA")
bg.putalpha(mask)

# --- 3. database cylinder glyph (white) ---
g = Image.new("RGBA", (S, S), (0, 0, 0, 0))
gd2 = ImageDraw.Draw(g)
cw = int(S * 0.44)          # cylinder width
ch = int(S * 0.54)          # cylinder height
x0 = (S - cw) // 2
x1 = x0 + cw
y0 = (S - ch) // 2 + int(S * 0.01)
y1 = y0 + ch
ry = int(cw * 0.17)         # top-face ellipse half-height

# soft drop shadow
sh = Image.new("RGBA", (S, S), (0, 0, 0, 0))
shd = ImageDraw.Draw(sh)
shd.rectangle([x0, y0 + ry, x1, y1 - ry], fill=(0, 20, 60, 255))
shd.ellipse([x0, y0, x1, y0 + 2 * ry], fill=(0, 20, 60, 255))
shd.ellipse([x0, y1 - 2 * ry, x1, y1], fill=(0, 20, 60, 255))
sh = sh.filter(ImageFilter.GaussianBlur(int(S * 0.022)))
sh = sh.point(lambda v: int(v * 0.32)) if False else sh  # keep alpha, scale below
# scale shadow alpha to 30%
sa = sh.getchannel("A").point(lambda v: int(v * 0.30))
sh.putalpha(sa)
shadow_offset = Image.new("RGBA", (S, S), (0, 0, 0, 0))
shadow_offset.alpha_composite(sh, (0, int(S * 0.018)))

# cylinder body: white with slight vertical shading (white -> #E4EFFF)
body = Image.new("RGBA", (S, S), (0, 0, 0, 0))
bd = ImageDraw.Draw(body)
bd.rectangle([x0, y0 + ry, x1, y1 - ry], fill=(255, 255, 255, 255))
bd.ellipse([x0, y1 - 2 * ry, x1, y1], fill=(228, 239, 255, 255))  # bottom face shading
bd.ellipse([x0, y0, x1, y0 + 2 * ry], fill=(255, 255, 255, 255))  # top face
# bottom shade overlay for depth
bd.ellipse([x0, y1 - 2 * ry, x1, y1], outline=(190, 212, 245, 255), width=max(2, S // 512))
# top rim: subtle blue-gray ring to read as opening
bd.ellipse([x0, y0, x1, y0 + 2 * ry], outline=(191, 214, 248, 255), width=max(3, S // 340))

# punch two divider gaps so the blue background shows through (iOS cutout look)
gap_h = int(S * 0.016)
for gy in (y0 + int(ch * 0.38), y0 + int(ch * 0.60)):
    bd.rectangle([x0, gy, x1, gy + gap_h], fill=(0, 0, 0, 0))
    # erase: use destination-out via alpha manipulation
    pass

# erase gaps with a punch mask
punch = Image.new("L", (S, S), 255)
pd = ImageDraw.Draw(punch)
for gy in (y0 + int(ch * 0.38), y0 + int(ch * 0.60)):
    pd.rectangle([x0, gy, x1, gy + gap_h], fill=0)
ba = body.getchannel("A").point(lambda v: v)
import PIL.ImageChops as Chops
ba = Chops.multiply(ba, punch)
body.putalpha(ba)

g.alpha_composite(shadow_offset)
g.alpha_composite(body)

# --- 4. composite glyph over background (masked to squircle) ---
icon = Image.alpha_composite(bg, g)

# --- 5. iOS edge: inner top highlight + inner stroke for definition ---
edge = Image.new("RGBA", (S, S), (0, 0, 0, 0))
ed = ImageDraw.Draw(edge)
inset = max(2, S // 700)
ed.rounded_rectangle([inset, inset, S - 1 - inset, S - 1 - inset],
                     radius=int(S * RADIUS_RATIO) - inset,
                     outline=(255, 255, 255, 70), width=max(3, S // 500))
edge.putalpha(Chops.multiply(edge.getchannel("A"), mask))
icon = Image.alpha_composite(icon, edge)

# subtle bottom inner darkening for depth
shade = Image.new("RGBA", (S, S), (0, 0, 0, 0))
shd2 = ImageDraw.Draw(shade)
shd2.rounded_rectangle([0, 0, S - 1, S - 1], radius=int(S * RADIUS_RATIO),
                       outline=(0, 30, 90, 46), width=max(4, S // 400))
shade.putalpha(Chops.multiply(shade.getchannel("A"), mask))
icon = Image.alpha_composite(icon, shade)

# --- 6. downscale to 1024 with transparent corners preserved ---
icon = icon.resize((OUT, OUT), Image.LANCZOS)
icon.save("apps/mieosql-desktop/icons/icon.png")
print("wrote apps/mieosql-desktop/icons/icon.png", icon.size)
