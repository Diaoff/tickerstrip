//! 图形化设置窗口。
//!
//! 用 AppKit 原生控件拼的表单：上面是自选列表，下面按「外观 / 刷新与数据源」分组。
//! 所有控件共用一个 target，靠 `tag` 区分（`settingsSliderChanged:` / `settingsStockAction:`），
//! 这样 `define_class!` 里只要几个方法就够。
//!
//! 编辑在副本上进行，点「保存」才写回磁盘并生效，「取消」直接丢弃。

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{MainThreadMarker, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSBorderType, NSButton, NSButtonType, NSColor, NSControl, NSControlStateValueOff,
    NSControlStateValueOn, NSFont, NSFontWeightBold, NSFontWeightRegular, NSMenu, NSMenuItem,
    NSPopUpButton, NSScrollView, NSSlider, NSTextAlignment, NSTextField, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::config::{Config, DISPLAY_NAME, Stock};
use crate::quote::Candidate;

// ---- 布局 ----
const W: f64 = 620.0;
const PAD: f64 = 22.0;
const ROW_H: f64 = 26.0;
const LABEL_X: f64 = PAD;
const CTRL_X: f64 = 200.0;
const CTRL_W: f64 = 270.0;
const VAL_X: f64 = 484.0;
const VAL_W: f64 = 110.0;
const LIST_H: f64 = 132.0;

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

/// 滑块绑定的配置字段
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SliderKind {
    RefreshSecs,
    MarqueeStep,
    BarWidth,
    BarFont,
    PanelWidth,
    PanelFont,
    MaxRows,
    HoverDelay,
    HideDelay,
}

impl SliderKind {
    /// (最小值, 最大值, 刻度数, 单位, 小数位)
    fn spec(self) -> (f64, f64, isize, &'static str, usize) {
        match self {
            Self::RefreshSecs => (1.0, 60.0, 60, " 秒", 0),
            Self::MarqueeStep => (1.0, 6.0, 6, "", 0),
            Self::BarWidth => (120.0, 400.0, 29, " pt", 0),
            Self::BarFont => (10.0, 18.0, 9, " pt", 0),
            Self::PanelWidth => (240.0, 480.0, 25, " pt", 0),
            Self::PanelFont => (10.0, 18.0, 9, " pt", 0),
            Self::MaxRows => (4.0, 30.0, 27, " 行", 0),
            Self::HoverDelay => (0.0, 600.0, 31, " ms", 0),
            Self::HideDelay => (0.0, 1000.0, 21, " ms", 0),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::RefreshSecs => "行情刷新间隔",
            Self::MarqueeStep => "滚动速度",
            Self::BarWidth => "状态栏宽度",
            Self::BarFont => "状态栏字号",
            Self::PanelWidth => "面板宽度",
            Self::PanelFont => "面板字号",
            Self::MaxRows => "面板最多行数",
            Self::HoverDelay => "悬停展开延时",
            Self::HideDelay => "移开收起延时",
        }
    }

    fn format(self, v: f64) -> String {
        let (_, _, _, unit, decimals) = self.spec();
        format!("{v:.decimals$}{unit}")
    }
}

/// 勾选框绑定的配置字段
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CheckKind {
    ShowPrice,
    ShowPct,
    BarColor,
    Marquee,
    MarketHoursOnly,
    PreferTencent,
}

impl CheckKind {
    fn label(self) -> &'static str {
        match self {
            Self::ShowPrice => "状态栏显示价格",
            Self::ShowPct => "状态栏显示涨跌幅",
            Self::BarColor => "状态栏按涨跌上色",
            Self::Marquee => "横向滚动",
            Self::MarketHoursOnly => "非交易时段自动降频",
            Self::PreferTencent => "优先用腾讯行情",
        }
    }
}

pub struct SliderRow {
    slider: Retained<NSSlider>,
    value: Retained<NSTextField>,
    kind: SliderKind,
}

pub struct CheckRow {
    button: Retained<NSButton>,
    kind: CheckKind,
}

/// 行内视图由父视图持有，这里只是留个引用方便将来做局部刷新
#[allow(dead_code)]
struct StockRow {
    name: Retained<NSTextField>,
    code: Retained<NSTextField>,
}

