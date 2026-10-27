//! 菜单栏 App：状态栏滚动行情 + 悬停展开自选面板。
//!
//! 全部用 AppKit 原生控件拼装（没有自定义 `NSView` 子类）：
//! - 状态栏文字：`NSStatusItem` 的 `title`，由心跳定时器逐帧滚动
//! - 悬停判定：心跳里读 `NSEvent::mouseLocation()` 与图标/面板矩形比对
//!   （比 `NSTrackingArea` 省事，且鼠标移进面板内部也不会误收起）
//! - 面板：`NSPanel`（NonactivatingPanel）+ `NSVisualEffectView` + 若干 `NSTextField`

use std::cell::RefCell;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBitmapImageFileType,
    NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSButton, NSColor, NSControl,
    NSControlStateValueOn, NSEvent, NSEventMask, NSEventType, NSFont, NSFontWeightRegular,
    NSForegroundColorAttributeName, NSImage, NSMenu, NSMenuItem, NSPanel, NSScreen, NSStatusBar,
    NSStatusBarButton, NSStatusItem, NSStatusWindowLevel, NSTextAlignment, NSTextField,
    NSTrackingArea, NSTrackingAreaOptions, NSVariableStatusItemLength, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindowStyleMask,
};
use objc2_foundation::{
    NSDictionary, NSMutableAttributedString, NSPoint, NSRange, NSRect, NSSize, NSString, NSTimer,
};

use crate::config::{self, APP_NAME, Config, DISPLAY_NAME, Stock};
use crate::quote::{self, Client, Quote};
use crate::secid;
use crate::settings::{self, Settings};

/// 心跳间隔：滚动够顺滑，又不至于太耗电
const TICK: f64 = 0.13;
/// 配置文件 mtime 的检查间隔
const CONFIG_POLL: Duration = Duration::from_millis(1000);
/// 抓取卡死后的兜底重试
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// 悬停展开时，数据超过这个年龄就顺手刷一次（防连点刷爆接口）
const HOVER_STALE: Duration = Duration::from_millis(1500);
/// 候选搜索防抖：输入停顿这么久才发请求
const SUGGEST_DEBOUNCE: Duration = Duration::from_millis(350);

const PAD: f64 = 10.0;
const HEADER_H: f64 = 22.0;
const FOOTER_LINES: usize = 3;
const FOOTER_LINE_H: f64 = 14.0;
const FOOTER_H: f64 = FOOTER_LINES as f64 * FOOTER_LINE_H + 6.0;
const NAME_W: f64 = 116.0;
const PRICE_W: f64 = 88.0;
const PCT_W: f64 = 66.0;

/// `MBQUOTE_DEBUG=1` 时每秒把图标/鼠标坐标打到日志，便于核对悬停几何
fn debug_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MBQUOTE_DEBUG").is_some())
}

/// `MBQUOTE_DEBUG_PANEL=1` 时不等悬停、有数据就展开（用于截图核对样式）
fn debug_force_panel() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MBQUOTE_DEBUG_PANEL").is_some())
}

/// 调试输出节奏（`MBQUOTE_DEBUG_MS`，毫秒，默认 1000）
fn debug_cadence() -> Duration {
    static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    Duration::from_millis(*MS.get_or_init(|| {
        std::env::var("MBQUOTE_DEBUG_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000)
    }))
}

/// `MBQUOTE_DEBUG_SHOT=<目录>`：等行情到位后渲染 README 用的素材
/// （`raw-bar.png` = 状态栏文字，`raw-panel.png` = 悬停面板）
fn debug_shot_dir() -> Option<String> {
    std::env::var("MBQUOTE_DEBUG_SHOT")
        .ok()
        .filter(|s| !s.is_empty())
}

/// `MBQUOTE_DEBUG_SETTINGS=1` 时启动就打开设置窗口（免去手点，便于核对排版）
fn debug_settings() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MBQUOTE_DEBUG_SETTINGS").is_some())
}

/// `MBQUOTE_DEBUG_SNAPSHOT=/tmp/x.png` 时把面板内容渲染成 PNG（不依赖录屏权限）
fn debug_snapshot_path() -> Option<String> {
    std::env::var("MBQUOTE_DEBUG_SNAPSHOT").ok()
}

/// 状态栏里一段文字的上色规则
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarStyle {
    /// 跟随系统（菜单栏深浅色自适应，别硬指定颜色）
    Plain,
    Up,
    Down,
}

/// 拼状态栏滚动内容。抽成纯函数，方便单测「谁进滚动、谁上什么色」。
pub fn build_bar_segments(quotes: &[Quote], cfg: &Config) -> Vec<(String, BarStyle)> {
    // 只有 in_bar 的标的进滚动栏；标了「仅面板」的照常拉行情，只是不在这里出现
    let in_bar: std::collections::HashSet<&str> = cfg
        .stocks
        .iter()
        .filter(|s| s.in_bar)
        .map(|s| s.secid.as_str())
        .collect();

    let mut out: Vec<(String, BarStyle)> = Vec::new();
    for q in quotes.iter() {
        if !in_bar.contains(q.secid.as_str()) {
            continue;
        }
        // 名称保持系统色，只给价格和涨跌幅上色
        out.push((q.display_name(), BarStyle::Plain));
        let dir = match q.rising() {
            Some(true) => BarStyle::Up,
            Some(false) => BarStyle::Down,
            None => BarStyle::Plain,
        };
        if cfg.show_price
            && let Some(p) = q.price
        {
            out.push((format!(" {}", quote::fmt_price(p)), dir));
        }
        if cfg.show_pct
            && let Some(p) = q.pct
        {
            out.push((format!(" {}", quote::fmt_pct(p)), dir));
        }
        out.push(("   ".to_string(), BarStyle::Plain));
    }

    // 全都标成「仅面板」时留个可点的入口，别让状态栏变空白
    if out.is_empty() {
        out.push(("盯盘 · 悬停查看自选".to_string(), BarStyle::Plain));
    }
    out
}

/// 把逐字符的样式合并成连续段，减少属性设置次数
fn merge_runs(chars: &[(char, BarStyle)]) -> Vec<(String, BarStyle)> {
    let mut out: Vec<(String, BarStyle)> = Vec::new();
    for (c, style) in chars {
        match out.last_mut() {
            Some((text, last)) if last == style => text.push(*c),
            _ => out.push((c.to_string(), *style)),
        }
    }
    out
}

/// `Retained<T>` 的指针本身就是对象指针，转成 `&AnyObject` 给 AppKit
fn as_any<T: objc2::Message>(obj: &Retained<T>) -> &AnyObject {
    unsafe { &*(Retained::as_ptr(obj) as *const AnyObject) }
}

fn style_color(style: BarStyle, cfg: &Config) -> Option<Retained<NSColor>> {
    match style {
        BarStyle::Plain => None,
        BarStyle::Up => Some(srgb(&cfg.up_color)),
        BarStyle::Down => Some(srgb(&cfg.down_color)),
    }
}

/// 状态栏用 attributedTitle 分段上色。
/// 只有涨跌段设 `NSForegroundColorAttributeName`，名称等保持系统色，
/// 这样菜单栏切深浅色时默认部分仍然可读。
fn set_attributed_title(button: &NSStatusBarButton, runs: &[(String, BarStyle)], cfg: &Config) {
    let plain: String = runs.iter().map(|(t, _)| t.as_str()).collect();
    // NSMutableAttributedString 是 AnyThread 类，alloc 不需要 MainThreadMarker
    let attr =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &ns(&plain));
    let mut loc = 0usize;
    for (text, style) in runs {
        // NSRange 的坐标是 UTF-16 码元，不能用 chars().count()
        let len = text.encode_utf16().count();
        if len == 0 {
            continue;
        }
        if let Some(color) = style_color(*style, cfg) {
            unsafe {
                attr.addAttribute_value_range(
                    NSForegroundColorAttributeName,
                    as_any(&color),
                    NSRange::new(loc, len),
                );
            }
        }
        loc += len;
    }
    button.setAttributedTitle(&attr);
}

/// 只有「确定是代码」的输入才走快路径；纯字母可能是拼音首字母，交给搜索判断
fn code_fast_path(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    secid::infer(t).ok()
}

