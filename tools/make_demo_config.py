#!/usr/bin/env python3
"""写一份 demo 自选配置，供 `make shot` 出 README 示意图用。

用热门股 + 三大指数（指数顺手演示「仅面板」），避免把作者自己的自选暴露在截图里。

    python3 tools/make_demo_config.py /tmp/tickerstrip-shot/config.json
"""

import json
import sys
from pathlib import Path

# (secid, code, name, 是否进状态栏滚动)
DEMO = [
    ("1.000001", "000001", "上证指数", False),
    ("0.399001", "399001", "深证成指", False),
    ("0.399006", "399006", "创业板指", False),
    ("1.600519", "600519", "贵州茅台", True),
    ("0.300750", "300750", "宁德时代", True),
    ("0.002594", "002594", "比亚迪", True),
    ("1.688981", "688981", "中芯国际", True),
    ("116.00700", "00700", "腾讯控股", True),
    ("116.01810", "01810", "小米集团-W", True),
    ("105.AAPL", "AAPL", "苹果", True),
    ("105.NVDA", "NVDA", "英伟达", True),
]


def main() -> None:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/tickerstrip-shot/config.json")
    out.parent.mkdir(parents=True, exist_ok=True)
    cfg = {
        "stocks": [
            {"secid": s, "code": c, "name": n, "in_bar": b} for s, c, n, b in DEMO
        ],
        # 截图里字号调大一点，README 里更清楚
        "panel_font_size": 13.0,
        "bar_font_size": 13.0,
        "panel_width": 320.0,
        "bar_width": 250.0,
    }
    out.write_text(json.dumps(cfg, ensure_ascii=False, indent=2) + "\n")
    print(f"demo 配置已写入 {out}（{len(DEMO)} 只）")


if __name__ == "__main__":
    main()
