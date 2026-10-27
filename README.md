# 摸鱼盯盘 · tickerStrip

**上班摸鱼时瞄一眼行情。** macOS 菜单栏小工具（Rust + AppKit）：状态栏**横向滚动**自选行情，
鼠标悬停**展开面板**看全部自选，红涨绿跌。不登录、不上云、不装 Dock 图标、不读你任何网页。

- 中文名 **摸鱼盯盘**（一眼行情，谁也不知道你在看盘）
- 仓库名 / 命令 `tickerstrip`（英文 slug，便于 `cargo install`）

**和同类工具的区别**：菜单栏行情工具已经有好几个，它们大多只接一个数据源，撞上东财限流就整条变灰。
tickerStrip 的行情层是**腾讯优先 + 东财九域名镜像链自动降级**（`push2` → `1./7./21./28./72./82.push2` → `push2delay/2`），
失败域名进 120 秒冷却并切下一个；港股窝轮、期货、期权、板块这些拿不到数据的品种从搜索候选里直接过滤掉，不会加进来一个永远显示「—」的条目。

```
┌ 菜单栏 ────────────────────────── 贵州茅台 1356.72 +0.50%   宁德时代 234.63 ─┐
│                                                                              │
│        ┌──────────────────────────────────┐                                 │
│        │ 更新于 14:46:09 · 腾讯   ·  8 只  │  ← 悬停状态栏文字自动展开        │
│        │ 上证指数      3666.62    -0.84%  │                                 │
│        │ 深证成指     11337.24    -1.71%  │                                 │
│        │ 贵州茅台      1356.72    +0.50%  │                                 │
│        │ …                                │                                 │
│        │ 左键状态栏 = 立即刷新             │                                 │
│        │ 右键状态栏 = 菜单                 │                                 │
│        └──────────────────────────────────┘                                 │
└──────────────────────────────────────────────────────────────────────────────┘
```

## 构建与安装

```bash
# 直接从仓库装（推荐）
cargo install --git https://github.com/Diaoff/tickerstrip

# 或者本地构建
git clone https://github.com/Diaoff/tickerstrip && cd tickerstrip
make install    # release 构建并装到 ~/.local/bin/tickerstrip
```

其它目标：`make build` / `make test` / `make run` / `make debug` / `make snapshot` /
`make autostart`（登录自启，写 LaunchAgent `com.tickerstrip.plist`）/ `make unautostart`。

依赖：Rust 1.80+、Xcode Command Line Tools。无其它运行时依赖，产物是单个二进制（约 2 MB）。

## 用法

```bash
tickerstrip                          # 启动菜单栏（不进 Dock，不抢焦点）
tickerstrip add 600519 茅台 gzmt AAPL      # 加自选：代码 / 名称 / 拼音都行
tickerstrip rm  600519               # 删自选
tickerstrip search 宁德               # 搜代码 / 名称 / 拼音，看有哪些候选
tickerstrip list                     # 看自选
tickerstrip fetch                    # 终端里打一次行情（排查用）
tickerstrip path                     # 打印配置文件路径
```

**添加自选支持三种写法**（名称和拼音靠东财 suggest 接口解析，腾讯 smartbox 兜底）：

| 输入 | 结果 |
|---|---|
| `600519` | 代码直接加入 |
| `茅台` / `贵州茅` | 名称 → `1.600519 贵州茅台` |
| `gzmt` | 拼音首字母 → `1.600519 贵州茅台` |
| `AAPL` | 精确代码优先，不会被拼音抢（→ `105.AAPL` 苹果） |
| `宁德` | 候选不唯一（A 股 + 港股），列出候选让你说清楚 |
| `--panel-only` | 加进来但**只在悬停面板显示**，不进状态栏滚动 |

```bash
tickerstrip add --panel-only 000001     # 指数只在面板看，不占滚动栏
tickerstrip bar 600519                  # 切换「是否进状态栏滚动」
tickerstrip list                        # 每行会标出「滚动栏 / 仅面板」
```

代码写法（`tickerstrip add` 会自己推断市场）：

| 输入 | 含义 | secid |
|---|---|---|
| `600519` | 沪市 | `1.600519` |
| `000001` | 深市（平安银行） | `0.000001` |
| `1.000001` | 上证指数（指数与深市代码会撞，必须显式写前缀） | `1.000001` |
| `00700` | 港股（5 位数字） | `116.00700` |
| `430047` | 北交所（4/8 开头） | `0.430047` |
| `AAPL` | 美股（默认纳斯达克） | `105.AAPL` |
| `sh600519` / `sz000001` / `hk00700` / `usAAPL` / `106.BABA` | 显式指定 | 同上 |

菜单栏交互：

- **悬停**状态栏文字 → 展开行情面板（`hover_delay_ms` 毫秒后）
- 状态栏**按涨跌上色**：名称保持系统色（菜单栏切深浅色时仍可读），只有价格和涨跌幅染上
  红/绿（`bar_color` 可关，颜色取 `up_color` / `down_color`）
