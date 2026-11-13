"""Generate the synthetic image seed corpus for the decode fuzz targets.

Synthetic content only (gradients and seeded noise): no faces, no third-party images. Re-run with
the lab venv (Pillow with WebP support):

    lab/.venv/Scripts/python service/fuzz/make_seeds.py
"""

import io
import random
from pathlib import Path

from PIL import Image

OUT = Path(__file__).parent / "seeds" / "image"
rng = random.Random(1234)


def synthetic(w, h, mode="RGB"):
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            n = rng.randint(0, 40)
            px[x, y] = ((x * 255 // max(w - 1, 1) + n) % 256, (y * 255 // max(h - 1, 1)) % 256, (x * y + n) % 256)
    if mode != "RGB":
        img = img.convert(mode)
    return img


def save(name, img, fmt, **kw):
    buf = io.BytesIO()
    img.save(buf, fmt, **kw)
    (OUT / name).write_bytes(buf.getvalue())


OUT.mkdir(parents=True, exist_ok=True)
base = synthetic(48, 40)
for q in (5, 30, 75, 95, 100):
    save(f"jpeg-q{q}.jpg", base, "JPEG", quality=q)
save("jpeg-progressive.jpg", base, "JPEG", quality=80, progressive=True)
save("jpeg-444.jpg", base, "JPEG", quality=80, subsampling=0)
save("jpeg-gray.jpg", base.convert("L"), "JPEG", quality=70)
save("jpeg-cmyk.jpg", base.convert("CMYK"), "JPEG", quality=70)
save("jpeg-optimize-restart.jpg", base, "JPEG", quality=60, optimize=True, restart_marker_blocks=1)
exif = Image.Exif()
exif[0x0112] = 6  # orientation: rotate 90 CW
save("jpeg-exif-orient6.jpg", base, "JPEG", quality=70, exif=exif.tobytes())
save("jpeg-tiny.jpg", synthetic(1, 1), "JPEG", quality=50)

save("png-rgb.png", base, "PNG")
save("png-rgba.png", synthetic(32, 24, "RGBA"), "PNG")
save("png-gray.png", base.convert("L"), "PNG")
save("png-palette.png", base.convert("P"), "PNG")
save("png-1bit.png", base.convert("1"), "PNG")
save("png-gray16.png", base.convert("I;16"), "PNG")
save("png-tiny.png", synthetic(1, 1), "PNG")

for q in (10, 50, 90):
    save(f"webp-lossy-q{q}.webp", base, "WEBP", quality=q)
save("webp-lossless.webp", base, "WEBP", lossless=True)
save("webp-lossy-alpha.webp", synthetic(32, 24, "RGBA"), "WEBP", quality=60)
frames = [synthetic(24, 20), synthetic(24, 20)]
save("webp-animated.webp", frames[0], "WEBP", save_all=True, append_images=frames[1:], duration=100, quality=50)
save("webp-tiny.webp", synthetic(1, 1), "WEBP", quality=50)
print(sorted(p.name for p in OUT.iterdir()))
