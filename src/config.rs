//! 配置读写。默认落在 `~/Library/Application Support/tickerstrip/config.json`。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 程序名（仓库名 / crate 名 / 二进制名 / 配置目录名）
pub const APP_NAME: &str = "tickerstrip";
/// 界面里显示的中文名
pub const DISPLAY_NAME: &str = "摸鱼盯盘";
/// 改名前的旧配置目录名，用于一次性搬迁自选与设置
const LEGACY_DIR_NAME: &str = "mbquote";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stock {
    /// 东财 secid，例如 `1.600519`、`0.300750`、`116.00700`、`105.AAPL`
    pub secid: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub name: String,
    /// 是否出现在状态栏的滚动行情里。
    /// `false` = 只在悬停展开的面板里显示（老配置没这个字段时按 `true` 读）。
    #[serde(default = "yes")]
    pub in_bar: bool,
}

/// serde 默认值：老配置里缺 `in_bar` 时按「进滚动栏」处理
fn yes() -> bool {
    true
}

impl Default for Stock {
    /// 注意 `in_bar` 默认 **true**：`#[derive(Default)]` 会给 false，那是反的
    fn default() -> Self {
        Self {
            secid: String::new(),
            code: String::new(),
            name: String::new(),
            in_bar: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 自选列表（顺序即滚动顺序）
    pub stocks: Vec<Stock>,

    // ---- 抓取 ----
    /// 行情刷新间隔（秒），最小 1
    pub refresh_secs: u64,
    /// 非交易时段自动降频到 12 倍间隔（省电、也少打扰数据源）
    pub market_hours_only: bool,
    /// 东财接口的 ut 令牌（一般不用改）
    pub ut: String,
    /// 优先用腾讯行情（实测最稳），失败再走东财
    pub prefer_tencent: bool,
    /// 东财主机链（主域名被限流时依次尝试镜像）
    pub em_hosts: Vec<String>,

    // ---- 状态栏滚动 ----
    /// 是否横向滚动
    pub marquee: bool,
    /// 滚动帧间隔（毫秒）
    pub marquee_ms: u64,
    /// 每帧前进的字符数
    pub marquee_step: usize,
    /// 状态栏占用宽度（pt）
    pub bar_width: f64,
    /// 状态栏是否显示价格
    pub show_price: bool,
    /// 状态栏是否显示涨跌幅
    pub show_pct: bool,
    /// 状态栏字号
    pub bar_font_size: f64,
    /// 状态栏按涨跌上色（红涨绿跌，颜色取自 up_color / down_color）
    pub bar_color: bool,

    // ---- 悬停面板 ----
    /// 悬停多久后展开（毫秒）
    pub hover_delay_ms: u64,
    /// 移开后多久收起（毫秒）
    pub hide_delay_ms: u64,
    /// 面板宽度（pt）
    pub panel_width: f64,
    /// 面板最多显示多少行
    pub max_rows: usize,
    /// 面板字号
    pub panel_font_size: f64,

    // ---- 颜色（十六进制，中国习惯：红涨绿跌）----
    pub up_color: String,
    pub down_color: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            stocks: default_stocks(),
            refresh_secs: 5,
            market_hours_only: true,
            ut: "fa5fd1943c7b386f172d6893dbfba10b".into(),
            prefer_tencent: true,
            em_hosts: default_em_hosts(),

            marquee: true,
            marquee_ms: 130,
            marquee_step: 1,
            bar_width: 210.0,
            show_price: true,
            show_pct: true,
            bar_font_size: 12.0,
            bar_color: true,

            hover_delay_ms: 100,
            hide_delay_ms: 320,
            panel_width: 302.0,
            max_rows: 16,
            panel_font_size: 12.0,

            up_color: "#ff5c5c".into(),
            down_color: "#35c46a".into(),
        }
    }
}

