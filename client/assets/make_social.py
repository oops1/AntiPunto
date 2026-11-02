"""Social preview 1280×640: текст и плашки поверх фона, сгенерированного FLUX.2 [klein].

python make_social.py <фон.png> → docs/social-preview.png
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parents[2]
W, H = 1280, 640
FONTS = Path("C:/Windows/Fonts")
TEAL, AMBER, TEXT, MUTED = (64, 224, 208), (255, 176, 80), (240, 244, 248), (150, 162, 178)


def font(name: str, size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(str(FONTS / name), size)


def pill(d: ImageDraw.ImageDraw, x: int, y: int, text: str, color, f) -> int:
    w = int(d.textlength(text, font=f)) + 30
    d.rounded_rectangle((x, y, x + w, y + 40), 20, fill=(*color, 28), outline=(*color, 150), width=1)
    d.text((x + 15, y + 20), text, font=f, fill=(*color, 255), anchor="lm")
    return x + w + 12


def card(img: Image.Image, x: int, y: int, typed: str, fixed: str) -> None:
    mono = font("consola.ttf", 30)
    layer = Image.new("RGBA", img.size)
    d = ImageDraw.Draw(layer)
    w = int(d.textlength(f"{typed}  →  {fixed}", font=mono)) + 44
    d.rounded_rectangle((x, y, x + w, y + 60), 14, fill=(8, 14, 24, 215), outline=(*TEAL, 120), width=1)
    d.text((x + 22, y + 30), typed, font=mono, fill=(*MUTED, 255), anchor="lm")
    tx = x + 22 + d.textlength(f"{typed}  ", font=mono)
    d.text((tx, y + 30), "→", font=mono, fill=(*AMBER, 255), anchor="lm")
    d.text((tx + d.textlength("→  ", font=mono), y + 30), fixed, font=mono, fill=(*TEXT, 255), anchor="lm")
    img.alpha_composite(layer)


def main(bg_path: str) -> None:
    img = Image.open(bg_path).convert("RGBA").resize((W, H), Image.LANCZOS)
    # «буквы» FLUX над клавиатурой — абракадабра: размыть в блики
    mask = Image.new("L", (W, H))
    ImageDraw.Draw(mask).ellipse((660, -120, 1420, 330), fill=255)
    img = Image.composite(img.filter(ImageFilter.GaussianBlur(9)), img, mask.filter(ImageFilter.GaussianBlur(40)))
    # затемнение слева под текст
    shade = Image.new("RGBA", (W, H))
    sd = ImageDraw.Draw(shade)
    for x in range(W):
        a = int(max(0.0, 1 - x / 760) ** 1.5 * 170)
        sd.line([(x, 0), (x, H)], fill=(4, 8, 16, a))
    img.alpha_composite(shade)

    card(img, 760, 150, "ghbdtn", "привет")
    card(img, 840, 232, "преокт", "проект")

    layer = Image.new("RGBA", img.size)
    d = ImageDraw.Draw(layer)
    ico = Image.open(ROOT / "client/assets/app.ico")
    ico.size = max(ico.ico.sizes())
    icon = ico.convert("RGBA").resize((84, 84), Image.LANCZOS)
    glow = Image.new("RGBA", img.size)
    glow.alpha_composite(icon, (64, 108))
    layer.alpha_composite(glow.filter(ImageFilter.GaussianBlur(14)))
    layer.alpha_composite(icon, (64, 108))
    d.text((168, 150), "AntiPunto", font=font("segoeuib.ttf", 76), fill=(*TEXT, 255), anchor="lm")
    d.rounded_rectangle((64, 222, 204, 226), 2, fill=(*TEAL, 255))
    d.rounded_rectangle((144, 222, 204, 226), 2, fill=(*AMBER, 255))
    d.text((64, 266), "Раскладка по смыслу фразы", font=font("seguisb.ttf", 32), fill=(*TEXT, 255), anchor="lm")
    d.text((64, 306), "и исправление опечаток для Windows", font=font("seguisb.ttf", 32), fill=(*TEXT, 255), anchor="lm")
    d.text((64, 350), "Локальная нейросеть на NPU или CPU", font=font("segoeui.ttf", 22), fill=(*MUTED, 255), anchor="lm")
    f = font("segoeui.ttf", 19)
    x = pill(d, 64, 396, "RU ↔ EN", TEAL, f)
    x = pill(d, x, 396, "Опечатки", AMBER, f)
    x = pill(d, x, 396, "Intel NPU · CPU", (140, 170, 255), f)
    x = pill(d, 64, 448, "Backspace — отмена", (190, 150, 255), f)
    x = pill(d, x, 448, "Без облака", TEAL, f)
    d.text((64, 580), "github.com/oops1/AntiPunto · MIT", font=font("segoeui.ttf", 20), fill=(*MUTED, 255), anchor="lm")
    img.alpha_composite(layer)

    out = ROOT / "docs" / "social-preview.png"
    out.parent.mkdir(exist_ok=True)
    img.convert("RGB").save(out, optimize=True)
    print(out)


if __name__ == "__main__":
    main(sys.argv[1])
