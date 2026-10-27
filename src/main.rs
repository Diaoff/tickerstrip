//! 摸鱼盯盘 tickerStrip — macOS 菜单栏行情小工具
//!
//! 状态栏横向滚动自选行情，鼠标悬停展开面板查看全部自选。

mod config;
mod quote;
mod secid;
mod settings;
mod ui;

use config::{Config, Stock};
use quote::Client;

const HELP: &str = "\
摸鱼盯盘 tickerStrip — macOS 菜单栏自选行情

状态栏滚动自选，鼠标悬停展开面板。仓库名 / crate 名 / 二进制名都是 tickerstrip。

用法：
  tickerstrip                     启动菜单栏（不出 Dock 图标）
  tickerstrip add [--panel-only] <代码|名称…>
                              添加自选，例如：tickerstrip add 600519 茅台 gzmt AAPL
                              --panel-only：只在悬停面板显示，不进状态栏滚动
  tickerstrip bar <代码|名称…>     切换某只是否进状态栏滚动
  tickerstrip rm  <代码…>         移除自选
  tickerstrip search <关键词>     搜代码 / 名称 / 拼音，看有哪些候选
  tickerstrip list                查看自选
  tickerstrip fetch               终端里打印一次行情（排查用）
  tickerstrip path                打印配置文件路径

代码写法（也可以直接填名称或拼音，会自动搜）：
  茅台 / gzmt   名称或拼音首字母 → 600519 贵州茅台
  600519        沪市（自动补 1. 前缀）
  000001        深市（自动补 0. 前缀）
  1.000001      上证指数（指数与深市股票代码会撞，需显式写市场前缀）
  00700         港股（5 位数字 → 116.）
  AAPL          美股（默认纳斯达克 105.，可用 105./106./107. 指定）
  sh600519 / sz000001 / hk00700 / usAAPL

配置：~/.config/tickerstrip/config.json（改完自动生效，无需重启）
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("run");

    match cmd {
        "run" | "" => ui::run(),
        "-h" | "--help" | "help" => {
            print!("{HELP}");
        }
        "path" => println!("{}", config::config_path().display()),
        "list" => list(),
        "add" => {
            if args.len() < 2 {
                eprintln!("用法：tickerstrip add <代码…>");
                std::process::exit(2);
            }
            add(&args[1..]);
        }
        "rm" | "remove" | "del" => {
            if args.len() < 2 {
                eprintln!("用法：tickerstrip rm <代码…>");
                std::process::exit(2);
            }
            remove(&args[1..]);
        }
        "bar" => {
            if args.len() < 2 {
                eprintln!("用法：tickerstrip bar <代码|名称…>   # 切换「是否出现在状态栏滚动里」");
                std::process::exit(2);
            }
            toggle_bar(&args[1..]);
        }
        "search" | "s" => {
            if args.len() < 2 {
                eprintln!("用法：tickerstrip search <关键词>");
                std::process::exit(2);
            }
            search(&args[1..]);
        }
        "fetch" | "show" => fetch_once(),
        other => {
            eprintln!("未知命令：{other}\n");
            print!("{HELP}");
            std::process::exit(2);
        }
    }
}

fn list() {
    let cfg = Config::load();
    if cfg.stocks.is_empty() {
        println!("（自选为空）");
        return;
    }
    for (i, s) in cfg.stocks.iter().enumerate() {
        println!(
            "{:>2}. {:<12} {:<10} {:<22} {}",
            i + 1,
            s.secid,
            s.code,
            truncate_display(&s.name, 10),
            if s.in_bar { "滚动栏" } else { "仅面板" }
        );
    }
    println!("\n配置文件：{}", config::config_path().display());
}

fn add(inputs: &[String]) {
    // `--panel-only`：加进来但只出现在悬停面板，不进状态栏滚动
    let panel_only = inputs.iter().any(|s| s == "--panel-only" || s == "-p");
    let items: Vec<&String> = inputs.iter().filter(|s| !s.starts_with('-')).collect();
    if items.is_empty() {
        eprintln!("用法：tickerstrip add [--panel-only] <代码|名称…>");
        std::process::exit(2);
    }
    let mut cfg = Config::load();
    let client = Client::new();
    let mut added = 0usize;

    for raw in items {
        // 先当代码解析（数字 / 带市场前缀的最确定）；纯字母要小心：
        // `gzmt` 既可能是拼音首字母，也可能是美股代码，交给搜索去判断
        let (secid, mut name) = match code_fast_path(raw) {
            Some(s) => (s, String::new()),
            None => match resolve_keyword(&client, &cfg, raw) {
                Ok(pair) => pair,
                Err(e) => {
                    eprintln!("跳过 {raw}：{e}");
                    continue;
                }
            },
        };
        if cfg.stocks.iter().any(|s| s.secid == secid) {
            println!("已存在：{secid}");
            continue;
        }
        if name.is_empty() {
            name = quote::resolve_name(&client, &secid, &cfg).unwrap_or_default();
        }
        if name.is_empty() {
            eprintln!("提示：{secid} 暂时取不到名称，稍后会自动补上");
        }
        cfg.stocks.push(Stock {
            secid: secid.clone(),
            code: secid::code_of(&secid),
            name: name.clone(),
            in_bar: !panel_only,
        });
        println!(
            "已添加：{secid} {}{}",
            if name.is_empty() {
                "（名称待补全）"
            } else {
                &name
            },
            if panel_only {
                "  ← 只在面板里显示"
            } else {
                ""
            }
        );
        added += 1;
    }

    if added > 0 {
        if let Err(e) = cfg.save() {
            eprintln!("写入配置失败：{e}");
            std::process::exit(1);
        }
        println!("\n配置：{}", config::config_path().display());
        println!("菜单栏程序会自动重载（若在运行中）。");
    }
}