/// 所有回调都从 AppKit 的主线程 run loop 来，这里直接拿标记。
fn mtm() -> MainThreadMarker {
    unsafe { MainThreadMarker::new_unchecked() }
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn mins(now: Instant, earlier: Instant) -> Duration {
    now.saturating_duration_since(earlier)
}

pub fn run() -> ! {
    let app = NSApplication::sharedApplication(mtm());
    // 只做菜单栏程序：不进 Dock、不抢焦点
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let cfg = Config::load();
    let (tx, rx) = mpsc::channel::<Result<quote::Outcome, String>>();
    let (name_tx, name_rx) = mpsc::channel::<(String, String)>();
    let (cand_tx, cand_rx) = mpsc::channel::<(String, Vec<quote::Candidate>)>();

    // ---- 状态栏 ----
    let status_bar = NSStatusBar::systemStatusBar();
    let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
    let button = status_item.button(mtm()).expect("拿不到状态栏按钮");
    button.setTitle(&ns("盯盘…"));
    button.setToolTip(Some(&*ns(&format!(
        "{DISPLAY_NAME} · 悬停展开自选，左键打开设置，右键菜单"
    ))));
    status_item.setLength(cfg.bar_width);

    // ---- 面板 ----
    let (panel, effect) = build_panel(&cfg);

    // ---- 菜单 ----
    let menu = build_menu();

    let ivars = Ivars {
        status_item,
        button,
        panel,
        effect: effect.clone(),
        rows: RefCell::new(Vec::new()),
        header: RefCell::new(None),
        footers: RefCell::new(Vec::new()),
        panel_size: RefCell::new(NSSize::new(cfg.panel_width, 120.0)),
        menu,
        client: Client::new(),
        tx,
        rx: RefCell::new(rx),
        name_tx,
        name_rx: RefCell::new(name_rx),
        cand_tx,
        cand_rx: RefCell::new(cand_rx),
        settings: RefCell::new(None),
        settings_selftested: std::cell::Cell::new(false),
        debug_ticks: std::cell::Cell::new(0),
        shot_done: std::cell::Cell::new(false),
        state: RefCell::new(State::new(cfg)),
    };

    let ctrl = Controller::new(ivars);

    // 菜单项指向控制器
    for item in ctrl.ivars().menu.itemArray().iter() {
        unsafe { set_target(&item, &ctrl) };
    }

    // 悬停：给状态栏按钮挂一个 TrackingArea，事件直接回调到控制器
    // （多显示器时每块屏幕各有一个按钮实例，事件驱动天然覆盖全部）
    unsafe {
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect;
        let area = NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            ctrl.ivars().button.bounds(),
            options,
            Some(&ctrl),
            None,
        );
        ctrl.ivars().button.addTrackingArea(&area);
    }

    // 状态栏按钮：左右键都走 statusItemClicked:
    unsafe {
        let _: () = msg_send![&*ctrl.ivars().button, setTarget: &*ctrl];
        let _: () = msg_send![&*ctrl.ivars().button, setAction: sel!(statusItemClicked:)];
        let mask = (NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp).0;
        let _: isize = msg_send![&*ctrl.ivars().button, sendActionOn: mask];
    }

    // 心跳：NSTimer 会 retain target，控制器不会被回收
    let _timer: Retained<NSTimer> = unsafe {
        msg_send![
            objc2::class!(NSTimer),
            scheduledTimerWithTimeInterval: TICK,
            target: &*ctrl,
            selector: sel!(tick:),
            userInfo: std::ptr::null::<AnyObject>(),
            repeats: true
        ]
    };

    // 让设置窗口里的文本框能用 ⌘C / ⌘V / ⌘A
    settings::install_edit_menu();

    ctrl.render_if_needed();
    ctrl.start_fetch(true);

    app.run();
    std::process::exit(0)
}

/// `setTarget:` 取 `&AnyObject`，用 `msg_send!` 比类型转换清爽
unsafe fn set_target(item: &NSMenuItem, target: &Controller) {
    let _: () = unsafe { msg_send![item, setTarget: target] };
}

// ---------------------------------------------------------------- 状态

struct State {
    cfg: Config,
    quotes: Vec<Quote>,
    /// 最近一次命中的数据源
    source: String,
    /// 滚动文本按字符拆开（带样式，供状态栏分段上色）
    bar: Vec<(char, BarStyle)>,
    /// 上一次真正写进状态栏的文字（避免重复设置 attributedTitle）
    last_bar_text: String,
    offset: usize,
    last_fetch: Instant,
    fetching: bool,
    fetch_started: Instant,
    /// 鼠标是否停在状态栏图标上（由 NSTrackingArea 事件驱动，比算矩形可靠：
    /// 多显示器下每个屏幕都有各自的菜单栏实例，坐标换算会漂）
    over_icon: bool,
    /// 鼠标刚移入图标的时刻（用于 hover 延时）
    hover_since: Option<Instant>,
    /// 展开时记住锚点 x，避免数据刷新时面板跟着鼠标抖
    anchor_x: f64,
    /// 最近一次鼠标还在图标/面板上的时刻（用于收起延时）
    last_over: Instant,
    visible: bool,
    /// 面板顶部提示（更新时间 / 错误）
    header: String,
    last_mtime_check: Instant,
    cfg_mtime: Option<SystemTime>,
    /// 渲染去重：内容没变就不动 AppKit
    render_key: String,
    // ---- 候选搜索状态机（输入名称 / 拼音时用）----
    /// 上次看到的输入框内容
    search_input: String,
    /// 该内容最后一次变化的时刻（防抖）
    search_at: Instant,
    /// 已经搜过的关键词
    search_query: String,
    searching: bool,
    /// 这次搜索是「点添加/回车」触发的，出结果就直接选中
    add_after_search: bool,
    /// 学到新名称、待落盘
    names_dirty: bool,
    /// 上次落盘时间（名称回写做节流）
    last_save: Instant,
}

impl State {
    fn new(cfg: Config) -> Self {
        let now = Instant::now();
        Self {
            cfg,
            quotes: Vec::new(),
            source: String::new(),
            bar: Vec::new(),
            last_bar_text: String::new(),
            offset: 0,
            last_fetch: now - Duration::from_secs(3600),
            fetching: false,
            fetch_started: now,
            over_icon: false,
            hover_since: None,
            anchor_x: 0.0,
            last_over: now - Duration::from_secs(3600),
            visible: false,
            header: "加载中…".into(),
            last_mtime_check: now,
            cfg_mtime: std::fs::metadata(config::config_path())
                .and_then(|m| m.modified())
                .ok(),
            render_key: String::new(),
            search_input: String::new(),
            search_at: now,
            search_query: String::new(),
            searching: false,
            add_after_search: false,
            names_dirty: false,
            last_save: now,
        }
    }
}

struct Row {
    name: Retained<NSTextField>,
    price: Retained<NSTextField>,
    pct: Retained<NSTextField>,
}

struct Ivars {
    status_item: Retained<NSStatusItem>,
    button: Retained<NSStatusBarButton>,
    panel: Retained<NSPanel>,
    effect: Retained<NSVisualEffectView>,
    rows: RefCell<Vec<Row>>,
    header: RefCell<Option<Retained<NSTextField>>>,
    footers: RefCell<Vec<Retained<NSTextField>>>,
    panel_size: RefCell<NSSize>,
    menu: Retained<NSMenu>,
    client: Client,
    tx: Sender<Result<quote::Outcome, String>>,
    rx: RefCell<Receiver<Result<quote::Outcome, String>>>,
    /// 设置窗口里「添加自选」的名称回填通道
    name_tx: Sender<(String, String)>,
    name_rx: RefCell<Receiver<(String, String)>>,
    /// 候选搜索通道：(关键词, 候选列表)
    cand_tx: Sender<(String, Vec<quote::Candidate>)>,
    cand_rx: RefCell<Receiver<(String, Vec<quote::Candidate>)>>,
    /// 设置窗口（首次打开时构建，之后复用）
    settings: RefCell<Option<Settings>>,
    /// 设置窗口自检只跑一次
    settings_selftested: std::cell::Cell<bool>,
    /// 调试：模拟输入的打点计数
    debug_ticks: std::cell::Cell<u32>,
    /// 调试：截图素材是否已产出
    shot_done: std::cell::Cell<bool>,
    state: RefCell<State>,
}