pub struct Settings {
    pub window: Retained<NSWindow>,
    content: Retained<NSView>,
    sliders: Vec<SliderRow>,
    checks: Vec<CheckRow>,
    palette: Retained<NSPopUpButton>,
    /// 输入名称/拼音时的候选下拉
    cand_popup: Retained<NSPopUpButton>,
    candidates: RefCell<Vec<Candidate>>,
    /// 候选对应的关键词（防止用到上一次搜索的残留候选）
    cand_query: RefCell<String>,
    list_doc: Retained<NSView>,
    list_w: f64,
    rows: RefCell<Vec<StockRow>>,
    /// 编辑中的自选副本
    stocks: RefCell<Vec<Stock>>,
    add_field: Retained<NSTextField>,
    hint: Retained<NSTextField>,
}

/// 按 tag 拆解列表行按钮：tag = index * 10 + op
const OP_UP: isize = 0;
const OP_DOWN: isize = 1;
const OP_DEL: isize = 2;
/// 切换「是否进状态栏滚动」
const OP_TOGGLE_BAR: isize = 3;

impl Settings {
    /// `target` 是动作接收者（控制器），`action` 一律由控制器按 tag 分发。
    pub fn new<T: objc2::Message + 'static>(target: &T, cfg: &Config) -> Self {
        let mtm = MainThreadMarker::new().expect("设置窗口必须在主线程构建");
        let content = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, W, 720.0));

        let mut b = Builder {
            content: content.clone(),
            y: 0.0,
            sliders: Vec::new(),
            checks: Vec::new(),
        };

        // ---------------- 自选 ----------------
        b.section("自选");
        let list_w = W - PAD * 2.0;
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(PAD, 0.0, list_w, LIST_H));
        scroll.setHasVerticalScroller(true);
        scroll.setBorderType(NSBorderType::LineBorder);
        scroll.setDrawsBackground(true);
        let doc_h = LIST_H;
        let list_doc = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, list_w, doc_h));
        scroll.setDocumentView(Some(&list_doc));
        b.at(scroll.as_ref(), PAD, list_w, LIST_H);
        b.advance(LIST_H, 6.0);

        let add_field =
            NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(PAD, 0.0, 210.0, 24.0));
        add_field.setPlaceholderString(Some(&ns("600519 / 00700 / AAPL / sh600519")));
        add_field.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        add_field.setTag(0);
        wire_action(add_field.as_ref(), target, sel!(settingsAddStock:));
        b.at(add_field.as_ref(), PAD, 210.0, 24.0);

        let add_btn = push_button(&ns("添加"), target, sel!(settingsAddStock:));
        b.at(add_btn.as_ref(), PAD + 218.0, 64.0, 24.0);

        let hint = make_label(&NSFont::systemFontOfSize(11.0));
        hint.setStringValue(&ns("名称会自动补全；列表里用 ↑ ↓ 调顺序"));
        hint.setTextColor(Some(&*NSColor::secondaryLabelColor()));
        b.at(hint.as_ref(), PAD + 292.0, 300.0, 20.0);
        b.advance(24.0, 4.0);

        // 候选行：填名称 / 拼音时实时给候选，选一个点「添加」或直接回车
        let cand_label = make_label(&NSFont::systemFontOfSize(12.5));
        cand_label.setStringValue(&ns("候选"));
        b.at(cand_label.as_ref(), PAD, 40.0, 20.0);
        let cand_popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(0.0, 0.0, 500.0, 22.0),
            false,
        );
        cand_popup.addItemWithTitle(&ns("（无候选）"));
        b.at(cand_popup.as_ref(), PAD + 44.0, 500.0, 22.0);
        b.advance(22.0, 6.0);

        // ---------------- 外观 ----------------
        b.section("外观与布局");
        b.checks_inline(
            &[
                (CheckKind::ShowPrice, cfg.show_price),
                (CheckKind::ShowPct, cfg.show_pct),
                (CheckKind::BarColor, cfg.bar_color),
            ],
            target,
        );
        b.checks_inline(
            &[
                (CheckKind::Marquee, cfg.marquee),
                (CheckKind::MarketHoursOnly, cfg.market_hours_only),
            ],
            target,
        );

        // 涨跌配色
        let palette = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(CTRL_X, 0.0, CTRL_W, 24.0),
            false,
        );
        for t in [
            "红涨绿跌（A 股习惯）",
            "绿涨红跌（美股习惯）",
            "橙涨蓝跌",
            "自定义（保留原色）",
        ] {
            palette.addItemWithTitle(&ns(t));
        }
        palette.setTag(0);
        wire_action(palette.as_ref(), target, sel!(settingsSliderChanged:));
        b.row("涨跌配色", palette.as_ref(), CTRL_W, 24.0);

        for kind in [
            SliderKind::MarqueeStep,
            SliderKind::BarWidth,
            SliderKind::BarFont,
            SliderKind::PanelWidth,
            SliderKind::PanelFont,
        ] {
            b.slider(kind, target);
        }

        // ---------------- 刷新与数据源 ----------------
        b.section("刷新与数据源");
        for kind in [
            SliderKind::RefreshSecs,
            SliderKind::HoverDelay,
            SliderKind::HideDelay,
            SliderKind::MaxRows,
        ] {
            b.slider(kind, target);
        }
        b.checks_inline(&[(CheckKind::PreferTencent, cfg.prefer_tencent)], target);

        // ---------------- 底部按钮 ----------------
        b.advance(10.0, 0.0);
        let buttons_y = b.y - 28.0;
        let reset = push_button(&ns("恢复默认"), target, sel!(settingsReset:));
        content.addSubview(reset.as_ref());
        reset.setFrame(rect(PAD, buttons_y, 96.0, 28.0));

        let cancel = push_button(&ns("取消"), target, sel!(settingsCancel:));
        content.addSubview(cancel.as_ref());
        cancel.setFrame(rect(W - PAD - 76.0 - 96.0, buttons_y, 76.0, 28.0));

        let save = push_button(&ns("保存"), target, sel!(settingsSave:));
        content.addSubview(save.as_ref());
        save.setFrame(rect(W - PAD - 96.0, buttons_y, 96.0, 28.0));
        b.y = buttons_y - 10.0;

        let height = (b.y.abs() + 8.0).max(400.0);

        // 顶部对齐：把内容整体上移，使 y=0 为窗口顶部
        let shift = height;
        for sub in content.subviews().iter() {
            let mut f = sub.frame();
            f.origin.y += shift;
            sub.setFrame(f);
        }
        content.setFrame(rect(0.0, 0.0, W, height));

        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, W, height),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                objc2_app_kit::NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&ns(&format!("{DISPLAY_NAME} 设置")));
        window.setContentView(Some(&content));
        unsafe { window.setReleasedWhenClosed(false) };

        let s = Self {
            window,
            content: content.clone(),
            cand_popup,
            candidates: RefCell::new(Vec::new()),
            cand_query: RefCell::new(String::new()),
            sliders: b.sliders,
            checks: b.checks,
            palette,
            list_doc,
            list_w,
            rows: RefCell::new(Vec::new()),
            stocks: RefCell::new(cfg.stocks.clone()),
            add_field,
            hint,
        };
        s.populate(cfg);
        s.rebuild_list(target);
        s
    }

    /// 把配置写进控件
    pub fn populate(&self, cfg: &Config) {
        for row in &self.sliders {
            let (min, max, _, _, _) = row.kind.spec();
            row.slider.setMinValue(min);
            row.slider.setMaxValue(max);
            let v = match row.kind {
                SliderKind::RefreshSecs => cfg.refresh_secs as f64,
                SliderKind::MarqueeStep => cfg.marquee_step as f64,
                SliderKind::BarWidth => cfg.bar_width,
                SliderKind::BarFont => cfg.bar_font_size,
                SliderKind::PanelWidth => cfg.panel_width,
                SliderKind::PanelFont => cfg.panel_font_size,
                SliderKind::MaxRows => cfg.max_rows as f64,
                SliderKind::HoverDelay => cfg.hover_delay_ms as f64,
                SliderKind::HideDelay => cfg.hide_delay_ms as f64,
            };
            row.slider.setDoubleValue(v.clamp(min, max));
            row.value
                .setStringValue(&ns(&row.kind.format(row.slider.doubleValue())));
        }
        for row in &self.checks {
            let on = match row.kind {
                CheckKind::ShowPrice => cfg.show_price,
                CheckKind::ShowPct => cfg.show_pct,
                CheckKind::BarColor => cfg.bar_color,
                CheckKind::Marquee => cfg.marquee,
                CheckKind::MarketHoursOnly => cfg.market_hours_only,
                CheckKind::PreferTencent => cfg.prefer_tencent,
            };
            row.button.setState(if on {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        self.palette.selectItemAtIndex(palette_index(cfg));
        *self.stocks.borrow_mut() = cfg.stocks.clone();
        self.hint
            .setStringValue(&ns("名称会自动补全；列表里用 ↑ ↓ 调顺序"));
    }

    /// 从控件读回配置
    pub fn read_config(&self, base: &Config) -> Config {
        let mut cfg = base.clone();
        for row in &self.sliders {
            let v = row.slider.doubleValue();
            match row.kind {
                SliderKind::RefreshSecs => cfg.refresh_secs = v.round().max(1.0) as u64,
                SliderKind::MarqueeStep => cfg.marquee_step = v.round().max(1.0) as usize,
                SliderKind::BarWidth => cfg.bar_width = v.round().max(80.0),
                SliderKind::BarFont => cfg.bar_font_size = v.round().max(8.0),
                SliderKind::PanelWidth => cfg.panel_width = v.round().max(200.0),
                SliderKind::PanelFont => cfg.panel_font_size = v.round().max(8.0),
                SliderKind::MaxRows => cfg.max_rows = v.round().max(1.0) as usize,
                SliderKind::HoverDelay => cfg.hover_delay_ms = v.round().max(0.0) as u64,
                SliderKind::HideDelay => cfg.hide_delay_ms = v.round().max(0.0) as u64,
            }
        }
        for row in &self.checks {
            let on = row.button.state() == NSControlStateValueOn;
            match row.kind {
                CheckKind::ShowPrice => cfg.show_price = on,
                CheckKind::ShowPct => cfg.show_pct = on,
                CheckKind::BarColor => cfg.bar_color = on,
                CheckKind::Marquee => cfg.marquee = on,
                CheckKind::MarketHoursOnly => cfg.market_hours_only = on,
                CheckKind::PreferTencent => cfg.prefer_tencent = on,
            }
        }
        match self.palette.indexOfSelectedItem() {
            0 => {
                cfg.up_color = "#ff5c5c".into();
                cfg.down_color = "#35c46a".into();
            }
            1 => {
                cfg.up_color = "#35c46a".into();
                cfg.down_color = "#ff5c5c".into();
            }
            2 => {
                cfg.up_color = "#ff9f0a".into();
                cfg.down_color = "#3b82f6".into();
            }
            _ => {}
        }
        cfg.stocks = self.stocks.borrow().clone();
        cfg
    }

    /// 输入框当前内容（不清空）
    pub fn add_input(&self) -> String {
        self.add_field.stringValue().to_string().trim().to_string()
    }

    pub fn clear_add_input(&self) {
        self.add_field.setStringValue(&ns(""));
    }

    /// 更新候选下拉；`query` 是产生这批候选的关键词
    pub fn set_candidates_for(&self, query: &str, list: &[Candidate]) {
        self.cand_popup.removeAllItems();
        let mut store = self.candidates.borrow_mut();
        store.clear();
        *self.cand_query.borrow_mut() = query.to_string();
        if list.is_empty() {
            self.cand_popup.addItemWithTitle(&ns("（无候选）"));
            return;
        }
        for c in list {
            self.cand_popup.addItemWithTitle(&ns(&c.label()));
        }
        store.extend_from_slice(list);
        self.cand_popup.selectItemAtIndex(0);
    }

    /// 只有「候选对应的关键词」和当前输入一致时才认这个选择，避免用上一次的残留
    pub fn selected_candidate_for(&self, query: &str) -> Option<Candidate> {
        if self.cand_query.borrow().as_str() != query {
            return None;
        }
        let idx = self.cand_popup.indexOfSelectedItem();
        if idx < 0 {
            return None;
        }
        self.candidates.borrow().get(idx as usize).cloned()
    }

    /// 当前候选数量
    pub fn candidate_count(&self) -> usize {
        self.candidates.borrow().len()
    }

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    /// 内容视图（调试快照用）
    pub fn content_view(&self) -> Retained<NSView> {
        self.content.clone()
    }

    /// 调试用：模拟往添加框里打字
    pub fn set_string_for_debug(&self, text: &str) {
        self.add_field.setStringValue(&ns(text));
    }

    /// 调试用：当前编辑中的自选
    pub fn debug_stocks(&self) -> Vec<Stock> {
        self.stocks.borrow().clone()
    }

    /// 调试用：把状态栏宽度改成 240、关掉「显示涨跌幅」，用来验证保存链路
    pub fn debug_bump(&self) {
        if let Some(row) = self.sliders.iter().find(|r| r.kind == SliderKind::BarWidth) {
            row.slider.setDoubleValue(240.0);
        }
        if let Some(row) = self.checks.iter().find(|r| r.kind == CheckKind::ShowPct) {
            row.button.setState(NSControlStateValueOff);
        }
    }

    /// 往返自检：先确认「控件读回来 == 原配置」，再故意改几个控件看是否只读到该改的项。
    /// 用来兜住 tag 映射写错、漏接线这类问题。
    pub fn selftest(&self, base: &Config) -> String {
        let mut out = String::new();
        let round = self.read_config(base);
        let mut diffs = Vec::new();
        if round.refresh_secs != base.refresh_secs {
            diffs.push(format!(
                "refresh_secs {}→{}",
                base.refresh_secs, round.refresh_secs
            ));
        }
        if round.bar_width != base.bar_width {
            diffs.push(format!("bar_width {}→{}", base.bar_width, round.bar_width));
        }
        if round.panel_width != base.panel_width {
            diffs.push(format!(
                "panel_width {}→{}",
                base.panel_width, round.panel_width
            ));
        }
        if round.marquee_step != base.marquee_step {
            diffs.push(format!(
                "marquee_step {}→{}",
                base.marquee_step, round.marquee_step
            ));
        }
        if round.max_rows != base.max_rows {
            diffs.push(format!("max_rows {}→{}", base.max_rows, round.max_rows));
        }
        if round.hover_delay_ms != base.hover_delay_ms {
            diffs.push(format!("hover_delay {}", base.hover_delay_ms));
        }
        if round.hide_delay_ms != base.hide_delay_ms {
            diffs.push(format!("hide_delay {}", base.hide_delay_ms));
        }
        if round.show_price != base.show_price || round.show_pct != base.show_pct {
            diffs.push("show_price/show_pct".into());
        }
        if round.up_color != base.up_color || round.down_color != base.down_color {
            diffs.push(format!(
                "colors {} {} (原 {} {})",
                round.up_color, round.down_color, base.up_color, base.down_color
            ));
        }
        if round.stocks.len() != base.stocks.len() {
            diffs.push(format!(
                "stocks {}→{}",
                base.stocks.len(),
                round.stocks.len()
            ));
        }
        let hint = if diffs.is_empty() {
            "一致".to_string()
        } else {
            diffs.join("; ")
        };
        out.push_str(&format!("往返：{hint}\n"));

        // 故意改：状态栏宽度 → 300、关掉显示价格、配色切「绿涨红跌」、加一只
        if let Some(row) = self.sliders.iter().find(|r| r.kind == SliderKind::BarWidth) {
            row.slider.setDoubleValue(300.0);
        }
        if let Some(row) = self.checks.iter().find(|r| r.kind == CheckKind::ShowPrice) {
            row.button.setState(NSControlStateValueOff);
        }
        self.palette.selectItemAtIndex(1);
        self.stocks.borrow_mut().push(Stock {
            secid: "1.601398".into(),
            code: "601398".into(),
            name: "工商银行".into(),
            ..Default::default()
        });
        let changed = self.read_config(base);
        out.push_str(&format!(
            "改后：bar_width={} show_price={} up={} down={} stocks={}\n",
            changed.bar_width,
            changed.show_price,
            changed.up_color,
            changed.down_color,
            changed.stocks.len()
        ));
        // 还原
        self.populate(base);
        out
    }

    /// 每个控件的实际 frame，排查布局用
    pub fn dump_frames(&self) -> String {
        let mut out = format!("content={:?}\n", self.content.frame());
        for sub in self.content.subviews().iter() {
            let f = sub.frame();
            let cls = sub.class().name().to_string_lossy().into_owned();
            let text = unsafe {
                let responds: bool = msg_send![&*sub, respondsToSelector: sel!(stringValue)];
                if responds {
                    let v: Retained<NSString> = msg_send![&*sub, stringValue];
                    v.to_string()
                } else {
                    String::new()
                }
            };
            out.push_str(&format!(
                "  {:<14} x={:>6.1} y={:>6.1} w={:>6.1} h={:>5.1}  {:?}\n",
                cls.rsplit('.').next().unwrap_or(&cls),
                f.origin.x,
                f.origin.y,
                f.size.width,
                f.size.height,
                text.chars().take(18).collect::<String>()
            ));
        }
        out
    }

    pub fn show(&self) {
        let app = objc2_app_kit::NSApplication::sharedApplication(unsafe {
            MainThreadMarker::new_unchecked()
        });
        app.activate();
        self.window.makeKeyAndOrderFront(None);
    }

    pub fn order_out(&self) {
        self.window.orderOut(None);
    }

    /// 滑块拖动：刷新旁边的数值标签
    pub fn slider_changed(&self, sender: &NSControl) {
        let tag = sender.tag();
        if let Some(row) = self.sliders.get(tag as usize) {
            let v = sender.doubleValue().round();
            row.value.setStringValue(&ns(&row.kind.format(v)));
        }
    }

    pub fn set_hint(&self, text: &str, error: bool) {
        self.hint.setStringValue(&ns(text));
        self.hint.setTextColor(Some(&*if error {
            NSColor::systemRedColor()
        } else {
            NSColor::secondaryLabelColor()
        }));
    }

    /// 添加自选：先落一行占位，名称等后台线程回填
    pub fn add_pending(&self, secid: String, code: String, name: String) -> bool {
        {
            let stocks = self.stocks.borrow();
            if stocks.iter().any(|s| s.secid == secid) {
                return false;
            }
        }
        self.stocks.borrow_mut().push(Stock {
            secid,
            code,
            name,
            ..Default::default()
        });
        true
    }

    /// 后台解析到名称后回填并重画列表
    pub fn update_name<T: objc2::Message + 'static>(&self, target: &T, secid: &str, name: &str) {
        {
            let mut stocks = self.stocks.borrow_mut();
            if let Some(s) = stocks.iter_mut().find(|s| s.secid == secid) {
                if s.name == name {
                    return;
                }
                s.name = name.to_string();
            } else {
                return;
            }
        }
        self.rebuild_list(target);
    }

    /// 行内按钮：上移 / 下移 / 删除
    pub fn stock_action<T: objc2::Message + 'static>(&self, target: &T, tag: isize, checked: bool) {
        let index = (tag / 10) as usize;
        let op = tag % 10;
        let structure_changed = {
            let mut stocks = self.stocks.borrow_mut();
            if index >= stocks.len() {
                return;
            }
            match op {
                OP_UP if index > 0 => {
                    stocks.swap(index - 1, index);
                    true
                }
                OP_DOWN if index + 1 < stocks.len() => {
                    stocks.swap(index, index + 1);
                    true
                }
                OP_DEL => {
                    stocks.remove(index);
                    true
                }
                OP_TOGGLE_BAR => {
                    // 勾选框的新状态直接来自控件，不用自己取反
                    stocks[index].in_bar = checked;
                    false
                }
                _ => false,
            }
        };
        // 增删改序要重画列表；只是切开关就保持原样（避免勾选框闪一下）
        if structure_changed {
            self.rebuild_list(target);
        }
    }

    /// 重建自选列表（行数少，直接全量重画最省心）
    pub fn rebuild_list<T: objc2::Message + 'static>(&self, target: &T) {
        for sub in self.list_doc.subviews().iter() {
            sub.removeFromSuperview();
        }
        let stocks = self.stocks.borrow().clone();
        let n = stocks.len();
        let doc_h = ((n as f64) * ROW_H).max(LIST_H);
        self.list_doc.setFrame(rect(0.0, 0.0, self.list_w, doc_h));

        let name_font = NSFont::systemFontOfSize(12.0);
        let code_font =
            unsafe { NSFont::monospacedDigitSystemFontOfSize_weight(11.0, NSFontWeightRegular) };
        let mut rows = self.rows.borrow_mut();
        rows.clear();
        for (i, s) in stocks.iter().enumerate() {
            let y = doc_h - (i as f64 + 1.0) * ROW_H;
            let name = make_label(&name_font);
            name.setStringValue(&ns(if s.name.is_empty() {
                "（取名称中…）"
            } else {
                &s.name
            }));
            name.setFrame(rect(8.0, y + 3.0, 118.0, 20.0));
            self.list_doc.addSubview(name.as_ref());

            let code = make_label(&code_font);
            code.setStringValue(&ns(&s.code));
            code.setTextColor(Some(&*NSColor::secondaryLabelColor()));
            code.setFrame(rect(130.0, y + 4.0, 88.0, 20.0));
            self.list_doc.addSubview(code.as_ref());

            // 「滚动栏」开关：取消勾选 = 只在悬停面板里显示
            let toggle = NSButton::initWithFrame(
                NSButton::alloc(MainThreadMarker::new().unwrap()),
                rect(self.list_w - 8.0 - 3.0 * 34.0 - 96.0, y + 3.0, 92.0, 20.0),
            );
            toggle.setButtonType(NSButtonType::Switch);
            toggle.setTitle(&ns("滚动栏"));
            toggle.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            toggle.setTag(i as isize * 10 + OP_TOGGLE_BAR);
            toggle.setState(if s.in_bar {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            toggle.setToolTip(Some(&*ns(
                "勾选 = 出现在状态栏滚动里；取消 = 只在悬停展开的面板里显示",
            )));
            wire_action(toggle.as_ref(), target, sel!(settingsStockAction:));
            self.list_doc.addSubview(toggle.as_ref());

            // 「仅面板」的标的把名字调淡，一眼能看出来
            if !s.in_bar {
                name.setTextColor(Some(&*NSColor::tertiaryLabelColor()));
            }

            let bx = self.list_w - 8.0 - 3.0 * 34.0;
            for (k, (title, op)) in [("▲", OP_UP), ("▼", OP_DOWN), ("✕", OP_DEL)]
                .iter()
                .enumerate()
            {
                // 无边框按钮：标题直接铺满 frame，小尺寸下不会被 bezel 挤成小点
                let btn = plain_button(&ns(title), target, sel!(settingsStockAction:));
                btn.setTag(i as isize * 10 + *op);
                btn.setFrame(rect(bx + k as f64 * 34.0, y + 2.0, 30.0, 22.0));
                self.list_doc.addSubview(btn.as_ref());
            }
            rows.push(StockRow { name, code });
        }
    }
}

fn palette_index(cfg: &Config) -> isize {
    let up = cfg.up_color.to_ascii_lowercase();
    let down = cfg.down_color.to_ascii_lowercase();
    let is = |a: &str, b: &str, c: &str, d: &str| up == a && down == b || up == c && down == d;
    if is("#ff5c5c", "#35c46a", "#d94a45", "#2f9e5f") {
        0
    } else if is("#35c46a", "#ff5c5c", "#2f9e5f", "#d94a45") {
        1
    } else if up == "#ff9f0a" && down == "#3b82f6" {
        2
    } else {
        3
    }
}

// ---------------------------------------------------------------- 构建辅助

struct Builder {
    content: Retained<NSView>,
    /// 游标 = 下一行的**顶部**，从 0 往下变负
    y: f64,
    sliders: Vec<SliderRow>,
    checks: Vec<CheckRow>,
}

impl Builder {
    /// 在当前游标处放控件（不动游标）
    fn at(&self, view: &NSView, x: f64, w: f64, h: f64) {
        view.setFrame(rect(x, self.y - h, w, h));
        self.content.addSubview(view);
    }

    /// 游标移到当前行下方（h = 行高，gap = 与下一行的间距）
    fn advance(&mut self, h: f64, gap: f64) {
        self.y -= h + gap;
    }

    fn section(&mut self, title: &str) {
        self.advance(8.0, 0.0);
        let font = unsafe { NSFont::systemFontOfSize_weight(13.0, NSFontWeightBold) };
        let label = make_label(&font);
        label.setStringValue(&ns(title));
        self.at(label.as_ref(), LABEL_X, 300.0, 20.0);
        self.advance(20.0, 4.0);
    }

    /// 左侧标题 + 控件
    fn row(&mut self, label: &str, ctrl: &NSControl, ctrl_w: f64, h: f64) {
        let lab = make_label(&NSFont::systemFontOfSize(12.5));
        lab.setStringValue(&ns(label));
        self.at(lab.as_ref(), LABEL_X, CTRL_X - LABEL_X - 8.0, h);
        self.at(ctrl, CTRL_X, ctrl_w, h);
        self.advance(h, 6.0);
    }

    fn slider<T: objc2::Message + 'static>(&mut self, kind: SliderKind, target: &T) {
        let (min, max, ticks, _, _) = kind.spec();
        let slider = NSSlider::initWithFrame(
            NSSlider::alloc(MainThreadMarker::new().unwrap()),
            rect(CTRL_X, 0.0, CTRL_W, 20.0),
        );
        slider.setMinValue(min);
        slider.setMaxValue(max);
        // 不画刻度线：刻度一多就糊成一片，整数语义由 read_config 里的 round 保证
        let _ = ticks;
        slider.setContinuous(true);
        slider.setTag(self.sliders.len() as isize);
        wire_action(slider.as_ref(), target, sel!(settingsSliderChanged:));

        let value = make_label(&NSFont::systemFontOfSize(12.0));
        value.setAlignment(NSTextAlignment::Right);

        let lab = make_label(&NSFont::systemFontOfSize(12.5));
        lab.setStringValue(&ns(kind.label()));
        self.at(lab.as_ref(), LABEL_X, CTRL_X - LABEL_X - 8.0, 20.0);
        self.at(slider.as_ref(), CTRL_X, CTRL_W, 20.0);
        self.at(value.as_ref(), VAL_X, VAL_W, 20.0);
        self.advance(20.0, 6.0);

        self.sliders.push(SliderRow {
            slider,
            value,
            kind,
        });
    }

    /// 一行放 N 个勾选框
    fn checks_inline<T: objc2::Message + 'static>(
        &mut self,
        items: &[(CheckKind, bool)],
        target: &T,
    ) {
        let avail = W - CTRL_X - PAD;
        let gap = 8.0;
        let n = items.len().max(1) as f64;
        let w = (avail - gap * (n - 1.0)) / n;
        for (k, (kind, on)) in items.iter().enumerate() {
            let btn = NSButton::initWithFrame(
                NSButton::alloc(MainThreadMarker::new().unwrap()),
                rect(CTRL_X + k as f64 * (w + gap), self.y - 20.0, w, 20.0),
            );
            btn.setButtonType(NSButtonType::Switch);
            btn.setTitle(&ns(kind.label()));
            btn.setFont(Some(&NSFont::systemFontOfSize(12.5)));
            btn.setState(if *on {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            btn.setTag(self.checks.len() as isize);
            wire_action(btn.as_ref(), target, sel!(settingsSliderChanged:));
            self.content.addSubview(btn.as_ref());
            self.checks.push(CheckRow {
                button: btn,
                kind: *kind,
            });
        }
        self.advance(20.0, 6.0);
    }
}

fn make_label(font: &NSFont) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&ns(""), MainThreadMarker::new().unwrap());
    field.setFont(Some(font));
    field.setTextColor(Some(&*NSColor::labelColor()));
    field.setBordered(false);
    field.setDrawsBackground(false);
    field.setEditable(false);
    field.setSelectable(false);
    field
}

