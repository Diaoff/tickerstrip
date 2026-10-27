"""把一张 1024 以下的图标放大并转成 .icns。

源图（`assets/icon-src.png`）自带圆角和透明边，所以这里**只做缩放**，
不重画、不加底板——避免和原设计的留白/圆角打架。

    python3 tools/make_icon.py            # 默认读 assets/icon-src.png
    python3 tools/make_icon.py --src x.png --out assets/tickerstrip.icns
"""

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

SIZE = 1024


def upscale(src: Image.Image) -> Image.Image:
    """小图放大到 1024。先做一次 lanczos，再轻微锐化，避免糊。"""
    from PIL import ImageFilter

    if src.size == (SIZE, SIZE):
        return src
    big = src.resize((SIZE, SIZE), Image.LANCZOS)
    # 只锐化 RGB，alpha 保持平滑（否则圆角会出现锯齿）
    rgb = big.convert("RGB").filter(ImageFilter.UnsharpMask(radius=2, percent=45, threshold=2))
    out = rgb.convert("RGBA")
    out.putalpha(big.getchannel("A"))
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", type=Path, default=Path("assets/icon-src.png"))
    ap.add_argument("--out", type=Path, default=Path("assets/tickerstrip.icns"))
    args = ap.parse_args()

    if not args.src.exists():
        sys.exit(f"找不到源图：{args.src}")
    src = Image.open(args.src).convert("RGBA")
    master = upscale(src)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    master.save(args.out.with_suffix(".png"))

    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "tickerstrip.iconset"
        iconset.mkdir()
        for pt in (16, 32, 128, 256, 512):
            for mult in (1, 2):
                px = pt * mult
                if px > SIZE:
                    continue
                name = f"icon_{pt}x{pt}{'@2x' if mult == 2 else ''}.png"
                master.resize((px, px), Image.LANCZOS).save(iconset / name)
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(args.out)], check=True)
    print(f"图标已生成：{args.out}  与  {args.out.with_suffix('.png')}  （源图 {src.size}）")


if __name__ == "__main__":
    main()