// ---------------------------------------------------------------- 控制器

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MBQuoteController"]
    #[ivars = Ivars]
    struct Controller;

    impl Controller {
        /// 心跳
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            self.on_tick();
        }

        /// 状态栏按钮被点击（左右键都走这里）
        #[unsafe(method(statusItemClicked:))]
        fn status_item_clicked(&self, _sender: &AnyObject) {
            let is_right = NSApplication::sharedApplication(mtm())
                .currentEvent()
                .map(|e| {
                    let t: NSEventType = e.r#type();
                    t == NSEventType::RightMouseUp || t == NSEventType::RightMouseDown
                })
                .unwrap_or(false);
            if is_right {
                // 弹出的是"重载配置 / 退出"这类操作菜单
                #[allow(deprecated)]
                self.ivars().status_item.popUpStatusItemMenu(&self.ivars().menu);
            } else {
                // 左键 = 打开设置窗口（行情刷新走右键菜单 / 悬停时自动补）
                self.open_settings();
            }
        }

        /// 鼠标进入状态栏图标（TrackingArea 事件）
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            let mut st = self.ivars().state.borrow_mut();
            st.over_icon = true;
            if st.hover_since.is_none() {
                st.hover_since = Some(Instant::now());
            }
        }

        /// 鼠标离开状态栏图标
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            let mut st = self.ivars().state.borrow_mut();
            st.over_icon = false;
        }

        #[unsafe(method(doRefresh:))]
        fn do_refresh(&self, _sender: &AnyObject) {
            self.refresh_now();
        }

        /// 打开设置窗口（菜单 / 左键状态栏）
        #[unsafe(method(openSettings:))]
        fn open_settings_action(&self, _sender: &AnyObject) {
            self.open_settings();
        }

        /// 设置窗口里所有滑块 / 勾选框统一走这里，靠 tag 区分
        #[unsafe(method(settingsSliderChanged:))]
        fn settings_slider_changed(&self, sender: &NSControl) {
            if let Some(win) = self.ivars().settings.borrow().as_ref() {
                win.slider_changed(sender);
            }
        }

        /// 自选列表的行内按钮：tag = index * 10 + op
        #[unsafe(method(settingsStockAction:))]
        fn settings_stock_action(&self, sender: &NSButton) {
            let tag = sender.tag();
            let slot = self.ivars().settings.borrow();
            if let Some(win) = slot.as_ref() {
                let checked = sender.state() == NSControlStateValueOn;
                win.stock_action(self, tag, checked);
            }
        }

        /// 添加自选：立即落一行，名称后台回填
        #[unsafe(method(settingsAddStock:))]
        fn settings_add_stock(&self, _sender: &AnyObject) {
            self.do_add_stock();
        }

        #[unsafe(method(settingsSave:))]
        fn settings_save(&self, _sender: &AnyObject) {
            self.do_save_settings();
        }

        #[unsafe(method(settingsReset:))]
        fn settings_reset(&self, _sender: &AnyObject) {
            // 恢复默认只重置外观与刷新参数，自选列表保留
            let names: Vec<(String, String)> = self
                .ivars()
                .state
                .borrow()
                .cfg
                .stocks
                .iter()
                .map(|s| (s.secid.clone(), s.name.clone()))
                .collect();
            let cfg = Config {
                stocks: names
                    .into_iter()
                    .map(|(secid, name)| Stock {
                        code: secid::code_of(&secid),
                        secid,
                        name,
                        ..Default::default()
                    })
                    .collect(),
                ..Config::default()
            };
            let slot = self.ivars().settings.borrow();
            if let Some(win) = slot.as_ref() {
                win.populate(&cfg);
                win.rebuild_list(self);
                win.set_hint("已恢复默认外观与刷新参数（自选列表保留），点「保存」生效", false);
            }
        }

        #[unsafe(method(settingsCancel:))]
        fn settings_cancel(&self, _sender: &AnyObject) {
            if let Some(win) = self.ivars().settings.borrow().as_ref() {
                win.order_out();
            }
        }

        #[unsafe(method(reloadConfig:))]
        fn reload_config(&self, _sender: &AnyObject) {
            self.reload_config_now();
            self.refresh_now();
        }

        #[unsafe(method(openConfig:))]
        fn open_config(&self, _sender: &AnyObject) {
            let path = config::config_path();
            if !path.exists() {
                let _ = self.ivars().state.borrow().cfg.save();
            }
            let _ = std::process::Command::new("open").arg("-t").arg(&path).spawn();
        }

        #[unsafe(method(openConfigDir:))]
        fn open_config_dir(&self, _sender: &AnyObject) {
            let dir = config::config_dir();
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::process::Command::new("open").arg(&dir).spawn();
        }

        #[unsafe(method(quitApp:))]
        fn quit_app(&self, _sender: &AnyObject) {
            NSApplication::sharedApplication(mtm()).terminate(None);
        }
    }
);