/// 无边框按钮（只有标题，用于列表行内的 ▲▼✕）
fn plain_button(title: &NSString, target: &impl objc2::Message, action: Sel) -> Retained<NSButton> {
    let btn: Retained<NSButton> = unsafe {
        msg_send![
            objc2::class!(NSButton),
            buttonWithTitle: title,
            target: target,
            action: action
        ]
    };
    btn.setBordered(false);
    btn.setFont(Some(&NSFont::systemFontOfSize(14.0)));
    btn.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    btn
}

fn push_button(title: &NSString, target: &impl objc2::Message, action: Sel) -> Retained<NSButton> {
    // 用 AppKit 的工厂方法建，bezel/字体/对齐都由系统配好；
    // 手搓 initWithFrame + setBezelStyle 在小尺寸下标题会被压得极小
    let btn: Retained<NSButton> = unsafe {
        msg_send![
            objc2::class!(NSButton),
            buttonWithTitle: title,
            target: target,
            action: action
        ]
    };
    btn.setFont(Some(&NSFont::systemFontOfSize(12.5)));
    btn.setFrame(rect(0.0, 0.0, 80.0, 24.0));
    btn
}

fn wire_action(control: &NSControl, target: &impl objc2::Message, action: Sel) {
    unsafe {
        let _: () = msg_send![control, setTarget: target];
        let _: () = msg_send![control, setAction: action];
    }
}