/// 东财主机链：实时主站 → 实时镜像 → 延时兜底
fn default_em_hosts() -> Vec<String> {
    [
        "https://push2.eastmoney.com",
        "https://1.push2.eastmoney.com",
        "https://7.push2.eastmoney.com",
        "https://21.push2.eastmoney.com",
        "https://28.push2.eastmoney.com",
        "https://72.push2.eastmoney.com",
        "https://82.push2.eastmoney.com",
        "https://push2delay.eastmoney.com",
        "https://push2delay2.eastmoney.com",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// 首次运行给几条指数，方便立刻看到效果。
fn default_stocks() -> Vec<Stock> {
    vec![
        Stock {
            secid: "1.000001".into(),
            code: "000001".into(),
            name: "上证指数".into(),
            ..Default::default()
        },
        Stock {
            secid: "0.399001".into(),
            code: "399001".into(),
            name: "深证成指".into(),
            ..Default::default()
        },
        Stock {
            secid: "0.399006".into(),
            code: "399006".into(),
            name: "创业板指".into(),
            ..Default::default()
        },
        Stock {
            secid: "1.600519".into(),
            code: "600519".into(),
            name: "贵州茅台".into(),
            ..Default::default()
        },
        Stock {
            secid: "0.300750".into(),
            code: "300750".into(),
            name: "宁德时代".into(),
            ..Default::default()
        },
    ]
}

pub fn config_dir() -> PathBuf {
    base_dir().join(APP_NAME)
}

fn base_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// 旧版（mbquote）的配置路径，存在时才返回
fn legacy_config_path() -> Option<PathBuf> {
    let path = base_dir().join(LEGACY_DIR_NAME).join("config.json");
    path.exists().then_some(path)
}

/// 首次启动时从旧目录搬一份配置过来，别让人白加了自选
fn migrate_legacy_config() {
    let new = config_path();
    if new.exists() {
        return;
    }
    let Some(old) = legacy_config_path() else {
        return;
    };
    if let Some(dir) = new.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::copy(&old, &new) {
        Ok(_) => eprintln!("[{APP_NAME}] 已从旧配置 {} 迁移自选与设置", old.display()),
        Err(e) => eprintln!("[{APP_NAME}] 迁移旧配置失败：{e}"),
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

impl Config {
    /// 读配置；文件不存在时写入一份默认配置。
    pub fn load() -> Self {
        migrate_legacy_config();
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<Config>(&s) {
                Ok(cfg) => cfg,
                Err(e) => {
                    eprintln!(
                        "[{APP_NAME}] 配置文件解析失败（{e}），暂时使用默认配置：{}",
                        path.display()
                    );
                    Config::default()
                }
            },
            Err(_) => {
                let cfg = Config::default();
                let _ = cfg.save();
                cfg
            }
        }
    }

    /// 原子写：先落临时文件再 rename。
    /// 避免其它进程（比如同时跑的 `tickerstrip add`）读到写了一半的 JSON。
    pub fn save(&self) -> std::io::Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        let text = serde_json::to_string_pretty(self).unwrap_or_default();
        let tmp = dir.join("config.json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, config_path())
    }

    /// 只把「学到的名称」并回磁盘上的版本，**不**用内存里的整份配置覆盖磁盘。
    ///
    /// 后台补名称是自动行为，如果直接整份写回，就会把别的进程（CLI `add`）刚加的自选抹掉。
    pub fn merge_names_to_disk(&self) -> std::io::Result<()> {
        let disk = Config::load();
        let merged = merge_names(&disk.stocks, &self.stocks);
        let mut out = disk;
        out.stocks = merged;
        out.save()
    }

    pub fn refresh_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.refresh_secs.max(1))
    }
}

/// 把 `mine` 里非空的名称合并进 `disk` 的自选（以 disk 的列表为准，不增不减）
pub fn merge_names(disk: &[Stock], mine: &[Stock]) -> Vec<Stock> {
    disk.iter()
        .map(|d| {
            let mut s = d.clone();
            if let Some(m) = mine.iter().find(|m| m.secid == d.secid)
                && !m.name.is_empty()
            {
                s.name = m.name.clone();
            }
            s
        })
        .collect()
}

/// `#RRGGBB` / `#RGB` → (r, g, b)，取值 0.0~1.0
pub fn hex_to_rgb(hex: &str) -> (f64, f64, f64) {
    let h = hex.trim().trim_start_matches('#');
    let parse = |s: &str| u8::from_str_radix(s, 16).ok();
    let (r, g, b) = match h.len() {
        3 => {
            let c: Vec<Option<u8>> = h.chars().map(|c| parse(&format!("{c}{c}"))).collect();
            (c[0], c[1], c[2])
        }
        6 => (parse(&h[0..2]), parse(&h[2..4]), parse(&h[4..6])),
        _ => (None, None, None),
    };
    match (r, g, b) {
        (Some(r), Some(g), Some(b)) => (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0),
        _ => (0.5, 0.5, 0.5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(secid: &str, name: &str) -> Stock {
        Stock {
            secid: secid.into(),
            code: secid.split('.').nth(1).unwrap_or("").into(),
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn merge_names_keeps_disk_list() {
        // 磁盘上：3 只（含别的进程刚加的一只），其中一只还没名字
        let disk = vec![
            st("1.600519", "贵州茅台"),
            st("116.00700", ""),
            st("105.AAPL", "苹果"),
        ];
        // 内存里是更旧的 2 只 + 一个新学到的名称
        let mine = vec![st("1.600519", "贵州茅台"), st("116.00700", "腾讯控股")];

        let merged = merge_names(&disk, &mine);
        // 列表以磁盘为准：不能丢 105.AAPL，也不能多出内存里的其它标的
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].name, "贵州茅台");
        assert_eq!(merged[1].name, "腾讯控股"); // 名称补齐
        assert_eq!(merged[2].name, "苹果"); // 未涉及的不动
    }

    #[test]
    fn old_config_without_in_bar_still_loads() {
        // 老版本写的配置没有 in_bar 字段，读出来必须是 true（继续进滚动栏）
        let json = r#"{"stocks":[{"secid":"1.600519","code":"600519","name":"贵州茅台"}]}"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.stocks[0].in_bar);

        // 显式关掉的要保留
        let json =
            r#"{"stocks":[{"secid":"1.600519","code":"600519","name":"贵州茅台","in_bar":false}]}"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(!cfg.stocks[0].in_bar);
    }

    #[test]
    fn merge_names_ignores_empty_learned() {
        let disk = vec![st("105.AAPL", "苹果")];
        let mine = vec![st("105.AAPL", "")];
        assert_eq!(merge_names(&disk, &mine)[0].name, "苹果");
    }
}