impl Controller {
    fn new(ivars: Ivars) -> Retained<Self> {
        let this = Self::alloc(mtm()).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    // ------------------------------------------------------------ 心跳

    fn on_tick(&self) {
        self.maybe_reload_config();
        self.start_fetch(false);
        self.drain_fetch();
        self.drain_resolved_names();
        self.poll_suggest();
        self.drain_candidates();
        self.persist_learned_names();
        self.render_if_needed();
        self.update_marquee();
        self.update_hover();
        if debug_force_panel() {
            let ready = !self.ivars().state.borrow().quotes.is_empty();
            if ready {
                self.show_panel();
                self.position_panel();
            }
        }
        if debug_settings() {
            let already = self
                .ivars()
                .settings
                .borrow()
                .as_ref()
                .map(|w| w.is_visible())
                .unwrap_or(false);
            if !already {
                self.open_settings();
            }
        }
        if debug_settings()
            && std::env::var("MBQUOTE_DEBUG_SETTINGS").as_deref() == Ok("save")
            && !self.ivars().settings_selftested.replace(true)
        {
            {
                let slot = self.ivars().settings.borrow();
                if let Some(win) = slot.as_ref() {
                    win.debug_bump();
                }
            }
            self.do_save_settings();
            eprintln!("[tickerstrip/selftest] 已触发保存");
        }
        if debug_settings()
            && std::env::var("MBQUOTE_DEBUG_SETTINGS").as_deref() != Ok("save")
            && !self.ivars().settings_selftested.replace(true)
        {
            let base = self.ivars().state.borrow().cfg.clone();
            if let Some(win) = self.ivars().settings.borrow().as_ref() {
                eprintln!("[tickerstrip/selftest] {}", win.selftest(&base));
            }
        }
        self.debug_maybe_shot();
        self.debug_maybe_type();
        if debug_on() || debug_snapshot_path().is_some() {
            self.debug_dump();
        }
    }

    /// 产出 README 素材：行情到位后渲染状态栏文字与面板各一张
    fn debug_maybe_shot(&self) {
        let Some(dir) = debug_shot_dir() else { return };
        if self.ivars().shot_done.get() {
            return;
        }
        {
            let st = self.ivars().state.borrow();
            if st.quotes.is_empty() {
                return;
            }
        }
        self.ivars().shot_done.set(true);
        // 滚动条回到起点，让图里从第一只股票开始，好看一点
        self.ivars().state.borrow_mut().offset = 0;
        self.update_marquee();
        let bar = format!("{dir}/raw-bar.png");
        let panel = format!("{dir}/raw-panel.png");
        snapshot_view_self(&self.ivars().button, &bar);
        snapshot_view(&self.ivars().effect, &panel);
        // 状态栏那一行的内容另外导出成 JSON：状态栏视图的渲染倍率跟着所在屏幕走
        // （1x 屏上只有 22px 高），交给合成脚本按任意倍率重绘才清晰
        self.export_bar_runs(&format!("{dir}/raw-bar.json"));
        eprintln!("[{APP_NAME}/shot] 素材已输出：{bar} 与 {panel}");
    }

    /// 导出状态栏可见文字的分段与颜色
    fn export_bar_runs(&self, path: &str) {
        let (runs, cfg) = {
            let st = self.ivars().state.borrow();
            let mut chars = st.bar.clone();
            // 只取起始的一段：合成脚本按宽度截断
            chars.truncate(400);
            (merge_runs(&chars), st.cfg.clone())
        };
        let items: Vec<serde_json::Value> = runs
            .into_iter()
            .map(|(text, style)| {
                let color = match style {
                    BarStyle::Plain => serde_json::Value::Null,
                    BarStyle::Up => serde_json::Value::String(cfg.up_color.clone()),
                    BarStyle::Down => serde_json::Value::String(cfg.down_color.clone()),
                };
                serde_json::json!({ "text": text, "color": color })
            })
            .collect();
        let panel_pt = self.ivars().effect.bounds().size;
        let doc = serde_json::json!({
            "width_pt": cfg.bar_width,
            "height_pt": 22.0,
            "font_size": cfg.bar_font_size,
            "panel_height_pt": panel_pt.height,
            "runs": items,
        });
        if let Err(e) = std::fs::write(path, doc.to_string()) {
            eprintln!("[{APP_NAME}/shot] 导出 {path} 失败：{e}");
        }
    }

    /// `MBQUOTE_DEBUG_SETTINGS=type` + `MBQUOTE_DEBUG_TYPE=<关键词>`：
    /// 自动往「添加」框里打字，验证「名称/拼音 → 候选 → 添加」整条链路
    fn debug_maybe_type(&self) {
        let mode = std::env::var("MBQUOTE_DEBUG_SETTINGS").unwrap_or_default();
        if mode != "type" && mode != "typeview" {
            return;
        }
        let Some(keyword) = std::env::var("MBQUOTE_DEBUG_TYPE").ok() else {
            return;
        };
        let n = self.ivars().debug_ticks.get() + 1;
        self.ivars().debug_ticks.set(n);
        let slot = self.ivars().settings.borrow();
        let Some(win) = slot.as_ref() else { return };
        // `typeview` 只打字不添加，方便截图看候选下拉
        let view_only = std::env::var("MBQUOTE_DEBUG_SETTINGS").as_deref() == Ok("typeview");
        match n {
            10 => {
                win.set_string_for_debug(&keyword);
                eprintln!("[tickerstrip/typetest] 已输入关键词：{keyword}");
            }
            26 => {
                eprintln!(
                    "[tickerstrip/typetest] 候选 {} 个；选中={:?}",
                    win.candidate_count(),
                    win.selected_candidate_for(&keyword).map(|c| c.label())
                );
                drop(slot);
                if !view_only {
                    self.do_add_stock();
                }
            }
            30 => {
                eprintln!("[tickerstrip/typetest] 添加后设置窗口里的自选：");
                for s in win.debug_stocks() {
                    eprintln!(
                        "[tickerstrip/typetest]    {} {} {}",
                        s.secid, s.code, s.name
                    );
                }
                self.ivars().debug_ticks.set(9999);
            }
            _ => {}
        }
    }

    /// 调试：验证状态栏 attributedTitle 真的带上了颜色。
    /// 做法是自己构造一遍，再用 `attribute:atIndex:effectiveRange:` 把颜色读回来比对。
    fn debug_dump_bar_colors(&self) {
        let (bar, cfg) = {
            let st = self.ivars().state.borrow();
            (st.bar.clone(), st.cfg.clone())
        };
        if bar.is_empty() {
            return;
        }
        let runs = merge_runs(&bar);
        let plain: String = runs.iter().map(|(t, _)| t.as_str()).collect();
        let attr = NSMutableAttributedString::initWithString(
            NSMutableAttributedString::alloc(),
            &ns(&plain),
        );
        let up = srgb(&cfg.up_color);
        let down = srgb(&cfg.down_color);
        let mut loc = 0usize;
        let mut report: Vec<String> = Vec::new();
        for (text, style) in &runs {
            let len = text.encode_utf16().count();
            if len == 0 {
                continue;
            }
            if let Some(color) = style_color(*style, &cfg) {
                unsafe {
                    attr.addAttribute_value_range(
                        NSForegroundColorAttributeName,
                        as_any(&color),
                        NSRange::new(loc, len),
                    );
                }
            }
            // 读回来看看这一段到底是什么颜色
            let read = unsafe {
                attr.attribute_atIndex_effectiveRange(
                    NSForegroundColorAttributeName,
                    loc,
                    std::ptr::null_mut(),
                )
            };
            let label = match read {
                Some(obj) => {
                    let is_up: bool = unsafe { msg_send![&*up, isEqual: &*obj] };
                    let is_down: bool = unsafe { msg_send![&*down, isEqual: &*obj] };
                    if is_up {
                        "涨色"
                    } else if is_down {
                        "跌色"
                    } else {
                        "其它色"
                    }
                }
                None => "系统色",
            };
            report.push(format!("{:?}={}", text.trim(), label));
            loc += len;
        }
        eprintln!("[tickerstrip/barcolor] {}", report.join("  "));
    }

    fn debug_dump(&self) {
        static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
        let now = Instant::now();
        {
            let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(t) = *last
                && now.duration_since(t) < debug_cadence()
            {
                return;
            }
            *last = Some(now);
        }
        if let Some(path) = debug_snapshot_path() {
            self.snapshot_panel(&path);
            self.snapshot_settings(&format!("{path}.settings.png"));
            snapshot_view_self(&self.ivars().button, &format!("{path}.bar.png"));
            if let Some(win) = self.ivars().settings.borrow().as_ref() {
                eprintln!("[tickerstrip/settings] {}", win.dump_frames());
            }
        }
        let icon = self.icon_rect();
        let mouse = NSEvent::mouseLocation();
        let panel = self.ivars().panel.frame();
        let st = self.ivars().state.borrow();
        let screens = NSScreen::screens(mtm());
        {
            let btn = &self.ivars().button;
            let bf = btn.frame();
            let wf = btn.window().map(|w| w.frame());
            let wn = btn.window().map(|w| w.title().to_string());
            let th = NSStatusBar::systemStatusBar().thickness();
            eprintln!(
                "[tickerstrip/dbg] btn_frame=({:.0},{:.0},{:.0}x{:.0}) win={:?} win_title={:?} thickness={:.0} item_visible={} screens={}",
                min_x(bf),
                bf.origin.y,
                bf.size.width,
                bf.size.height,
                wf.map(|r| (r.origin.x, r.origin.y, r.size.width, r.size.height)),
                wn,
                th,
                self.ivars().status_item.isVisible(),
                screens.iter().count()
            );
            for (i, sc) in screens.iter().enumerate() {
                let f = sc.frame();
                let vf = sc.visibleFrame();
                eprintln!(
                    "[tickerstrip/dbg] screen[{i}] frame=({:.0},{:.0},{:.0}x{:.0}) visible=({:.0},{:.0},{:.0}x{:.0})",
                    min_x(f),
                    f.origin.y,
                    f.size.width,
                    f.size.height,
                    min_x(vf),
                    vf.origin.y,
                    vf.size.width,
                    vf.size.height
                );
            }
        }
        let screens = NSScreen::screens(mtm());
        let main = screens.iter().next().map(|s| s.frame());
        eprintln!(
            "[tickerstrip] over_icon={} anchor={:.0} screen=({:.0},{:.0},{:.0}x{:.0}) icon=({:.0},{:.0},{:.0}x{:.0}) mouse=({:.0},{:.0}) panel=({:.0},{:.0},{:.0}x{:.0}) visible={} quotes={} title={:?}",
            st.over_icon,
            st.anchor_x,
            main.map(min_x).unwrap_or(0.0),
            main.map(|r| r.origin.y).unwrap_or(0.0),
            main.map(|r| r.size.width).unwrap_or(0.0),
            main.map(|r| r.size.height).unwrap_or(0.0),
            min_x(icon),
            icon.origin.y,
            icon.size.width,
            icon.size.height,
            mouse.x,
            mouse.y,
            min_x(panel),
            panel.origin.y,
            panel.size.width,
            panel.size.height,
            st.visible,
            st.quotes.len(),
            self.ivars().button.title().to_string()
        );
        self.debug_dump_bar_colors();
        let full: String = st.bar.iter().map(|(c, _)| *c).collect();
        eprintln!(
            "[tickerstrip/bartext] {}",
            full.chars().take(200).collect::<String>()
        );
        let _ = main;
    }

    /// 配置文件被 CLI 改动后自动生效（只看 mtime）
    fn maybe_reload_config(&self) {
        let now = Instant::now();
        if mins(now, self.ivars().state.borrow().last_mtime_check) < CONFIG_POLL {
            return;
        }
        let mtime = std::fs::metadata(config::config_path())
            .and_then(|m| m.modified())
            .ok();
        let changed = {
            let mut st = self.ivars().state.borrow_mut();
            st.last_mtime_check = now;
            if mtime != st.cfg_mtime {
                st.cfg_mtime = mtime;
                true
            } else {
                false
            }
        };
        if changed {
            self.reload_config_now();
        }
    }

    fn reload_config_now(&self) {
        self.apply_config(Config::load());
    }

    /// 把一份新配置应用到界面（设置窗口保存 / 磁盘配置变更都走这里）
    fn apply_config(&self, cfg: Config) {
        let width = cfg.bar_width;
        {
            let mut st = self.ivars().state.borrow_mut();
            st.cfg = cfg;
            st.offset = 0;
            st.render_key.clear();
            st.last_fetch = Instant::now() - Duration::from_secs(3600);
            st.cfg_mtime = std::fs::metadata(config::config_path())
                .and_then(|m| m.modified())
                .ok();
        }
        self.ivars().status_item.setLength(width);
        self.render_if_needed();
        self.update_marquee();
        self.start_fetch(true);
    }

    // ------------------------------------------------------------ 设置窗口

    fn open_settings(&self) {
        {
            let mut slot = self.ivars().settings.borrow_mut();
            if slot.is_none() {
                let cfg = self.ivars().state.borrow().cfg.clone();
                *slot = Some(Settings::new(self, &cfg));
            }
        }
        let cfg = self.ivars().state.borrow().cfg.clone();
        let slot = self.ivars().settings.borrow();
        if let Some(win) = slot.as_ref() {
            // 已经开着就别再灌一遍配置，免得覆盖用户正在改的内容
            if !win.is_visible() {
                win.populate(&cfg);
                win.rebuild_list(self);
                win.set_hint("名称会自动补全；列表里用 ↑ ↓ 调顺序", false);
            }
            win.show();
        }
    }

    /// 添加自选：代码直接加；名称 / 拼音走候选搜索。
    fn do_add_stock(&self) {
        let (raw, selected) = {
            let slot = self.ivars().settings.borrow();
            let Some(win) = slot.as_ref() else { return };
            (
                win.add_input(),
                win.selected_candidate_for(&win.add_input()),
            )
        };
        if raw.is_empty() {
            self.hint(
                "请输入代码、名称或拼音，例如 600519 / 茅台 / gzmt / AAPL",
                true,
            );
            return;
        }
        // 1) 能确定是代码 → 直接加
        if let Some(secid) = code_fast_path(&raw) {
            let name = selected
                .as_ref()
                .filter(|c| c.secid == secid)
                .map(|c| c.name.clone())
                .unwrap_or_default();
            self.add_stock_now(secid, name);
            return;
        }
        // 2) 候选已经搜出来了，用选中的那个
        if let Some(c) = selected {
            self.add_stock_now(c.secid.clone(), c.name.clone());
            return;
        }
        // 3) 还没搜 / 搜索没回来 → 立刻搜，出结果直接取最相关的
        {
            let mut st = self.ivars().state.borrow_mut();
            st.add_after_search = true;
            st.search_query.clear();
        }
        self.hint(&format!("正在搜索「{raw}」…"), false);
        self.start_suggest(&raw);
    }

    /// 真正落一条自选（去重 + 清输入 + 后台补名称）
    fn add_stock_now(&self, secid: String, name: String) {
        {
            let slot = self.ivars().settings.borrow();
            let Some(win) = slot.as_ref() else { return };
            if !win.add_pending(secid.clone(), secid::code_of(&secid), name.clone()) {
                win.set_hint(&format!("{secid} 已经在自选里了"), true);
                return;
            }
            win.clear_add_input();
            win.set_candidates_for("", &[]);
            win.rebuild_list(self);
            let msg = if name.is_empty() {
                format!("已添加 {secid}，正在取名称…")
            } else {
                format!("已添加 {} {secid}", name)
            };
            win.set_hint(&msg, false);
        }
        {
            let mut st = self.ivars().state.borrow_mut();
            st.search_input.clear();
            st.search_query.clear();
            st.add_after_search = false;
        }
        if name.is_empty() {
            let client = self.ivars().client.clone();
            let cfg = self.ivars().state.borrow().cfg.clone();
            let tx = self.ivars().name_tx.clone();
            let sid = secid.clone();
            std::thread::spawn(move || {
                if let Some(n) = quote::resolve_name(&client, &sid, &cfg) {
                    let _ = tx.send((sid, n));
                }
            });
        }
    }

    fn hint(&self, text: &str, error: bool) {
        if let Some(win) = self.ivars().settings.borrow().as_ref() {
            win.set_hint(text, error);
        }
    }

    /// 输入框内容变了就（防抖后）搜一次候选
    fn poll_suggest(&self) {
        let text = {
            let slot = self.ivars().settings.borrow();
            let Some(win) = slot.as_ref() else { return };
            if !win.is_visible() {
                return;
            }
            win.add_input()
        };
        let now = Instant::now();
        let mut start_search = false;
        {
            let mut st = self.ivars().state.borrow_mut();
            if st.search_input != text {
                st.search_input = text.clone();
                st.search_at = now;
                st.searching = false;
                st.search_query.clear();
                if text.is_empty() || code_fast_path(&text).is_some() {
                    st.add_after_search = false;
                }
            } else if !st.searching
                && !st.search_input.is_empty()
                && st.search_query != st.search_input
                && code_fast_path(&st.search_input).is_none()
                && mins(now, st.search_at) >= SUGGEST_DEBOUNCE
            {
                start_search = true;
            }
        }
        // 代码形式不用搜，顺手把候选清掉
        let is_code = !text.is_empty() && code_fast_path(&text).is_some();
        if is_code || text.is_empty() {
            if let Some(win) = self.ivars().settings.borrow().as_ref() {
                if win.candidate_count() > 0 {
                    win.set_candidates_for("", &[]);
                }
                if is_code {
                    win.set_hint(&format!("回车或点「添加」直接加入 {text}"), false);
                }
            }
            return;
        }
        if start_search {
            self.start_suggest(&text);
        }
    }

    fn start_suggest(&self, keyword: &str) {
        {
            let mut st = self.ivars().state.borrow_mut();
            if st.searching {
                return;
            }
            st.searching = true;
        }
        let client = self.ivars().client.clone();
        let tx = self.ivars().cand_tx.clone();
        let kw = keyword.to_string();
        std::thread::spawn(move || {
            let list = client.suggest(&kw, 8).unwrap_or_default();
            let _ = tx.send((kw, list));
        });
    }

    fn drain_candidates(&self) {
        let mut got = None;
        {
            let rx = self.ivars().cand_rx.borrow();
            while let Ok(v) = rx.try_recv() {
                got = Some(v);
            }
        }
        let Some((query, list)) = got else { return };
        let add_now = {
            let mut st = self.ivars().state.borrow_mut();
            st.searching = false;
            st.search_query = query.clone();
            // 只认当前输入的结果，避免旧搜索结果盖掉新的
            if st.search_input != query {
                return;
            }
            let take = st.add_after_search;
            st.add_after_search = false;
            take
        };
        {
            let slot = self.ivars().settings.borrow();
            let Some(win) = slot.as_ref() else { return };
            win.set_candidates_for(&query, &list);
            if list.is_empty() {
                win.set_hint(&format!("没搜到「{query}」，换个关键词或直接填代码"), true);
            }
        }
        if !list.is_empty() {
            if add_now {
                let c = &list[0];
                self.add_stock_now(c.secid.clone(), c.name.clone());
            } else {
                self.hint(
                    &format!("匹配到 {} 个候选，选一个再点「添加」", list.len()),
                    false,
                );
            }
        }
    }

    fn do_save_settings(&self) {
        let new_cfg = {
            let slot = self.ivars().settings.borrow();
            let Some(win) = slot.as_ref() else { return };
            let base = self.ivars().state.borrow().cfg.clone();
            win.read_config(&base)
        };
        if let Err(e) = new_cfg.save() {
            if let Some(win) = self.ivars().settings.borrow().as_ref() {
                win.set_hint(&format!("写入配置失败：{e}"), true);
            }
            return;
        }
        self.apply_config(new_cfg);
        if let Some(win) = self.ivars().settings.borrow().as_ref() {
            win.order_out();
        }
    }

    /// 设置窗口里新加的标的，名称取回来后回填列表 + 运行时配置
    fn drain_resolved_names(&self) {
        let mut got: Vec<(String, String)> = Vec::new();
        {
            let rx = self.ivars().name_rx.borrow();
            while let Ok(v) = rx.try_recv() {
                got.push(v);
            }
        }
        if got.is_empty() {
            return;
        }
        {
            let slot = self.ivars().settings.borrow();
            if let Some(win) = slot.as_ref() {
                for (secid, name) in &got {
                    win.update_name(self, secid, name);
                }
                if let Some((secid, name)) = got.last() {
                    win.set_hint(&format!("已添加 {secid} {name}"), false);
                }
            }
        }
        let mut st = self.ivars().state.borrow_mut();
        for (secid, name) in got {
            if let Some(stock) = st.cfg.stocks.iter_mut().find(|s| s.secid == secid) {
                stock.name = name;
            } else {
                st.cfg.stocks.push(Stock {
                    code: secid::code_of(&secid),
                    secid,
                    name,
                    ..Default::default()
                });
            }
        }
        st.names_dirty = true;
        st.render_key.clear();
    }

    /// 学到的名称攒一会儿再落盘，避免频繁写文件
    fn persist_learned_names(&self) {
        let due = {
            let st = self.ivars().state.borrow();
            st.names_dirty && mins(Instant::now(), st.last_save) >= Duration::from_secs(20)
        };
        if !due {
            return;
        }
        let cfg = {
            let mut st = self.ivars().state.borrow_mut();
            st.names_dirty = false;
            st.last_save = Instant::now();
            st.cfg.clone()
        };
        // 只把名称并回磁盘上的列表，别用内存里的整份配置覆盖（否则会抹掉别处新加的自选）
        if let Err(e) = cfg.merge_names_to_disk() {
            eprintln!("[{APP_NAME}] 名称回写失败：{e}");
        }
    }

    /// `force = false` 时遵守 `refresh_secs` 节流
    fn start_fetch(&self, force: bool) {
        let secids: Vec<String> = {
            let mut st = self.ivars().state.borrow_mut();
            let now = Instant::now();
            if st.fetching && mins(now, st.fetch_started) < FETCH_TIMEOUT {
                return;
            }
            let interval = if st.cfg.market_hours_only && !quote::in_trading_hours() {
                st.cfg.refresh_interval() * 12
            } else {
                st.cfg.refresh_interval()
            };
            if !force && !st.fetching && mins(now, st.last_fetch) < interval {
                return;
            }
            st.fetching = true;
            st.fetch_started = now;
            st.last_fetch = now;
            st.cfg.stocks.iter().map(|s| s.secid.clone()).collect()
        };
        if secids.is_empty() {
            let mut st = self.ivars().state.borrow_mut();
            st.fetching = false;
            st.header = format!("未配置自选 · 终端执行 {APP_NAME} add 600519");
            return;
        }
        let client = self.ivars().client.clone();
        let cfg = self.ivars().state.borrow().cfg.clone();
        let tx = self.ivars().tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(client.fetch(&secids, &cfg));
        });
    }

    fn drain_fetch(&self) {
        let mut got = None;
        {
            let rx = self.ivars().rx.borrow();
            while let Ok(v) = rx.try_recv() {
                got = Some(v);
            }
        }
        let Some(res) = got else { return };
        let mut st = self.ivars().state.borrow_mut();
        st.fetching = false;
        match res {
            Ok(outcome) => {
                let mut quotes = outcome.quotes;
                // 源没给名字（或本地还空着）时用配置里存的名字兜底，学到的名字回写内存配置
                for q in quotes.iter_mut() {
                    let known = st.cfg.stocks.iter().find(|s| s.secid == q.secid);
                    if let Some(stock) = known
                        && q.name.is_empty()
                    {
                        q.name = stock.name.clone();
                    }
                }
                for q in quotes.iter() {
                    if q.name.is_empty() {
                        continue;
                    }
                    if let Some(stock) = st.cfg.stocks.iter_mut().find(|s| s.secid == q.secid)
                        && stock.name != q.name
                    {
                        stock.name = q.name.clone();
                    }
                }
                st.quotes = quotes;
                st.source = outcome.source;
                st.header = format!("更新于 {} · {}", now_hms(), st.source);
            }
            Err(e) => st.header = format!("抓取失败：{e}"),
        }
        st.render_key.clear();
    }

    /// 立即刷新（左键 / 菜单）
    fn refresh_now(&self) {
        {
            let mut st = self.ivars().state.borrow_mut();
            st.last_fetch = Instant::now() - Duration::from_secs(3600);
        }
        self.start_fetch(true);
        self.drain_fetch();
        self.render_if_needed();
        self.update_marquee();
    }

    /// 悬停展开时，数据太旧就顺手补一次
    fn refresh_if_stale(&self) {
        let stale = {
            let st = self.ivars().state.borrow();
            !st.fetching && mins(Instant::now(), st.last_fetch) >= HOVER_STALE
        };
        if stale {
            self.start_fetch(true);
        }
    }

    // ------------------------------------------------------------ 渲染

    fn render_if_needed(&self) {
        let key = {
            let st = self.ivars().state.borrow();
            let mut k = st.header.clone();
            for q in &st.quotes {
                k.push('|');
                k.push_str(&q.secid);
                k.push_str(&format!("{:?}{:?}{:?}", q.price, q.pct, q.name));
            }
            k
        };
        {
            let mut st = self.ivars().state.borrow_mut();
            if st.render_key == key {
                return;
            }
            st.render_key = key;
        }
        self.rebuild_marquee_text();
        self.render_panel();
    }

    /// 把行情拼成一条长字符串供滚动（带方向上色信息）
    fn rebuild_marquee_text(&self) {
        let mut st = self.ivars().state.borrow_mut();
        let cfg = st.cfg.clone();
        let quotes = st.quotes.clone();
        let segs = if quotes.is_empty() {
            let placeholder = if cfg.stocks.is_empty() {
                format!("未配置自选 · {APP_NAME} add 600519")
            } else {
                format!("拉取行情中…（{} 只）", cfg.stocks.len())
            };
            vec![(placeholder, BarStyle::Plain)]
        } else {
            build_bar_segments(&quotes, &cfg)
        };
        let mut bar: Vec<(char, BarStyle)> = Vec::new();
        for (text, style) in segs {
            bar.extend(text.chars().map(|c| (c, style)));
        }
        st.bar = bar;
        if st.offset >= st.bar.len() {
            st.offset = 0;
        }
    }

    fn update_marquee(&self) {
        let (plain, runs, use_color) = {
            let mut st = self.ivars().state.borrow_mut();
            if st.bar.is_empty() {
                return;
            }
            let cfg = st.cfg.clone();
            let budget = (cfg.bar_width - 10.0).max(24.0);
            let n = st.bar.len();
            let mut picked: Vec<(char, BarStyle)> = Vec::new();
            let mut px = 0.0;
            let mut idx = st.offset;
            let mut count = 0usize;
            while px < budget && count <= n {
                let (c, style) = st.bar[idx % n];
                let w = char_width(c, cfg.bar_font_size);
                if px + w > budget && count > 0 {
                    break;
                }
                picked.push((c, style));
                px += w;
                idx += 1;
                count += 1;
            }
            if cfg.marquee {
                st.offset = (st.offset + cfg.marquee_step.max(1)) % n;
            }
            let plain: String = picked.iter().map(|(c, _)| *c).collect();
            // 只有真的存在有色段落、且开关打开时才走 attributedTitle
            let use_color = cfg.bar_color && picked.iter().any(|(_, s)| *s != BarStyle::Plain);
            let runs = if use_color {
                merge_runs(&picked)
            } else {
                Vec::new()
            };
            (plain, runs, use_color)
        };

        if self.ivars().state.borrow().last_bar_text == plain {
            return;
        }
        self.ivars().state.borrow_mut().last_bar_text = plain.clone();

        let button = &self.ivars().button;
        if use_color {
            let cfg = self.ivars().state.borrow().cfg.clone();
            set_attributed_title(button, &runs, &cfg);
        } else {
            button.setTitle(&ns(&plain));
        }
    }

    fn render_panel(&self) {
        let (quotes, header, cfg) = {
            let st = self.ivars().state.borrow();
            (st.quotes.clone(), st.header.clone(), st.cfg.clone())
        };

        let max_rows = cfg.max_rows.max(1);
        let shown = quotes.len().min(max_rows);
        let row_height = row_h(&cfg);
        let width = cfg.panel_width;
        let height = PAD + HEADER_H + shown as f64 * row_height + FOOTER_H + PAD;

        // 行
        {
            let mut rows = self.ivars().rows.borrow_mut();
            while rows.len() < shown {
                rows.push(make_row(&self.ivars().effect, &cfg));
            }
            let name_max = ((NAME_W / cfg.panel_font_size).floor() as usize).max(4);
            for (i, row) in rows.iter().enumerate() {
                if i >= shown {
                    row.name.setHidden(true);
                    row.price.setHidden(true);
                    row.pct.setHidden(true);
                    continue;
                }
                let q = &quotes[i];
                row.name.setHidden(false);
                row.price.setHidden(false);
                row.pct.setHidden(false);
                row.name
                    .setStringValue(&ns(&truncate(&q.display_name(), name_max)));
                row.name.setToolTip(Some(&*ns(&q.detail_tooltip())));
                row.price.setToolTip(Some(&*ns(&q.detail_tooltip())));
                row.pct.setToolTip(Some(&*ns(&q.detail_tooltip())));
                row.price
                    .setStringValue(&ns(&q.price.map(quote::fmt_price).unwrap_or("—".into())));
                row.pct
                    .setStringValue(&ns(&q.pct.map(quote::fmt_pct).unwrap_or("—".into())));
                let color = direction_color(&cfg, q.rising());
                row.price.setTextColor(Some(&*color));
                row.pct.setTextColor(Some(&*color));

                let y = PAD + FOOTER_H + (shown - i - 1) as f64 * row_height;
                layout_row(row, y, width, row_height);
            }
        }

        // 表头
        {
            let mut slot = self.ivars().header.borrow_mut();
            if slot.is_none() {
                let field = make_label(&header_text_font(&cfg));
                self.ivars().effect.addSubview(&field);
                *slot = Some(field);
            }
            let field = slot.as_ref().unwrap();
            field.setStringValue(&ns(&format!("{}   ·   {} 只", header, quotes.len())));
            field.setFrame(NSRect::new(
                NSPoint::new(PAD, height - PAD - HEADER_H + 3.0),
                NSSize::new(width - PAD * 2.0, HEADER_H - 3.0),
            ));
        }

        // 页脚：三行操作提示
        {
            let mut footers = self.ivars().footers.borrow_mut();
            if footers.is_empty() {
                let hints: [String; 3] = [
                    "左键状态栏 = 打开设置".to_string(),
                    "右键状态栏 = 菜单（刷新 / 退出）".to_string(),
                    format!("终端：{APP_NAME} add 600519"),
                ];
                for text in hints {
                    let field = make_label(&footer_font(&cfg));
                    field.setStringValue(&ns(&text));
                    field.setTextColor(Some(&NSColor::tertiaryLabelColor()));
                    self.ivars().effect.addSubview(&field);
                    footers.push(field);
                }
            }
            for (i, field) in footers.iter().enumerate() {
                field.setFrame(NSRect::new(
                    NSPoint::new(PAD, PAD + (footers.len() - i - 1) as f64 * FOOTER_LINE_H),
                    NSSize::new(width - PAD * 2.0, FOOTER_LINE_H - 1.0),
                ));
            }
        }

        *self.ivars().panel_size.borrow_mut() = NSSize::new(width, height);
        self.resize_panel();
    }

    fn resize_panel(&self) {
        let size = *self.ivars().panel_size.borrow();
        let panel = &self.ivars().panel;
        panel.setFrame_display(NSRect::new(NSPoint::new(0.0, 0.0), size), false);
        self.ivars()
            .effect
            .setFrame(NSRect::new(NSPoint::new(0.0, 0.0), size));
        self.position_panel();
    }

    /// 把面板放到「展开时鼠标所在那块屏幕」的菜单栏正下方。
    /// 用 `visibleFrame().max_y`（菜单栏下沿）而不是状态栏按钮矩形：
    /// 多显示器时按钮矩形会随当前菜单栏实例漂移。
    fn position_panel(&self) {
        let size = *self.ivars().panel_size.borrow();
        let anchor_x = self.ivars().state.borrow().anchor_x;
        if size.width <= 0.0 {
            return;
        }
        let visible = self.screen_visible_frame(NSEvent::mouseLocation());
        let mut frame = self.ivars().panel.frame();
        frame.origin.x = (anchor_x - size.width / 2.0)
            .clamp(min_x(visible) + 8.0, max_x(visible) - size.width - 8.0);
        frame.origin.y = (max_y(visible) - size.height - 6.0).max(min_y(visible) + 8.0);
        self.ivars().panel.setFrame_display(frame, false);
    }

    /// 某个点所在屏幕的可用区域（不含菜单栏与 Dock）
    fn screen_visible_frame(&self, point: NSPoint) -> NSRect {
        let screens = NSScreen::screens(mtm());
        for s in screens.iter() {
            let f = s.frame();
            if point.x >= min_x(f)
                && point.x <= max_x(f)
                && point.y >= min_y(f)
                && point.y <= max_y(f)
            {
                return s.visibleFrame();
            }
        }
        NSScreen::mainScreen(mtm())
            .map(|s| s.visibleFrame())
            .or_else(|| screens.iter().next().map(|s| s.visibleFrame()))
            .unwrap_or_else(|| NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0)))
    }

    /// 状态栏图标在屏幕坐标系中的矩形
    fn icon_rect(&self) -> NSRect {
        let empty = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
        let button = &self.ivars().button;
        let Some(window) = button.window() else {
            return empty;
        };
        let rect = window.convertRectToScreen(button.frame());
        if rect.size.width <= 0.0 || rect.size.width > 600.0 {
            empty
        } else {
            rect
        }
    }

    // ------------------------------------------------------------ 悬停

    fn update_hover(&self) {
        let now = Instant::now();
        let mouse = NSEvent::mouseLocation();
        let over_icon = self.ivars().state.borrow().over_icon;
        let over_panel = {
            let st = self.ivars().state.borrow();
            st.visible && contains(self.ivars().panel.frame(), mouse)
        };

        let (hover_delay, hide_delay, visible) = {
            let st = self.ivars().state.borrow();
            (
                Duration::from_millis(st.cfg.hover_delay_ms),
                Duration::from_millis(st.cfg.hide_delay_ms),
                st.visible,
            )
        };

        {
            let mut st = self.ivars().state.borrow_mut();
            if over_icon || over_panel {
                st.last_over = now;
                // 进入图标时开始计时；移到面板上时保持计时
                if over_icon && st.hover_since.is_none() {
                    st.hover_since = Some(now);
                }
            } else {
                st.hover_since = None;
            }
        }

        let hovered_for = {
            let st = self.ivars().state.borrow();
            st.hover_since.map(|t| mins(now, t))
        };
        let since_over = mins(now, self.ivars().state.borrow().last_over);

        if !visible && over_icon && hovered_for.unwrap_or_default() >= hover_delay {
            self.show_panel();
        } else if visible && !over_icon && !over_panel && since_over >= hide_delay {
            self.hide_panel();
        }
    }

    /// 把面板内容视图渲染成 PNG，用于核对排版
    /// 把面板内容视图渲染成 PNG，用于核对排版
    fn snapshot_panel(&self, path: &str) {
        snapshot_view(&self.ivars().effect, path);
    }

    /// 设置窗口也渲染一张，方便核对表单排版
    fn snapshot_settings(&self, path: &str) {
        if let Some(win) = self.ivars().settings.borrow().as_ref() {
            snapshot_view(&win.content_view(), path);
        }
    }

    fn show_panel(&self) {
        {
            let mut st = self.ivars().state.borrow_mut();
            if st.visible {
                return;
            }
            if !st.visible {
                st.anchor_x = NSEvent::mouseLocation().x;
            }
            st.visible = true;
        }
        self.render_if_needed();
        self.position_panel();
        self.ivars().panel.setAlphaValue(1.0);
        self.ivars().panel.orderFront(None);
        self.refresh_if_stale();
    }

    fn hide_panel(&self) {
        {
            let mut st = self.ivars().state.borrow_mut();
            if !st.visible {
                return;
            }
            st.visible = false;
        }
        self.ivars().panel.orderOut(None);
    }
}