/// 给 Accessory 应用装一个最小主菜单：只为了让文本框的 ⌘C/⌘V/⌘A 能用
pub fn install_edit_menu() {
    let mtm = MainThreadMarker::new().expect("主菜单需要在主线程装");
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let main = NSMenu::new(mtm);

    let app_item = NSMenuItem::new(mtm);
    main.addItem(&app_item);
    let app_menu = NSMenu::new(mtm);
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &ns(&format!("退出 {DISPLAY_NAME}")),
            Some(sel!(terminate:)),
            &ns("q"),
        )
    };
    app_menu.addItem(&quit);
    app_item.setSubmenu(Some(&app_menu));

    let edit_item = NSMenuItem::new(mtm);
    main.addItem(&edit_item);
    let edit_menu = NSMenu::new(mtm);
    for (title, action, key) in [
        ("剪切", sel!(cut:), "x"),
        ("复制", sel!(copy:), "c"),
        ("粘贴", sel!(paste:), "v"),
        ("全选", sel!(selectAll:), "a"),
    ] {
        // target 留空 → 沿响应链找到第一响应者（即当前文本框）
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &ns(title),
                Some(action),
                &ns(key),
            )
        };
        edit_menu.addItem(&item);
    }
    edit_item.setSubmenu(Some(&edit_menu));

    app.setMainMenu(Some(&main));
}
