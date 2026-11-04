"""Generate the Serial Desk application icon set from the supplied logo art."""

from pathlib import Path

from PIL import Image, ImageDraw, ImageOps

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "assets" / "screwdriver.png"
ICON_DIR = ROOT / "src-tauri" / "icons"
ICON_DIR.mkdir(parents=True, exist_ok=True)

SIZE = 1024


def app_icon() -> Image.Image:
    source = Image.open(SOURCE).convert("RGBA")
    source = ImageOps.fit(source, (SIZE, SIZE), method=Image.Resampling.LANCZOS)

    # A restrained rounded tile keeps the supplied artwork at full size while
    # fitting native Windows icon shapes.
    scale = 4
    mask = Image.new("L", (SIZE * scale, SIZE * scale), 0)
    draw = ImageDraw.Draw(mask)
    radius = int(SIZE * scale * 0.18)
    draw.rounded_rectangle(
        (0, 0, SIZE * scale - 1, SIZE * scale - 1),
        radius=radius,
        fill=255,
    )
    mask = mask.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    source.putalpha(mask)
    return source


def main() -> None:
    source = app_icon()
    source.save(ICON_DIR / "icon.png")

    for size in (32, 128, 256):
        name = "128x128@2x.png" if size == 256 else f"{size}x{size}.png"
        source.resize((size, size), Image.Resampling.LANCZOS).save(ICON_DIR / name)

    source.resize((256, 256), Image.Resampling.LANCZOS).save(
        ICON_DIR / "icon.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )

    print(f"icons written to {ICON_DIR}")


if __name__ == "__main__":
    main()