// ---------------------------------------------------------------- 构建

fn row_h(cfg: &Config) -> f64 {
    (cfg.panel_font_size + 8.0).max(18.0)
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let mut out: String = chars[..max.saturating_sub(1)].iter().collect();
    out.push('…');
    out
}

fn min_x(r: NSRect) -> f64 {
    r.origin.x
}
fn max_x(r: NSRect) -> f64 {
    r.origin.x + r.size.width
}
fn min_y(r: NSRect) -> f64 {
    r.origin.y
}
fn max_y(r: NSRect) -> f64 {
    r.origin.y + r.size.height
}

fn contains(r: NSRect, p: NSPoint) -> bool {
    r.size.width > 0.0
        && r.size.height > 0.0
        && p.x >= min_x(r)
        && p.x <= max_x(r)
        && p.y >= r.origin.y
        && p.y <= r.origin.y + r.size.height
}

/// 粗略字宽：ASCII 约 0.6em，CJK 约 1.0em
fn char_width(c: char, font_size: f64) -> f64 {
    if c.is_ascii() {
        font_size * 0.6
    } else {
        font_size
    }
}

fn now_hms() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64 + 8 * 3600;
    let s = secs.rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

fn header_text_font(cfg: &Config) -> Retained<NSFont> {
    unsafe {
        NSFont::monospacedDigitSystemFontOfSize_weight(
            cfg.panel_font_size - 1.0,
            NSFontWeightRegular,
        )
    }
}