/// 只有「确定是代码」的输入才走快路径，纯字母（可能是拼音）返回 None 交给搜索。
fn code_fast_path(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    // 纯字母：可能只是美股代码，也可能是拼音首字母，不做判断
    if t.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    secid::infer(t).ok()
}

/// 按名称/拼音找标的。命中唯一才自动选，多个候选列出来让用户说清楚。
fn resolve_keyword(
    client: &Client,
    cfg: &Config,
    keyword: &str,
) -> Result<(String, String), String> {
    let list = client.suggest(keyword, 8)?;
    // 关键词本身就是某个候选的代码（如 AAPL）→ 直接用它，别让拼音命中抢占
    let kw = keyword.trim().to_ascii_uppercase();
    if let Some(hit) = list.iter().find(|c| c.code.to_ascii_uppercase() == kw) {
        return Ok((hit.secid.clone(), hit.name.clone()));
    }
    match list.len() {
        0 => Err(format!(
            "没搜到「{keyword}」，试试 tickerstrip search {keyword}"
        )),
        1 => Ok((list[0].secid.clone(), list[0].name.clone())),
        _ => {
            let _ = cfg;
            eprintln!(
                "「{keyword}」匹配到 {} 个，请用更精确的关键词或直接填代码：",
                list.len()
            );
            for c in &list {
                eprintln!("    {:<12} {:<8} {}", c.secid, c.code, c.name);
            }
            Err("候选不唯一".into())
        }
    }
}

fn search(inputs: &[String]) {
    let keyword = inputs.join(" ");
    let client = Client::new();
    let cfg = Config::load();
    let _ = &cfg;
    match client.suggest(&keyword, 12) {
        Ok(list) if list.is_empty() => println!("没搜到「{keyword}」"),
        Ok(list) => {
            println!("「{keyword}」的候选：");
            for (i, c) in list.iter().enumerate() {
                println!(
                    "{:>2}. {:<12} {:<10} {:<22} {}",
                    i + 1,
                    c.secid,
                    c.code,
                    c.name,
                    c.market
                );
            }
            println!("\n添加：tickerstrip add {}", list[0].code);
        }
        Err(e) => {
            eprintln!("搜索失败：{e}");
            std::process::exit(1);
        }
    }
}

/// 切换「是否进状态栏滚动」
fn toggle_bar(inputs: &[String]) {
    let client = Client::new();
    let mut cfg = Config::load();
    let mut changed = false;
    for raw in inputs {
        let target = match code_fast_path(raw) {
            Some(s) => s,
            None => match resolve_keyword(&client, &cfg, raw) {
                Ok((secid, _)) => secid,
                Err(e) => {
                    eprintln!("跳过 {raw}：{e}");
                    continue;
                }
            },
        };
        match cfg.stocks.iter_mut().find(|s| s.secid == target) {
            Some(s) => {
                s.in_bar = !s.in_bar;
                changed = true;
                println!(
                    "{} {}  → {}",
                    s.code,
                    s.name,
                    if s.in_bar {
                        "进滚动栏"
                    } else {
                        "只在悬停面板"
                    }
                );
            }
            None => println!("不在自选里：{target}"),
        }
    }
    if changed && let Err(e) = cfg.save() {
        eprintln!("写入配置失败：{e}");
        std::process::exit(1);
    }
    println!("配置：{}", config::config_path().display());
}

fn remove(inputs: &[String]) {
    let mut cfg = Config::load();
    for raw in inputs {
        let raw_lc = raw.trim().to_ascii_lowercase();
        let secid = secid::infer(raw).unwrap_or_else(|_| raw.trim().to_string());
        let secid_lc = secid.to_ascii_lowercase();
        let before = cfg.stocks.len();
        cfg.stocks.retain(|s| {
            s.secid.to_ascii_lowercase() != secid_lc && s.code.to_ascii_lowercase() != raw_lc
        });
        if cfg.stocks.len() == before {
            println!("未找到：{secid}");
        } else {
            println!("已移除：{secid}");
        }
    }
    if let Err(e) = cfg.save() {
        eprintln!("写入配置失败：{e}");
        std::process::exit(1);
    }
}

/// 终端打印一次，用来确认接口通不通、代码对不对
fn fetch_once() {
    let cfg = Config::load();
    let client = Client::new();
    let secids: Vec<String> = cfg.stocks.iter().map(|s| s.secid.clone()).collect();
    if secids.is_empty() {
        println!("（自选为空，先 tickerstrip add 600519）");
        return;
    }
    match client.fetch(&secids, &cfg) {
        Ok(outcome) => {
            let quotes = outcome.quotes;
            println!("数据源：{}\n", outcome.source);
            println!(
                "{:<12} {:<10} {:>10} {:>9} {:>9}",
                "secid", "名称", "最新价", "涨跌幅", "涨跌额"
            );
            for q in quotes {
                println!(
                    "{:<12} {:<10} {:>10} {:>9} {:>9}",
                    q.secid,
                    truncate_display(&q.display_name(), 8),
                    q.price.map(quote::fmt_price).unwrap_or_else(|| "—".into()),
                    q.pct.map(quote::fmt_pct).unwrap_or_else(|| "—".into()),
                    q.change
                        .map(|c| format!("{c:+.2}"))
                        .unwrap_or_else(|| "—".into()),
                );
            }
        }
        Err(e) => {
            eprintln!("抓取失败：{e}");
            std::process::exit(1);
        }
    }
}

fn truncate_display(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let mut out: String = chars[..max.saturating_sub(1)].iter().collect();
    out.push('…');
    out
}
