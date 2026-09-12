"""Generate every WinSpaces icon asset from one drawing.

Outputs (all committed; rerun after editing the spec below):
  assets/logo.svg        scalable source, used by README and the website
  crates/winspaces/winspaces.ico
                         exe / installer icon: 16 20 24 32 48 64 256 (inside the
                         crate so cargo publish ships it)
  site/favicon.svg       copy of logo.svg
  site/og-image.png      1200x630 social preview

The drawing: two overlapping spaces, like the Windows Task View icon; the
front one carries a tiled layout (one tall window, two stacked). Sizes up
to 24 px merge the two stacked windows into one, which would only blur at
that scale.

Requires Pillow (pip install pillow).
"""

from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
GRAD = ("#38bdf8", "#818cf8")
WHITE = "#ffffff"

# Every shape lives on a 64x64 canvas: (kind, x, y, w, h, rx, fill, opacity).
BG = ("bg", 0, 0, 64, 64, 14, GRAD, 1)
CARDS = [
    ("rect", 11, 11, 34, 30, 4, WHITE, 0.25),
    ("rect", 19, 21, 34, 30, 4, WHITE, 0.45),
]
WINDOWS_FULL = [
    ("rect", 22, 24, 13, 24, 2, WHITE, 1),
    ("rect", 37, 24, 13, 11, 2, WHITE, 1),
    ("rect", 37, 37, 13, 11, 2, WHITE, 1),
]
WINDOWS_SMALL = [
    ("rect", 22, 24, 13, 24, 2, WHITE, 1),
    ("rect", 37, 24, 13, 24, 2, WHITE, 1),
]
FULL = [BG] + CARDS + WINDOWS_FULL
SMALL = [BG] + CARDS + WINDOWS_SMALL


def to_svg(shapes: list[tuple]) -> str:
    out = [
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">',
        '<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">'
        f'<stop offset="0" stop-color="{GRAD[0]}"/><stop offset="1" stop-color="{GRAD[1]}"/>'
        "</linearGradient></defs>",
    ]
    for kind, x, y, w, h, rx, fill, a in shapes:
        op = "" if a == 1 else f' opacity="{a:g}"'
        if kind == "bg":
            out.append(f'<rect width="64" height="64" rx="{rx}" fill="url(#g)"/>')
        elif kind == "rect":
            out.append(
                f'<rect x="{x:g}" y="{y:g}" width="{w:g}" height="{h:g}" rx="{rx:g}" fill="{fill}"{op}/>'
            )
        elif kind == "circle":
            out.append(f'<circle cx="{x:g}" cy="{y:g}" r="{w:g}" fill="{fill}"{op}/>')
    out.append("</svg>\n")
    return "\n".join(out)


def rgba(color: str, alpha: float = 1) -> tuple[int, int, int, int]:
    c = color.lstrip("#")
    return (int(c[0:2], 16), int(c[2:4], 16), int(c[4:6], 16), round(255 * alpha))


def gradient(size: int) -> Image.Image:
    a, b = rgba(GRAD[0]), rgba(GRAD[1])
    im = Image.new("RGBA", (size, size))
    px = im.load()
    for y in range(size):
        for x in range(size):
            t = (x + y) / (2 * size - 2)
            px[x, y] = tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(4))
    return im


def raster(shapes: list[tuple], size: int, ss: int = 8) -> Image.Image:
    big = 64 * ss
    im = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    for kind, x, y, w, h, rx, fill, a in shapes:
        layer = Image.new("RGBA", (big, big), (0, 0, 0, 0))
        d = ImageDraw.Draw(layer)
        if kind == "bg":
            mask = Image.new("L", (big, big), 0)
            ImageDraw.Draw(mask).rounded_rectangle([0, 0, big - 1, big - 1], rx * ss, fill=255)
            layer.paste(gradient(big), (0, 0), mask)
        elif kind == "rect":
            d.rounded_rectangle(
                [x * ss, y * ss, (x + w) * ss, (y + h) * ss], rx * ss, fill=rgba(fill, a)
            )
        elif kind == "circle":
            r = w * ss
            d.ellipse([x * ss - r, y * ss - r, x * ss + r, y * ss + r], fill=rgba(fill, a))
        im = Image.alpha_composite(im, layer)
    return im.resize((size, size), Image.LANCZOS)


def og_image() -> Image.Image:
    im = Image.new("RGBA", (1200, 630), rgba("#0d1117"))
    im.alpha_composite(raster(FULL, 320), (120, 155))
    d = ImageDraw.Draw(im)
    try:
        title = ImageFont.truetype("segoeuib.ttf", 92)
        sub = ImageFont.truetype("segoeui.ttf", 36)
    except OSError:
        title = sub = ImageFont.load_default()
    d.text((500, 210), "WinSpaces", font=title, fill=rgba("#f0f6fc"))
    d.text((504, 330), "Independent spaces per monitor,", font=sub, fill=rgba("#8b949e"))
    d.text((504, 378), "Overview and tiling for Windows.", font=sub, fill=rgba("#8b949e"))
    return im.convert("RGB")


def main() -> int:
    assets = ROOT / "assets"
    assets.mkdir(exist_ok=True)
    svg = to_svg(FULL)
    (assets / "logo.svg").write_text(svg, encoding="utf-8", newline="\n")
    (ROOT / "site" / "favicon.svg").write_text(svg, encoding="utf-8", newline="\n")

    sizes = [16, 20, 24, 32, 48, 64, 256]
    frames = [raster(SMALL if s <= 24 else FULL, s) for s in sizes]
    frames[-1].save(
        ROOT / "crates" / "winspaces" / "winspaces.ico",
        sizes=[(s, s) for s in sizes],
        append_images=frames[:-1],
    )
    og_image().save(ROOT / "site" / "og-image.png", optimize=True)
    print("wrote assets/logo.svg assets/winspaces.ico site/favicon.svg site/og-image.png")
    return 0


if __name__ == "__main__":
    sys.exit(main())