fn footer_font(cfg: &Config) -> Retained<NSFont> {
    NSFont::systemFontOfSize((cfg.panel_font_size - 1.5).max(9.0))
}

fn body_font(cfg: &Config) -> Retained<NSFont> {
    NSFont::systemFontOfSize(cfg.panel_font_size)
}

fn num_font(cfg: &Config) -> Retained<NSFont> {
    unsafe {
        NSFont::monospacedDigitSystemFontOfSize_weight(cfg.panel_font_size, NSFontWeightRegular)
    }
}

fn direction_color(cfg: &Config, rising: Option<bool>) -> Retained<NSColor> {
    match rising {
        Some(true) => srgb(&cfg.up_color),
        Some(false) => srgb(&cfg.down_color),
        None => NSColor::secondaryLabelColor(),
    }
}

fn srgb(hex: &str) -> Retained<NSColor> {
    let (r, g, b) = config::hex_to_rgb(hex);
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0)
}

fn make_label(font: &NSFont) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&ns(""), mtm());
    field.setFont(Some(font));
    field.setTextColor(Some(&*NSColor::labelColor()));
    field.setAlignment(NSTextAlignment::Left);
    field.setBordered(false);
    field.setDrawsBackground(false);
    field.setEditable(false);
    field.setSelectable(false);
    field
}