- **左键** → 打开**图形化设置窗口**（见下）
- **右键** → 菜单（设置… / 立即刷新 / 重载配置 / 编辑配置文件 / 打开配置目录 / 退出）
- 终端里 `tickerstrip add|rm` 之后**无需重启**，程序每秒看一次配置文件 mtime，自动重载

## 图形化设置

左键点状态栏文字，或右键菜单选「设置…」（⌘,）。**不用碰 JSON**：

```
自选            [贵州茅台 600519  ☑滚动栏  ▲ ▼ ✕]   ← ▲▼ 调顺序，✕ 删除
                [春光集团 301531  ☑滚动栏  ▲ ▼ ✕]
                [胜宏科技 300476  ☐滚动栏  ▲ ▼ ✕]   ← 取消勾选 = 只在悬停面板显示
                [600519 / 00700 / AAPL / sh600519] [添加]

外观与布局       ☑ 状态栏显示价格   ☑ 状态栏显示涨跌幅
                ☑ 横向滚动        ☑ 非交易时段自动降频
                涨跌配色  [红涨绿跌（A 股习惯）▾]   ← 也有绿涨红跌 / 橙涨蓝跌 / 保留自定义
                滚动速度 / 状态栏宽度 / 状态栏字号 / 面板宽度 / 面板字号   ← 全部滑杆

刷新与数据源     行情刷新间隔 / 悬停展开延时 / 移开收起延时 / 面板最多行数
                ☑ 优先用腾讯行情

                [恢复默认]                    [取消] [保存]
```

- 改完点**保存**才生效（「取消」直接丢弃）；配置写回 `config.json`
- 加自选填 **代码 / 名称 / 拼音** 都行：边打边在「候选」下拉里出候选（防抖 350ms），
  选一个点「添加」或直接回车；候选唯一时回车即加入。名称/拼音会解析成标准 secid，
  代码形式则直接加入、不发搜索请求
- 港股窝轮、期货、期权、板块、英股德股这类在行情链路里拿不到的品种会被过滤掉，不会进候选
- 「仅面板」的标的**照常拉行情**，只是不进状态栏滚动 —— 面板里数据是全的
- 「恢复默认」只重置外观与刷新参数，**自选列表保留**
- 「状态栏按涨跌上色」可关掉，退回纯文字
- 每行可勾选「滚动栏」：取消勾选后该标的**只在悬停面板里出现**（名字会变淡），
  状态栏滚动不再带它；全都取消时状态栏显示「盯盘 · 悬停查看自选」占位，不会变空白点不到
- 文本框支持 ⌘C / ⌘V / ⌘A（Accessory 应用默认没有编辑菜单，程序自己装了一个）

## 配置

路径：`~/Library/Application Support/tickerstrip/config.json`（macOS 惯例位置，首次运行自动生成；`tickerstrip path` 可直接打印）

```jsonc
{
  "stocks": [ { "secid": "1.600519", "code": "600519", "name": "贵州茅台" } ],

  "refresh_secs": 5,          // 行情刷新间隔（秒）
  "market_hours_only": true,  // 非交易时段自动降到 12 倍间隔
  "prefer_tencent": true,     // true=先用腾讯（稳），false=先用东财
  "em_hosts": ["https://push2.eastmoney.com", "..."],  // 东财主机链，按序降级
  "ut": "fa5fd1943c7b386f172d6893dbfba10b",

  "marquee": true,            // 是否横向滚动
  "marquee_ms": 130,          // 滚动帧间隔（毫秒）
  "marquee_step": 1,          // 每帧前进几个字符
  "bar_width": 210.0,         // 状态栏宽度（pt）
  "show_price": true,
  "show_pct": true,
  "bar_font_size": 12.0,
  "bar_color": true,          // 状态栏按涨跌上色（红涨绿跌）

  "hover_delay_ms": 100,      // 悬停多久展开
  "hide_delay_ms": 320,       // 移开多久收起
  "panel_width": 302.0,
  "max_rows": 16,
  "panel_font_size": 12.0,

  "up_color": "#ff5c5c",      // 红涨
  "down_color": "#35c46a"     // 绿跌
}
```

## 数据源

| 源 | 端点 | 说明 |
|---|---|---|
| 腾讯（默认首选） | `qt.gtimg.cn/q=sh600519,...` | 一个请求打包全部标的，GBK 编码，A 股 / 北交所 / ETF / 指数 / 港股 / 美股全覆盖，实测最稳 |
| 东财 | `push2.eastmoney.com/api/qt/ulist.np/get` | JSON、字段最全；被限流时按 `em_hosts` 依次切镜像（`1.push2` / `7.push2` / `push2delay` …），刚失败的域名冷却 120 秒 |

**关于东财限流**：东财对单 IP 限流非常激进（直接 RST、不给 `close_notify`，插件里那句「需要手动完成人机验证」就是它）。所以本工具默认先用腾讯，东财只作为兜底 + 主机链降级。如果两边都拿不到数据，状态栏会显示 `拉取行情中…`，面板表头会显示具体失败原因。

