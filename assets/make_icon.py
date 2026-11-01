"""Regenerates icon.png and icon.ico. Run: python assets/make_icon.py (needs Pillow)."""
import math, os
from PIL import Image, ImageDraw, ImageFilter

S = 2048  # draw large, then downsample
here = os.path.dirname(os.path.abspath(__file__))

def lerp(a, b, t): return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))

img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
# background: rounded square with vertical gradient
grad = Image.new("RGBA", (S, S))
gd = ImageDraw.Draw(grad)
for y in range(S):
    gd.line([(0, y), (S, y)], fill=lerp((30, 36, 50), (12, 14, 20), y / S) + (255,))
mask = Image.new("L", (S, S), 0)
ImageDraw.Draw(mask).rounded_rectangle([0, 0, S - 1, S - 1], radius=int(S * 0.22), fill=255)
img.paste(grad, (0, 0), mask)

# spectrum bars: full height, then a sharp cliff, then nothing
d = ImageDraw.Draw(img)
bars = 15
left, right = S * 0.14, S * 0.86
base = S * 0.66
gap = (right - left) / bars
w = gap * 0.62
cliff = 11  # bars from this index on are "missing" (lossy cutoff)
random_h = [0.62, 0.74, 0.58, 0.80, 0.68, 0.86, 0.64, 0.78, 0.60, 0.72, 0.66]
cyan, cyan2 = (56, 189, 248), (120, 220, 255)
amber = (245, 180, 50)
for i in range(bars):
    x = left + i * gap + (gap - w) / 2
    if i < cliff:
        h = random_h[i] * S * 0.46
        top = base - h
        for k in range(int(h)):
            d.line([(x, top + k), (x + w, top + k)], fill=lerp(cyan2, (30, 120, 180), k / h) + (255,))
        d.rounded_rectangle([x, top, x + w, top + w * 0.5], radius=int(w * 0.3), fill=cyan2 + (255,))
    else:
        d.rounded_rectangle([x, base - S * 0.05, x + w, base], radius=int(w * 0.3), fill=(78, 88, 110, 255))

# amber cliff marker
cx = left + cliff * gap - gap * 0.02
d.rounded_rectangle([cx - S * 0.007, S * 0.20, cx + S * 0.007, base + S * 0.03], radius=S * 0.007, fill=amber + (255,))

# check mark badge (bottom-right)
r = S * 0.15
bx, by = S * 0.74, S * 0.82
glow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
ImageDraw.Draw(glow).ellipse([bx - r * 1.25, by - r * 1.25, bx + r * 1.25, by + r * 1.25], fill=(70, 190, 110, 120))
img.alpha_composite(glow.filter(ImageFilter.GaussianBlur(S * 0.02)))
d = ImageDraw.Draw(img)
d.ellipse([bx - r, by - r, bx + r, by + r], fill=(70, 190, 110, 255), outline=(12, 14, 20, 255), width=int(S * 0.012))
pts = [(bx - r * 0.45, by + r * 0.02), (bx - r * 0.1, by + r * 0.38), (bx + r * 0.5, by - r * 0.32)]
d.line(pts, fill=(255, 255, 255, 255), width=int(S * 0.038), joint="curve")
for p in (pts[0], pts[2]):
    e = S * 0.019
    d.ellipse([p[0] - e, p[1] - e, p[0] + e, p[1] + e], fill=(255, 255, 255, 255))

png = img.resize((512, 512), Image.LANCZOS)
png.save(os.path.join(here, "icon.png"))
img.save(os.path.join(here, "icon.ico"), sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
# 1280x640 social preview for GitHub (Settings > Social preview)
sp = Image.new("RGBA", (1280, 640), (15, 17, 22, 255))
sp.alpha_composite(img.resize((420, 420), Image.LANCZOS), (110, 110))
sd = ImageDraw.Draw(sp)
from PIL import ImageFont
try:
    f1 = ImageFont.truetype("segoeuib.ttf", 92); f2 = ImageFont.truetype("segoeui.ttf", 40)
except OSError:
    f1 = f2 = ImageFont.load_default()
sd.text((590, 230), "flac-checker", font=f1, fill=(240, 244, 250, 255))
sd.text((592, 350), "Is your lossless audio really lossless?", font=f2, fill=(138, 147, 166, 255))
sp.convert("RGB").save(os.path.join(here, "social-preview.png"))