fn make_row(effect: &NSVisualEffectView, cfg: &Config) -> Row {
    let text_font = body_font(cfg);
    let num = num_font(cfg);

    let name = make_label(&text_font);
    let price = make_label(&num);
    let pct = make_label(&num);
    price.setAlignment(NSTextAlignment::Right);
    pct.setAlignment(NSTextAlignment::Right);

    for v in [&name, &price, &pct] {
        effect.addSubview(v);
    }
    let row = Row { name, price, pct };
    layout_row(&row, 0.0, cfg.panel_width, row_h(cfg));
    row
}

fn layout_row(row: &Row, y: f64, width: f64, height: f64) {
    let inner = width - PAD * 2.0;
    let name_w = NAME_W.min(inner - PRICE_W - PCT_W - 12.0).max(60.0);
    row.name.setFrame(NSRect::new(
        NSPoint::new(PAD, y + 2.0),
        NSSize::new(name_w, height - 2.0),
    ));
    row.price.setFrame(NSRect::new(
        NSPoint::new(PAD + name_w, y + 2.0),
        NSSize::new(PRICE_W, height - 2.0),
    ));
    row.pct.setFrame(NSRect::new(
        NSPoint::new(width - PAD - PCT_W, y + 2.0),
        NSSize::new(PCT_W, height - 2.0),
    ));
}

