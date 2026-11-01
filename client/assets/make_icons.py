"""Иконки трея и приложения: рисуются кодом во всех размерах."""

import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = Path(__file__).parent
FONT = r"C:\Windows\Fonts\bahnschrift.ttf"
SS = 8
TRAY_SIZES = (16, 20, 24, 32, 40, 48, 64)
APP_SIZES = (16, 20, 24, 32, 40, 48, 64, 128, 256)

THEMES = {
    "ru": ((99, 102, 241), (139, 92, 246)),
    "en": ((14, 165, 233), (20, 184, 166)),
    "off": ((107, 114, 128), (75, 85, 99)),
}
SPARK = (251, 191, 36)


def font(px: float, weight: int = 700) -> ImageFont.FreeTypeFont:
    f = ImageFont.truetype(FONT, max(1, int(px)))
    try:
        f.set_variation_by_axes([weight, 75])
    except Exception:
        pass
    return f


def gradient(size: int, c1, c2) -> Image.Image:
    g = Image.new("RGBA", (size, size))
    px = g.load()
    for y in range(size):
        for x in range(size):
            t = (x + y) / (2 * size - 2)
            px[x, y] = tuple(int(a + (b - a) * t) for a, b in zip(c1, c2)) + (255,)
    return g


def rounded_mask(size: int, inset: float, radius: float) -> Image.Image:
    m = Image.new("L", (size, size), 0)
    ImageDraw.Draw(m).rounded_rectangle((inset, inset, size - 1 - inset, size - 1 - inset), radius=radius, fill=255)
    return m


def spark(draw: ImageDraw.ImageDraw, cx: float, cy: float, r: float, color) -> None:
    """Четырёхлучевая звезда."""
    pts = []
    for i in range(8):
        a = math.pi / 4 * i - math.pi / 2
        rr = r if i % 2 == 0 else r * 0.32
        pts.append((cx + rr * math.cos(a), cy + rr * math.sin(a)))
    draw.polygon(pts, fill=color)


def centered_text(draw, box, text, f, fill) -> None:
    l, t, r, b = draw.textbbox((0, 0), text, font=f)
    x0, y0, x1, y1 = box
    draw.text(((x0 + x1 - (r - l)) / 2 - l, (y0 + y1 - (b - t)) / 2 - t), text, font=f, fill=fill)


def tray(theme: str, size: int) -> Image.Image:
    S = size * SS
    c1, c2 = THEMES[theme]
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    inset = S * 0.02
    img.paste(gradient(S, c1, c2), (0, 0), rounded_mask(S, inset, S * 0.24))
    d = ImageDraw.Draw(img)
    white = (255, 255, 255, 255)
    small = size <= 20
    if theme == "off":
        bw, bh = S * 0.13, S * 0.42
        for cx in (S * 0.38, S * 0.62):
            d.rounded_rectangle((cx - bw / 2, (S - bh) / 2, cx + bw / 2, (S + bh) / 2), radius=bw * 0.35, fill=white)
        return img.resize((size, size), Image.LANCZOS)
    f = font(S * (0.64 if small else 0.54), 700)
    box = (inset, inset + S * (0.0 if small else 0.10), S - inset, S - inset)
    centered_text(d, box, theme.upper(), f, white)
    if not small:
        r = S * 0.17
        cx, cy = S - inset - r * 0.95, inset + r * 0.95
        d.ellipse((cx - r * 0.9, cy - r * 0.9, cx + r * 0.9, cy + r * 0.9), fill=c1 + (255,))
        spark(d, cx, cy, r, SPARK + (255,))
    return img.resize((size, size), Image.LANCZOS)


def app(size: int) -> Image.Image:
    S = size * SS
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    inset = S * 0.06
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle((inset, inset * 1.6, S - inset, S - inset * 0.4), radius=S * 0.2, fill=(0, 0, 0, 110))
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(S * 0.025)))
    img.paste(gradient(S, (17, 24, 39), (3, 7, 18)), (0, 0), rounded_mask(S, inset, S * 0.2))
    top = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    m = Image.new("L", (S, S), 0)
    ImageDraw.Draw(m).rounded_rectangle((S * 0.14, S * 0.11, S * 0.86, S * 0.80), radius=S * 0.13, fill=255)
    top.paste(gradient(S, (55, 65, 81), (31, 41, 55)), (0, 0), m)
    img.alpha_composite(top)
    d = ImageDraw.Draw(img)
    f = font(S * 0.36, 700)
    s = S * 0.17
    centered_text(d, (S * 0.36 - s, S * 0.38 - s, S * 0.36 + s, S * 0.38 + s), "A", f, (45, 212, 191, 255))
    centered_text(d, (S * 0.62 - s, S * 0.55 - s, S * 0.62 + s, S * 0.55 + s), "Я", f, (167, 139, 250, 255))
    spark(d, S * 0.71, S * 0.25, S * 0.085, SPARK + (255,))
    return img.resize((size, size), Image.LANCZOS)


def save_ico(path: Path, frames: list[Image.Image]) -> None:
    big = max(frames, key=lambda im: im.size[0])
    big.save(path, format="ICO", sizes=[im.size for im in frames], append_images=[f for f in frames if f is not big])


if __name__ == "__main__":
    for theme in THEMES:
        save_ico(HERE / f"tray_{theme}.ico", [tray(theme, s) for s in TRAY_SIZES])
    save_ico(HERE / "app.ico", [app(s) for s in APP_SIZES])

    # лист предпросмотра: размеры как в трее и крупно
    cell, width = 72, 660
    sheet = Image.new("RGBA", (width, cell * 4 + 300), (0, 0, 0, 0))
    for row, bg in enumerate([(243, 244, 246), (32, 32, 32)]):
        base = Image.new("RGBA", (width, cell * 2), bg + (255,))
        x = 8
        for theme in THEMES:
            for s in (16, 24, 32):
                base.alpha_composite(tray(theme, s), (x, (cell * 2 - s) // 2))
                x += s + 10
            x += 14
        base.alpha_composite(app(32), (x, (cell * 2 - 32) // 2))
        sheet.alpha_composite(base, (0, row * cell * 2))
    big = Image.new("RGBA", (width, 300), (229, 231, 235, 255))
    x = 10
    for theme in THEMES:
        big.alpha_composite(tray(theme, 96), (x, 100))
        x += 116
    big.alpha_composite(app(256).resize((256, 256), Image.LANCZOS), (x + 10, 22))
    sheet.alpha_composite(big, (0, cell * 4))
    sheet.save(HERE / "preview.png")
    print("ok")
