#!/usr/bin/env python3
"""把 App 渲染出来的素材合成一张 README 用的示意图。

素材来自 `MBQUOTE_DEBUG_SHOT=<dir> <程序>`：
  - raw-bar.png    状态栏文字（真实渲染，RGBA 透明底，深浅色文字带涨跌色）
  - raw-panel.png  悬停面板（真实渲染，含 HUD 材质底色）

菜单栏那条底色是合成的——菜单栏本身属于系统，App 画不出自己所在的那条 bar。
所以这张图是「真实控件渲染 + 示意底色」，不是屏幕截图。

用法：
    python3 tools/make_shot.py <素材目录> <输出 png>
"""

import json
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

# 状态栏文字用系统 CJK 字体重绘（App 那份渲染倍率跟着屏幕走，1x 屏上只有 22px 高）
CJK_FONTS = [
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/System/Library/Fonts/Supplemental/Songti.ttc",
]
# 深色菜单栏里 labelColor 的样子
SYSTEM_TEXT = (235, 235, 240)

# macOS 菜单栏高度（pt）；状态栏项的真实高度就是它，用来自动推断渲染倍率
MENUBAR_PT = 22
# 画布左侧留白（pt）：露出一点菜单栏，暗示右边还有别的图标
LEFT_CTX_PT = 80
GAP_PT = 6
MARGIN_BOTTOM_PT = 26
EDGE_PAD_PT = 8
# 输出宽度：App 的渲染倍率会跟系统叠乘（1x/2x/4x 都可能），统一缩放到这个宽度
TARGET_W = 900

BG_TOP = (36, 40, 48)
BG_BOTTOM = (22, 25, 30)
STRIP = (43, 47, 55, 235)
STRIP_LINE = (255, 255, 255, 26)


def load_font(size: int):
    for path in CJK_FONTS:
        if Path(path).exists():
            try:
                return ImageFont.truetype(path, size)
            except OSError:
                continue
    print("警告：没找到中文字体，状态栏文字会用默认字体画")
    return ImageFont.load_default()