/// 按指定倍率渲染视图自身。
///
/// 不依赖窗口所在屏幕的 `backingScaleFactor`：先开一张 `scale` 倍的 NSImage 画布，
/// 再用 `displayRectIgnoringOpacity:` 把视图画进去——文字是矢量的，放大后依然锐利。
fn snapshot_view_scaled(view: &NSView, scale: f64, path: &str) {
    view.layoutSubtreeIfNeeded();
    view.displayIfNeeded();
    let bounds = view.bounds();
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        eprintln!("[{APP_NAME}/shot] {path} 尺寸为 0，跳过");
        return;
    }
    let size = NSSize::new(bounds.size.width * scale, bounds.size.height * scale);
    let canvas = NSImage::initWithSize(NSImage::alloc(), size);
    canvas.lockFocus();
    view.displayRectIgnoringOpacity(bounds);
    canvas.unlockFocus();

    let Some(rep) = canvas
        .TIFFRepresentation()
        .and_then(|t| NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &t))
    else {
        eprintln!("[{APP_NAME}/shot] {path} 转位图失败");
        return;
    };
    let props: Retained<NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>> = NSDictionary::new();
    let Some(data) =
        (unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) })
    else {
        return;
    };
    let ok = data.writeToFile_atomically(&ns(path), true);
    eprintln!(
        "[{APP_NAME}/shot]   {path} {}x{} wrote={ok}",
        size.width as i32, size.height as i32
    );
}

/// 只渲染视图自身（用于状态栏按钮这种「父视图是整条菜单栏」的情况）
fn snapshot_view_self(view: &NSView, path: &str) {
    view.layoutSubtreeIfNeeded();
    view.displayIfNeeded();
    let bounds = view.bounds();
    eprintln!(
        "[{APP_NAME}/shot] snapshot_self {path} bounds={:?} hidden={} window={}",
        (
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height
        ),
        view.isHidden(),
        view.window().is_some()
    );
    let pdf = view.dataWithPDFInsideRect(bounds);
    let ok = pdf.writeToFile_atomically(&ns(&format!("{path}.pdf")), true);
    eprintln!("[{APP_NAME}/shot]   pdf bytes={} wrote={ok}", pdf.length());
    let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) else {
        eprintln!("[{APP_NAME}/shot]   bitmapImageRep = None");
        return;
    };
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let props: Retained<NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>> = NSDictionary::new();
    let Some(data) =
        (unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) })
    else {
        return;
    };
    let ok = data.writeToFile_atomically(&ns(path), true);
    eprintln!("[{APP_NAME}/shot]   png bytes={} wrote={ok}", data.length());
}

/// 把视图渲染出来核对排版（不需要录屏权限）。
///
/// 取父视图（窗口的 frame view）来画：它会补上窗口底色，否则深色外观下浅色文字画在
/// 透明底上，快照等于看不见。另外 `cacheDisplayInRect` 不触发控件 cell 布局，
/// 所以额外导出一份 PDF（打印路径）备用。
fn snapshot_view(view: &NSView, path: &str) {
    let parent: Option<Retained<NSView>> = unsafe { view.superview() };
    let target: &NSView = parent.as_deref().unwrap_or(view);
    target.layoutSubtreeIfNeeded();
    target.displayIfNeeded();
    let bounds = target.bounds();

    target
        .dataWithPDFInsideRect(bounds)
        .writeToFile_atomically(&ns(&format!("{path}.pdf")), true);

    let Some(rep) = target.bitmapImageRepForCachingDisplayInRect(bounds) else {
        return;
    };
    target.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let props: Retained<NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>> = NSDictionary::new();
    let Some(data) =
        (unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) })
    else {
        return;
    };
    data.writeToFile_atomically(&ns(path), true);
}

fn build_panel(cfg: &Config) -> (Retained<NSPanel>, Retained<NSVisualEffectView>) {
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(cfg.panel_width, 120.0));
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm()),
        rect,
        style,
        NSBackingStoreType::Buffered,
        false,
    );
    panel.setLevel(NSStatusWindowLevel);
    panel.setOpaque(false);
    panel.setHasShadow(true);
    panel.setBackgroundColor(Some(&*NSColor::clearColor()));
    panel.setHidesOnDeactivate(false);
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setMovable(false);
    unsafe { panel.setReleasedWhenClosed(false) };

    let effect = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm()), rect);
    effect.setMaterial(NSVisualEffectMaterial::HUDWindow);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    effect.setWantsLayer(true);
    if let Some(layer) = effect.layer() {
        unsafe {
            let _: () = msg_send![&*layer, setCornerRadius: 10.0f64];
            let _: () = msg_send![&*layer, setMasksToBounds: true];
        }
    }
    panel.setContentView(Some(&effect));

    (panel, effect)
}

fn build_menu() -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm());
    let add = |title: &str, action: Sel, key: &str| {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm()),
                &ns(title),
                Some(action),
                &ns(key),
            )
        };
        menu.addItem(&item);
    };
    add("设置…", sel!(openSettings:), ",");
    add("立即刷新", sel!(doRefresh:), "r");
    add("重载配置", sel!(reloadConfig:), "");
    menu.addItem(&NSMenuItem::separatorItem(mtm()));
    add("编辑配置文件", sel!(openConfig:), "");
    add("打开配置目录", sel!(openConfigDir:), "");
    menu.addItem(&NSMenuItem::separatorItem(mtm()));
    add("退出", sel!(quitApp:), "q");
    menu
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Stock};

    fn stock(secid: &str, in_bar: bool) -> Stock {
        Stock {
            secid: secid.into(),
            code: secid.split('.').nth(1).unwrap_or("").into(),
            name: format!("名{secid}"),
            in_bar,
        }
    }

    fn quote(secid: &str, pct: Option<f64>) -> Quote {
        Quote {
            secid: secid.into(),
            code: secid.split('.').nth(1).unwrap_or("").into(),
            name: format!("名{secid}"),
            price: Some(10.0),
            pct,
            ..Default::default()
        }
    }

    #[test]
    fn bar_segments_skip_panel_only() {
        let cfg = Config {
            stocks: vec![stock("1.000001", true), stock("0.300476", false)],
            ..Default::default()
        };
        let quotes = vec![quote("1.000001", Some(1.0)), quote("0.300476", Some(-2.0))];
        let segs = build_bar_segments(&quotes, &cfg);
        let text: String = segs.iter().map(|(t, _)| t.as_str()).collect();
        assert!(text.contains("名1.000001"), "进滚动栏的应该在：{text}");
        assert!(!text.contains("名0.300476"), "仅面板的不该出现：{text}");
    }

    #[test]
    fn bar_segments_color_by_direction() {
        let cfg = Config {
            stocks: vec![
                stock("1.600519", true),
                stock("0.300750", true),
                stock("1.000001", true),
            ],
            ..Default::default()
        };
        let quotes = vec![
            quote("1.600519", Some(0.52)),  // 涨
            quote("0.300750", Some(-0.94)), // 跌
            quote("1.000001", Some(0.0)),   // 平
        ];
        let segs = build_bar_segments(&quotes, &cfg);
        let style_of = |needle: &str| {
            segs.iter()
                .find(|(t, _)| t.trim_start().starts_with(needle))
                .map(|(_, s)| *s)
        };
        // 名称永远是系统色（菜单栏深浅色都能读）
        assert_eq!(style_of("名1.600519").unwrap(), BarStyle::Plain);
        // 价格/涨跌幅跟随涨跌方向
        assert!(
            segs.iter()
                .any(|(t, s)| *s == BarStyle::Up && t.contains("+0.52%"))
        );
        assert!(
            segs.iter()
                .any(|(t, s)| *s == BarStyle::Down && t.contains("-0.94%"))
        );
        assert!(
            segs.iter()
                .any(|(t, s)| *s == BarStyle::Plain && t.contains("0.00%"))
        );
    }

    #[test]
    fn bar_segments_fallback_when_all_panel_only() {
        let cfg = Config {
            stocks: vec![stock("1.000001", false)],
            ..Default::default()
        };
        let segs = build_bar_segments(&[quote("1.000001", Some(1.0))], &cfg);
        let text: String = segs.iter().map(|(t, _)| t.as_str()).collect();
        assert!(text.contains("盯盘"), "全「仅面板」时要有占位文字：{text}");
    }

    #[test]
    fn merge_runs_groups_same_style() {
        let chars: Vec<(char, BarStyle)> = "ab12"
            .chars()
            .zip([BarStyle::Plain, BarStyle::Plain, BarStyle::Up, BarStyle::Up])
            .collect();
        let runs = merge_runs(&chars);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].0, "ab");
        assert_eq!(runs[1].1, BarStyle::Up);
        assert_eq!(runs[1].0, "12");
    }
}