请求头带一个常规 Chrome UA —— 部分行情站点会对陌生 UA 直接断连。

## 开机自启

```bash
make autostart          # 写 LaunchAgent 并加载
make unautostart        # 卸载
```

等价于：

```bash
cat > ~/Library/LaunchAgents/com.tickerstrip.plist <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.tickerstrip</string>
  <key>ProgramArguments</key>
  <array><string>/Users/你的用户名/.local/bin/tickerstrip</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/tmp/tickerstrip.err.log</string>
</dict></plist>
EOF
launchctl load ~/Library/LaunchAgents/com.tickerstrip.plist
```

## 排查

```bash
MBQUOTE_DEBUG=1 tickerstrip                     # 打印图标/鼠标/面板坐标与悬停状态
MBQUOTE_DEBUG_MS=250 tickerstrip                # 配合上面，把节奏从 1s 调到 250ms
MBQUOTE_DEBUG_PANEL=1 tickerstrip               # 有数据就展开面板（不靠悬停）
MBQUOTE_DEBUG_SETTINGS=1 tickerstrip            # 启动就打开设置窗口
MBQUOTE_DEBUG_SETTINGS=1 tickerstrip            # 顺便跑一遍「控件↔配置」往返自检
MBQUOTE_DEBUG_SETTINGS=save tickerstrip         # 改两个控件后自动保存（验证落盘链路）
MBQUOTE_DEBUG_SETTINGS=type MBQUOTE_DEBUG_TYPE=茅台 tickerstrip   # 自动打字并添加（验证补全链路）
MBQUOTE_DEBUG_SETTINGS=typeview MBQUOTE_DEBUG_TYPE=宁德 tickerstrip # 只打字不添加，看候选下拉
MBQUOTE_DEBUG=1 tickerstrip                     # 日志里会打印每段状态栏文字读回来的实际颜色
MBQUOTE_DEBUG_SNAPSHOT=/tmp/p.png tickerstrip   # 把面板渲染成 PNG 看排版（不需要录屏权限）
tail -f /tmp/tickerstrip.err.log                # LaunchAgent 下的日志
```

程序化挪鼠标验证悬停（`CGWarpMouseCursorPosition` 不需要辅助功能权限）：

```python
#/tmp/warp.py  —— 入参是 CoreGraphics 全局坐标：左上原点、y 向下
import ctypes, ctypes.util, sys
cg = ctypes.cdll.LoadLibrary(ctypes.util.find_library('CoreGraphics'))
class P(ctypes.Structure):
    _fields_ = [('x', ctypes.c_double), ('y', ctypes.c_double)]
cg.CGWarpMouseCursorPosition(P(float(sys.argv[1]), float(sys.argv[2])))
```

## 实现要点

- 用 `objc2` 直接调 AppKit，没引 Tauri / winit 这类大件，二进制小、启动快
- 状态栏滚动 = 定时器改 `NSStatusItem` 的 `title`；宽度用字符宽度估算（ASCII 0.6em、CJK 1.0em）动态取窗口
- 悬停判定 = 心跳里比对 `NSEvent::mouseLocation()` 与「状态栏按钮屏幕矩形 / 面板矩形」，
  比 `NSTrackingArea` 省事，而且鼠标移进面板内部不会误收起
- 面板 = `NSPanel`（`NonactivatingPanel`，不抢焦点）+ `NSVisualEffectView`（HUD 材质）+ 若干 `NSTextField`
- 抓取在后台线程做，主线程只读 channel（8 秒超时也不会卡住菜单栏）

## 发布到 GitHub

仓库名 `tickerstrip`，中文名「摸鱼盯盘」。

```
Description: 摸鱼盯盘 tickerStrip — macOS 菜单栏 A股/港股/美股行情 · 状态栏滚动 + 悬停展开面板 ·
             Rust 单二进制 · 无需登录不上云 · 腾讯优先 + 东财多源抗限流
Topics:      macos  menubar  rust  stock-quotes  a-share  hongkong-stocks  moyu
             us-stocks  appkit  ticker  no-account  menu-bar-app
```

## 已知限制

- 只做了「看行情」。下单、持仓成本、K 线、预警通知都还没做
- 港股 / 美股的休市判断沿用 A 股交易日历（只看周末），节假日会照常请求但拿到的多半是收盘价
- 面板行数上限 `max_rows`，超出部分状态栏仍会滚动显示
- 状态栏项显示在哪块屏由系统决定（多显示器 + 独立 Space 时，AppKit 会在当前菜单栏所在屏托管按钮，
  且这个「归属屏」会在运行中变化）。所以悬停判定用 `NSTrackingArea` 事件而不是算按钮坐标 ——
  算出来的坐标会随菜单栏归属漂到另一块屏去
- 右键菜单需要辅助功能权限才能用键盘快捷键触发，鼠标点击不受影响
- 设置窗口里没有拖拽排序，用 ▲▼ 按钮调顺序
