"""Regenerates res/limitbar.ico: three usage bars on a dark rounded tile.

Run from the repo root: python tools/make_icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

S = 1024
TILE = (30, 31, 36, 255)
TRACK = (255, 255, 255, 46)
CLAUDE = (232, 142, 112, 255)
CODEX = (150, 168, 255, 255)


def master() -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((32, 32, S - 32, S - 32), radius=220, fill=TILE)
    left, right, h = 200, S - 200, 120
    for i, (fill, frac) in enumerate([(CLAUDE, 0.42), (CLAUDE, 0.86), (CODEX, 0.64)]):
        top = 272 + i * 180
        d.rounded_rectangle((left, top, right, top + h), radius=h // 2, fill=TRACK)
        d.rounded_rectangle((left, top, left + (right - left) * frac, top + h), radius=h // 2, fill=fill)
    return img


def main() -> None:
    out = Path(__file__).resolve().parent.parent / "res" / "limitbar.ico"
    sizes = [(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (256, 256)]
    master().save(out, format="ICO", sizes=sizes)
    print(out)


if __name__ == "__main__":
    main()