def draw_status_item(meta: dict, scale: float) -> Image.Image:
    """按 JSON 里的分段重绘状态栏那一行，返回 bar_width_pt 宽的 RGBA 图。"""
    w_pt, h_pt = meta["width_pt"], meta["height_pt"]
    font = load_font(max(8, round(meta["font_size"] * scale)))
    img = Image.new("RGBA", (round(w_pt * scale), round(h_pt * scale)), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    limit = img.width
    x = 0.0
    for run in meta["runs"]:
        if x >= limit:
            break
        color = tuple(int(run["color"][i : i + 2], 16) for i in (1, 3, 5)) + (255,) \
            if run["color"] else SYSTEM_TEXT + (255,)
        text = run["text"]
        # 超出右边界就截断
        while text and x + draw.textlength(text, font=font) > limit:
            text = text[:-1]
        draw.text((x, img.height / 2), text, font=font, fill=color, anchor="lm")
        x += draw.textlength(text, font=font)
    return img


def compose(bar_path: Path, panel_path: Path, out: Path, meta_path: Path | None = None) -> None:
    panel = Image.open(panel_path).convert("RGBA")

    # 状态栏那一行：优先用 JSON 重绘（清晰），没有就退回 App 的位图
    meta = None
    if meta_path and meta_path.exists():
        meta = json.loads(meta_path.read_text())
    if meta:
        # 面板是 2x 渲染的，状态栏也按同一倍率画，整张图才一致
        panel_scale = panel.height / max(1, meta.get("panel_height_pt", panel.height / 2))
        scale = 2.0 if panel_scale >= 1.5 else 1.0
        bar = draw_status_item(meta, scale)
    else:
        bar = Image.open(bar_path).convert("RGBA")

    # 渲染倍率：App 在不同屏幕上可能给 1x 或 2x，按状态栏项高度反推
    scale = max(1, round(bar.height / MENUBAR_PT))

    def px(pt: float) -> int:
        return round(pt * scale)

    # 面板圆角：App 里是 layer.cornerRadius = 10，位图渲染不一定带上
    radius = px(10)
    mask = Image.new("L", panel.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, panel.width - 1, panel.height - 1], radius=radius, fill=255
    )
    panel.putalpha(mask)

    # HUD 材质渲染出来是中性灰（缺了背后的壁纸），真实观感在深色下更暗。
    # 用一条平滑曲线把底色压暗、浅色文字保留：
    #   v < 140        底色 → 压到 42%
    #   140..200       抗锯齿边缘 → 平滑过渡，避免出现亮边光晕
    #   v > 200        文字 → 原样保留
    def tone(v: int) -> int:
        if v <= 140:
            return int(v * 0.42)
        if v >= 200:
            return v
        t = (v - 140) / 60.0
        return int(v * (0.42 + 0.58 * t))

    lut = [tone(v) for v in range(256)]
    r, g, b, a = panel.split()
    panel = Image.merge("RGBA", (r.point(lut), g.point(lut), b.point(lut), a))

    strip_h = bar.height  # 状态栏项占满菜单栏高度
    gap = px(GAP_PT)
    edge = px(EDGE_PAD_PT)
    canvas_w = panel.width + px(LEFT_CTX_PT) + edge
    canvas_h = strip_h + gap + panel.height + px(MARGIN_BOTTOM_PT)
    canvas = Image.new("RGBA", (canvas_w, canvas_h), (0, 0, 0, 255))

    # 背景：从深到更深的竖直渐变，给面板一点对比
    grad = Image.new("RGBA", (1, canvas_h))
    gd = ImageDraw.Draw(grad)
    for y in range(canvas_h):
        t = y / max(1, canvas_h - 1)
        gd.point((0, y), tuple(
            round(BG_TOP[i] + (BG_BOTTOM[i] - BG_TOP[i]) * t) for i in range(3)
        ) + (255,))
    canvas.alpha_composite(grad.resize((canvas_w, canvas_h)))

    draw = ImageDraw.Draw(canvas, "RGBA")

    # 菜单栏条 + 下沿发丝线
    draw.rectangle([0, 0, canvas_w, strip_h - 1], fill=STRIP)
    draw.line([0, strip_h - 1, canvas_w, strip_h - 1], fill=STRIP_LINE)

    # 状态栏项：右对齐；它在真实菜单栏里也是这个位置
    bar_x = canvas_w - edge - bar.width
    canvas.alpha_composite(bar, (bar_x, (strip_h - bar.height) // 2))

    # 面板：以状态栏项中心为锚点，右侧越界时贴边（和 App 里的定位逻辑一致）
    icon_center = bar_x + bar.width / 2
    panel_x = min(icon_center - panel.width / 2, canvas_w - edge - panel.width)
    panel_x = max(edge, panel_x)
    panel_y = strip_h + gap

    # 投影
    shadow = Image.new("RGBA", (canvas_w, canvas_h), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shadow)
    sd.rounded_rectangle(
        [panel_x + px(1), panel_y + px(4), panel_x + panel.width, panel_y + panel.height + px(4)],
        radius=radius,
        fill=(0, 0, 0, 150),
    )
    canvas.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(px(9))))

    # 面板本身（真实渲染）盖在投影上
    canvas.alpha_composite(panel, (int(panel_x), panel_y))

    # 统一宽度：太大就缩（缩小是锐的），太小才放大（1x 屏上只能这样）
    if canvas.width != TARGET_W:
        h = round(canvas.height * TARGET_W / canvas.width)
        canvas = canvas.resize((TARGET_W, h), Image.LANCZOS)
        if canvas.width < TARGET_W:
            print(f"  注意：源素材只有 {canvas.width}px，放大会略软")

    out.parent.mkdir(parents=True, exist_ok=True)
    canvas.convert("RGB").save(out, "PNG", optimize=True)
    print(f"已生成 {out}  ({canvas.width}x{canvas.height})")


def main() -> None:
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(2)
    src, dst = Path(sys.argv[1]), Path(sys.argv[2])
    bar, panel = src / "raw-bar.png", src / "raw-panel.png"
    if not panel.exists():
        print(f"缺少素材 {panel}（先跑 make shot）")
        sys.exit(1)
    if not bar.exists() and not (src / "raw-bar.json").exists():
        print("缺少状态栏素材（raw-bar.png 或 raw-bar.json）")
        sys.exit(1)
    compose(bar, panel, dst, src / "raw-bar.json")


if __name__ == "__main__":
    main()
