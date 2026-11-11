"""onehall: regenerate every icon asset from the raster brand icon.

Source: `onehall-icon-source.png` (1254x1254 RGBA, generated 2026-10-16: deep
teal rounded square, cream hall gate, warm light, transparent margin + soft
shadow). Upstream's pipeline starts from the vector `icon.svg`; ours starts from
this raster, so this script replaces `macos-icon.gen.py` for the brand icon
(the macOS 824/1024 safe-area rule it documents is honoured here the same way;
`tests/macos_icon_geometry.rs` still guards it).

    python3 src-tauri/icons/onehall-icon.gen.py        # needs Pillow + pnpm tauri

What it writes:
- src-tauri/icons/{32x32,64x64,128x128,128x128@2x,Square*,StoreLogo,icon}.png,
  icon.ico  — full-bleed (the body cropped to its solid edge, scaled to 1024)
- src-tauri/icons/icon.icns — the body at 824px centred on a 1024 canvas
- src-tauri/icons/icon.svg, public/icon.svg, public/icon-maskable.svg — SVG
  wrappers embedding the PNG (the web favicon / PWA manifest want SVG paths)
- public/icon-{32x32,128x128,192x192,512x512}.png, icon-maskable-{192,512}.png
  (maskable = teal background, body at 70% for the mask safe zone)
- src-tauri/icons/tray-icon-template.png — macOS menu-bar template (alpha-only
  hall-gate silhouette, 52x44 logical)
"""
import base64
import shutil
import subprocess
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw

ICONS = Path(__file__).resolve().parent
ROOT = ICONS.parents[1]
PUBLIC = ROOT / "public"
SOURCE = ICONS / "onehall-icon-source.png"


def body_square(im: Image.Image) -> Image.Image:
    alpha = im.getchannel("A")
    box = alpha.point(lambda v: 255 if v > 250 else 0).getbbox()
    m = 2
    crop = im.crop((box[0] - m, box[1] - m, box[2] + m, box[3] + m))
    side = max(crop.size)
    sq = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    sq.paste(crop, ((side - crop.width) // 2, (side - crop.height) // 2))
    return sq



def macos_canvas(body: Image.Image) -> Image.Image:
    """Fit alpha coverage to the existing 824px grid, preserving aspect ratio."""
    side = body.width
    body = body.copy()
    # The raster tile contains alpha=253 in its solid fill. Keep the soft
    # low-alpha edge, but make the tile opaque before measuring its coverage;
    # otherwise fitting coverage pushes its opaque edge outside the grid.
    alpha = body.getchannel("A").point(lambda value: 255 if value > 127 else value)
    body.putalpha(alpha)
    width = sum(alpha.crop((0, side // 2, side, side // 2 + 1)).getdata()) / 255
    height = sum(alpha.crop((side // 2, 0, side // 2 + 1, side)).getdata()) / 255
    coverage = (width + height) / 2
    if coverage <= 0:
        raise ValueError("Icon has no body through its center")
    # body_square retains antialiasing and a small transparent margin. Scale
    # the visible body, using the same measurement as macos_icon_geometry.rs.
    rendered_side = round(824 * side / coverage)
    if rendered_side > 1024:
        raise ValueError("Icon body does not fit the macOS canvas")
    inset = (1024 - rendered_side) // 2
    canvas = Image.new("RGBA", (1024, 1024), (0, 0, 0, 0))
    canvas.paste(body.resize((rendered_side, rendered_side), Image.LANCZOS), (inset, inset))
    return canvas


def tauri_icon(png: Path, out: Path) -> None:
    subprocess.run(["pnpm", "tauri", "icon", str(png), "-o", str(out)], cwd=ROOT, check=True)


def wrap_svg(png: Path, out: Path) -> None:
    b = base64.b64encode(png.read_bytes()).decode()
    out.write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" '
        'viewBox="0 0 1024 1024" width="1024" height="1024">'
        "<!-- onehall: raster brand icon (see src-tauri/icons/onehall-icon.gen.py) -->"
        f'<image width="1024" height="1024" xlink:href="data:image/png;base64,{b}"/></svg>\n'
    )


def tray_template(out: Path) -> None:
    w_l, h_l, sc = 52, 44, 4
    w, h = w_l * sc, h_l * sc
    img = Image.new("L", (w, h), 0)
    d = ImageDraw.Draw(img)

    def rrect(box, r):
        x0, y0, x1, y1 = box
        d.rectangle((x0 + r, y0, x1 - r, y1), fill=255)
        d.rectangle((x0, y0 + r, x1, y1 - r), fill=255)
        for cx, cy in ((x0, y0), (x1 - 2 * r, y0), (x0, y1 - 2 * r), (x1 - 2 * r, y1 - 2 * r)):
            d.ellipse((cx, cy, cx + 2 * r, cy + 2 * r), fill=255)

    pad, beam_h, pillar_w, gap = 4 * sc, 9 * sc, 11 * sc, 3 * sc
    rrect((pad, pad, w - pad, pad + beam_h), 3 * sc)
    top, bottom = pad + beam_h + gap, h - pad
    rrect((pad + 4 * sc, top, pad + 4 * sc + pillar_w, bottom), 3 * sc)
    rrect((w - pad - 4 * sc - pillar_w, top, w - pad - 4 * sc, bottom), 3 * sc)
    cx, cy, r = w // 2, (top + bottom) // 2, 4 * sc
    rrect((cx - r, cy - r, cx + r, cy + r), 2 * sc)
    small = img.resize((w_l, h_l), Image.LANCZOS)
    rgba = Image.new("RGBA", (w_l, h_l), (0, 0, 0, 0))
    rgba.putalpha(small)
    rgba.save(out)


def main() -> None:
    src = Image.open(SOURCE).convert("RGBA")
    sq = body_square(src)
    with tempfile.TemporaryDirectory() as tmp_s:
        tmp = Path(tmp_s)
        full = sq.resize((1024, 1024), Image.LANCZOS)
        full.save(tmp / "full.png")
        mac = macos_canvas(sq)
        mac.save(tmp / "mac.png")

        tauri_icon(tmp / "full.png", ICONS)
        for extra in ("android", "ios"):
            shutil.rmtree(ICONS / extra, ignore_errors=True)
        tauri_icon(tmp / "mac.png", tmp / "mac")
        shutil.copyfile(tmp / "mac" / "icon.icns", ICONS / "icon.icns")

        teal = full.getpixel((512, 40))[:3]
        for size in (512, 192, 128, 32):
            full.resize((size, size), Image.LANCZOS).save(PUBLIC / f"icon-{size}x{size}.png")
        for size in (512, 192):
            bg = Image.new("RGBA", (size, size), teal + (255,))
            b = int(size * 0.7)
            bg.alpha_composite(full.resize((b, b), Image.LANCZOS), ((size - b) // 2, (size - b) // 2))
            bg.save(PUBLIC / f"icon-maskable-{size}x{size}.png")
        wrap_svg(PUBLIC / "icon-512x512.png", ICONS / "icon.svg")
        wrap_svg(PUBLIC / "icon-512x512.png", PUBLIC / "icon.svg")
        wrap_svg(PUBLIC / "icon-maskable-512x512.png", PUBLIC / "icon-maskable.svg")
    tray_template(ICONS / "tray-icon-template.png")


if __name__ == "__main__":
    main()
