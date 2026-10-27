//! 行情抓取。
//!
//! 两个数据源、自动降级：
//! 1. **腾讯** `qt.gtimg.cn` —— 一个请求打包全部标的，实测最稳（A 股 / 北交所 / ETF / 指数 / 港股 / 美股全覆盖）
//! 2. **东财** `push2` 主机链 —— 主域名被限流时自动切镜像（`1.push2` / `7.push2` / `push2delay` …），
//!    并对刚失败的域名做冷却，避免反复撞墙
//!
//! 东财对单 IP 限流很凶（连接直接 RST、不发 `close_notify`），所以这里的失败处理必须当作常态。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::config::Config;

const EM_PATH: &str = "/api/qt/ulist.np/get";
/// 东财 suggest 接口的公开 token（网页版同款）
const SUGGEST_TOKEN: &str = "D43BF722C8E33BDC906FB84D85E326E8";
/// 行情链路支持的市场前缀（腾讯侧能映射的都在这里）
const SUPPORTED_MARKETS: [&str; 6] = ["1", "0", "116", "105", "106", "107"];
const EM_FIELDS: &str = "f2,f3,f4,f12,f13,f14,f15,f16,f17,f18";
/// 单请求最多带多少只
const BATCH: usize = 50;
/// 某域名失败后的冷却时间
const HOST_COOLDOWN: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Default)]
pub struct Quote {
    pub secid: String,
    pub code: String,
    pub name: String,
    pub price: Option<f64>,
    pub pct: Option<f64>,
    pub change: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub open: Option<f64>,
    pub prev_close: Option<f64>,
    /// 行情时间（源里带的原始字符串，拿不到就空）
    pub time: String,
}

impl Quote {
    /// `None` = 平盘 / 无数据；`Some(true)` = 上涨
    pub fn rising(&self) -> Option<bool> {
        let pct = self.pct?;
        if pct > 0.0 {
            Some(true)
        } else if pct < 0.0 {
            Some(false)
        } else {
            None
        }
    }

    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.code.clone()
        } else {
            self.name.clone()
        }
    }

    /// 悬停 tooltip：带上高/低/开/昨收，把不常用的字段用起来
    pub fn detail_tooltip(&self) -> String {
        let mut parts = vec![self.display_name()];
        let mut push = |k: &str, v: Option<f64>| {
            if let Some(v) = v {
                parts.push(format!("{k} {}", fmt_price(v)));
            }
        };
        push("最新", self.price);
        push("涨跌", self.change);
        push("最高", self.high);
        push("最低", self.low);
        push("今开", self.open);
        push("昨收", self.prev_close);
        let mut out = parts.join("  ");
        if !self.time.is_empty() {
            out.push_str(&format!("\n{}", self.time));
        }
        out
    }
}

/// 搜索候选
#[derive(Debug, Clone)]
pub struct Candidate {
    pub secid: String,
    pub code: String,
    pub name: String,
    /// 展示用市场标签，例如「沪A」「港股」「美股」
    pub market: String,
}

impl Candidate {
    /// 下拉里显示的一行：`贵州茅台  600519  沪A`
    pub fn label(&self) -> String {
        format!("{}  {}  {}", self.name, self.code, self.market)
    }
}

/// secid 的市场是否在支持范围内（否则行情链路拿不到数据，别让它进自选）
pub fn is_supported_market(secid: &str) -> bool {
    secid
        .split_once('.')
        .map(|(m, _)| SUPPORTED_MARKETS.contains(&m))
        .unwrap_or(false)
}

pub struct Outcome {
    pub quotes: Vec<Quote>,
    /// 实际命中的数据源，展示在面板表头
    pub source: String,
}

/// 北京时间，A 股交易时段（含集合竞价与收盘缓冲）
pub fn in_trading_hours() -> bool {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64
        + 8 * 3600;
    let day = secs.div_euclid(86400);
    let dow = (day + 4).rem_euclid(7); // 0 = 周日
    if dow == 0 || dow == 6 {
        return false;
    }
    let t = secs.rem_euclid(86400);
    let hm = |h: i64, m: i64| h * 3600 + m * 60;
    (hm(9, 15)..=hm(11, 35)).contains(&t) || (hm(12, 55)..=hm(15, 5)).contains(&t)
}

pub fn fmt_price(p: f64) -> String {
    // 低价标的（ETF / 基金 / 仙股）多给一位，否则 1.005 会显示成 1.00
    if p < 2.0 {
        format!("{p:.3}")
    } else {
        format!("{p:.2}")
    }
}

pub fn fmt_pct(p: f64) -> String {
    format!("{}{:.2}%", if p > 0.0 { "+" } else { "" }, p)
}

#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
    /// 域名 → 冷却到期时间
    host_state: std::sync::Arc<Mutex<HashMap<String, Instant>>>,
}

impl Client {
    pub fn new() -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .build();
        let cfg = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(8)))
            .user_agent(UA)
            .tls_config(tls)
            .build();
        Self {
            agent: cfg.into(),
            host_state: Default::default(),
        }
    }

    /// 按 `prefer_tencent` + 东财主机链依次尝试
    pub fn fetch(&self, secids: &[String], cfg: &Config) -> Result<Outcome, String> {
        if secids.is_empty() {
            return Ok(Outcome {
                quotes: Vec::new(),
                source: String::new(),
            });
        }
        let mut errors: Vec<String> = Vec::new();

        if cfg.prefer_tencent {
            match self.fetch_tencent(secids) {
                Ok(q) if !q.is_empty() => {
                    return Ok(Outcome {
                        quotes: q,
                        source: "腾讯".into(),
                    });
                }
                Ok(_) => errors.push("腾讯：返回空数据".into()),
                Err(e) => errors.push(format!("腾讯：{e}")),
            }
        }

        for host in self.host_chain(&cfg.em_hosts) {
            match self.fetch_eastmoney(&host, secids, &cfg.ut) {
                Ok(q) if !q.is_empty() => {
                    self.mark_host(&host, true);
                    let name = if host.contains("push2.eastmoney") && !host.contains("delay") {
                        "东财实时"
                    } else if host.contains("push2delay") {
                        "东财延时"
                    } else {
                        "东财镜像"
                    };
                    return Ok(Outcome {
                        quotes: q,
                        source: name.into(),
                    });
                }
                Ok(_) => {
                    self.mark_host(&host, false);
                    errors.push(format!("{}：空数据", short_host(&host)));
                }
                Err(e) => {
                    self.mark_host(&host, false);
                    errors.push(format!("{}：{e}", short_host(&host)));
                }
            }
        }

        if !cfg.prefer_tencent
            && let Ok(q) = self.fetch_tencent(secids)
            && !q.is_empty()
        {
            return Ok(Outcome {
                quotes: q,
                source: "腾讯".into(),
            });
        }

        Err(errors.join("；"))
    }

    /// 冷却过的域名排到最后
    fn host_chain(&self, hosts: &[String]) -> Vec<String> {
        let now = Instant::now();
        let state = self.host_state.lock().unwrap_or_else(|e| e.into_inner());
        let (ready, cooling): (Vec<_>, Vec<_>) = hosts
            .iter()
            .partition(|h| state.get(*h).map(|t| *t <= now).unwrap_or(true));
        ready.into_iter().chain(cooling).cloned().collect()
    }

    fn mark_host(&self, host: &str, ok: bool) {
        let mut state = self.host_state.lock().unwrap_or_else(|e| e.into_inner());
        if ok {
            state.remove(host);
        } else {
            state.insert(host.to_string(), Instant::now() + HOST_COOLDOWN);
        }
    }

    // ------------------------------------------------------------ 搜索（代码 / 名称 / 拼音）

    /// 关键词 → 候选列表。先试东财 suggest（返回里直接带 secid），失败再走腾讯 smartbox。
    pub fn suggest(&self, keyword: &str, limit: usize) -> Result<Vec<Candidate>, String> {
        let kw = keyword.trim();
        if kw.is_empty() {
            return Ok(Vec::new());
        }
        let mut errors = Vec::new();

        match self.suggest_eastmoney(kw, limit) {
            Ok(list) if !list.is_empty() => return Ok(list),
            Ok(_) => {}
            Err(e) => errors.push(format!("东财搜索：{e}")),
        }
        match self.suggest_tencent(kw, limit) {
            Ok(list) if !list.is_empty() => return Ok(list),
            Ok(_) => errors.push("腾讯搜索：无匹配".into()),
            Err(e) => errors.push(format!("腾讯搜索：{e}")),
        }
        if errors.is_empty() {
            Ok(Vec::new())
        } else {
            Err(errors.join("；"))
        }
    }

    /// `searchapi.eastmoney.com/api/suggest/get`：`QuoteID` 就是 secid，最省事
    fn suggest_eastmoney(&self, keyword: &str, limit: usize) -> Result<Vec<Candidate>, String> {
        let url = format!(
            "https://searchapi.eastmoney.com/api/suggest/get?input={}&type=14&count={}&token={SUGGEST_TOKEN}",
            urlencode(keyword),
            limit.max(1) * 3
        );
        let mut resp = self
            .agent
            .get(&url)
            .header("Referer", "https://quote.eastmoney.com/")
            .call()
            .map_err(|e| format!("请求失败：{e}"))?;
        let body: SuggestResp = resp
            .body_mut()
            .read_json()
            .map_err(|e| format!("解析失败：{e}"))?;

        let rows = body.table.and_then(|t| t.data).unwrap_or_default();
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            if row.quote_id.is_empty() || row.name.is_empty() {
                continue;
            }
            // 过滤掉行情链路不支持的品种（中证指数 / 板块 / 期货 / 期权 / 英股德股…）
            // 以及港股窝轮这类噪音
            if !is_supported_market(&row.quote_id)
                || !keep_classify(&row.classify, &row.type_us)
                || !seen.insert(row.quote_id.clone())
            {
                continue;
            }
            out.push(Candidate {
                secid: row.quote_id,
                code: row.code,
                name: row.name,
                market: if row.type_name.is_empty() {
                    row.classify
                } else {
                    row.type_name
                },
            });
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// `smartbox.gtimg.cn/s3/`（GBK）：`v_hint="sh~600519~贵州茅台~gzmt~GP-A^..."`
    fn suggest_tencent(&self, keyword: &str, limit: usize) -> Result<Vec<Candidate>, String> {
        let url = format!(
            "https://smartbox.gtimg.cn/s3/?q={}&t=all",
            urlencode(keyword)
        );
        let mut resp = self
            .agent
            .get(&url)
            .header("Referer", "https://gu.qq.com/")
            .call()
            .map_err(|e| format!("请求失败：{e}"))?;
        let bytes = resp
            .body_mut()
            .read_to_vec()
            .map_err(|e| format!("读取失败：{e}"))?;
        let text = encoding_rs::GBK.decode(&bytes).0;
        let mut out = parse_smartbox(&text);
        out.truncate(limit);
        Ok(out)
    }

    // ------------------------------------------------------------ 腾讯

    /// `https://qt.gtimg.cn/q=sh600519,sz300750,...`（GBK 编码）
    fn fetch_tencent(&self, secids: &[String]) -> Result<Vec<Quote>, String> {
        let pairs: Vec<(String, String)> = secids
            .iter()
            .filter_map(|s| tencent_symbol(s).map(|sym| (sym, s.clone())))
            .collect();
        if pairs.is_empty() {
            return Err("没有可转成腾讯代码的标的".into());
        }

        let mut out = Vec::with_capacity(secids.len());
        for chunk in pairs.chunks(BATCH) {
            let list = chunk
                .iter()
                .map(|(sym, _)| sym.clone())
                .collect::<Vec<_>>()
                .join(",");
            let url = format!("https://qt.gtimg.cn/q={list}");
            let mut resp = self
                .agent
                .get(&url)
                .header("Referer", "https://gu.qq.com/")
                .call()
                .map_err(|e| format!("请求失败：{e}"))?;
            let bytes = resp
                .body_mut()
                .read_to_vec()
                .map_err(|e| format!("读取失败：{e}"))?;
            let text = encoding_rs::GBK.decode(&bytes).0;
            out.extend(parse_tencent(&text, chunk));
        }

        // 保持入参顺序
        let mut by_secid: HashMap<String, Quote> =
            out.into_iter().map(|q| (q.secid.clone(), q)).collect();
        Ok(secids.iter().filter_map(|s| by_secid.remove(s)).collect())
    }

    // ------------------------------------------------------------ 东财

    fn fetch_eastmoney(
        &self,
        host: &str,
        secids: &[String],
        ut: &str,
    ) -> Result<Vec<Quote>, String> {
        let mut out = Vec::with_capacity(secids.len());
        let mut last_err = None;
        for chunk in secids.chunks(BATCH) {
            match self.fetch_eastmoney_chunk(host, chunk, ut) {
                Ok(mut q) => out.append(&mut q),
                Err(e) => last_err = Some(e),
            }
        }
        if out.is_empty()
            && let Some(e) = last_err
        {
            return Err(e);
        }
        Ok(out)
    }

    fn fetch_eastmoney_chunk(
        &self,
        host: &str,
        secids: &[String],
        ut: &str,
    ) -> Result<Vec<Quote>, String> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let url = format!(
            "{host}{EM_PATH}?fltt=2&invt=2&np=1&fields={EM_FIELDS}&secids={}&ut={ut}&_={ts}",
            secids.join(",")
        );
        let mut resp = self
            .agent
            .get(&url)
            .header("Referer", "https://quote.eastmoney.com/")
            .call()
            .map_err(|e| format!("请求失败：{e}"))?;
        let body: EmResp = resp
            .body_mut()
            .read_json()
            .map_err(|e| format!("解析失败：{e}"))?;

        let diff = body.data.map(|d| d.diff).unwrap_or_default();
        let mut by_secid: HashMap<String, EmRow> = HashMap::new();
        for row in diff {
            if row.f12.is_empty() {
                continue;
            }
            by_secid.insert(format!("{}.{}", row.f13, row.f12), row);
        }

        Ok(secids
            .iter()
            .filter_map(|sid| by_secid.remove(sid))
            .map(|row| {
                let code = row.f12.clone();
                Quote {
                    secid: format!("{}.{code}", row.f13),
                    code,
                    name: row.f14,
                    price: num(&row.f2),
                    pct: num(&row.f3),
                    change: num(&row.f4),
                    high: num(&row.f15),
                    low: num(&row.f16),
                    open: num(&row.f17),
                    prev_close: num(&row.f18),
                    time: String::new(),
                }
            })
            .collect())
    }
}

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

fn short_host(host: &str) -> String {
    host.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('.')
        .next()
        .unwrap_or(host)
        .to_string()
}

/// secid → 腾讯代码（`sh600519` / `sz300750` / `bj430047` / `hk00700` / `usAAPL`）
fn tencent_symbol(secid: &str) -> Option<String> {
    let (market, code) = secid.split_once('.')?;
    let sym = match market {
        "1" => format!("sh{code}"),
        "0" => {
            // 北交所（4/8 开头）在腾讯侧用 bj 前缀
            if code.starts_with('4') || code.starts_with('8') {
                format!("bj{code}")
            } else {
                format!("sz{code}")
            }
        }
        "116" => format!("hk{code}"),
        "105" | "106" | "107" => format!("us{code}"),
        _ => return None,
    };
    Some(sym)
}

/// 解析 `v_sh600519="1~贵州茅台~600519~1356.61~...";`
fn parse_tencent(text: &str, chunk: &[(String, String)]) -> Vec<Quote> {
    let map: HashMap<&str, &str> = chunk
        .iter()
        .map(|(sym, secid)| (sym.as_str(), secid.as_str()))
        .collect();
    let mut out = Vec::new();
    for piece in text.split(';') {
        let Some(rest) = piece.trim().strip_prefix("v_") else {
            continue;
        };
        let Some((sym, payload)) = rest.split_once('=') else {
            continue;
        };
        let Some(secid) = map.get(sym) else { continue };
        let payload = payload.trim().trim_matches('"');
        if payload.is_empty() {
            continue;
        }
        let f: Vec<&str> = payload.split('~').collect();
        let get = |i: usize| f.get(i).map(|s| s.trim()).filter(|s| !s.is_empty());
        out.push(Quote {
            secid: (*secid).to_string(),
            code: get(2).unwrap_or_default().to_string(),
            name: get(1).unwrap_or_default().to_string(),
            price: get(3).and_then(|s| s.parse().ok()),
            prev_close: get(4).and_then(|s| s.parse().ok()),
            open: get(5).and_then(|s| s.parse().ok()),
            time: get(30).unwrap_or_default().to_string(),
            change: get(31).and_then(|s| s.parse().ok()),
            pct: get(32).and_then(|s| s.parse().ok()),
            high: get(33).and_then(|s| s.parse().ok()),
            low: get(34).and_then(|s| s.parse().ok()),
        });
    }
    out
}

/// 百分号编码（只保留 URL 安全字符，中文按 UTF-8 逐字节编码）
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 解析腾讯 smartbox 的 `v_hint` 串
fn parse_smartbox(text: &str) -> Vec<Candidate> {
    let Some(inner) = text.split('"').nth(1) else {
        return Vec::new();
    };
    inner
        .split('^')
        .filter_map(|seg| {
            let f: Vec<&str> = seg.split('~').collect();
            if f.len() < 3 {
                return None;
            }
            let (prefix, raw_code, name) = (f[0], f[1], f[2]);
            // 第 5 段是类型：GP-A 沪深正股 / GP 港美正股 / ETF / ZS 指数；
            // QZ 窝轮、KZZ 可转债之类直接丢掉
            let kind = f.get(4).copied().unwrap_or("");
            if !matches!(kind, "GP-A" | "GP" | "ETF" | "ZS" | "GP-B") {
                return None;
            }
            let market_label;
            let (secid, code) = match prefix {
                "sh" => {
                    market_label = "沪A";
                    (format!("1.{raw_code}"), raw_code.to_string())
                }
                "sz" => {
                    market_label = "深A";
                    (format!("0.{raw_code}"), raw_code.to_string())
                }
                "bj" => {
                    market_label = "北交所";
                    (format!("0.{raw_code}"), raw_code.to_string())
                }
                "hk" => {
                    market_label = "港股";
                    (format!("116.{raw_code}"), raw_code.to_string())
                }
                "us" => {
                    // 腾讯给的是 `aapl.oq`，后缀代表交易所
                    let (sym, suffix) = raw_code.split_once('.').unwrap_or((raw_code, ""));
                    let m = match suffix {
                        "n" => "106",
                        "am" => "107",
                        _ => "105",
                    };
                    market_label = "美股";
                    (
                        format!("{m}.{}", sym.to_ascii_uppercase()),
                        sym.to_ascii_uppercase(),
                    )
                }
                _ => return None,
            };
            if !is_supported_market(&secid) {
                return None;
            }
            Some(Candidate {
                secid,
                code,
                name: name.to_string(),
                market: market_label.into(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------- 东财 JSON

#[derive(Deserialize)]
struct SuggestResp {
    #[serde(rename = "QuotationCodeTable")]
    table: Option<SuggestTable>,
}

#[derive(Deserialize)]
struct SuggestTable {
    #[serde(rename = "Data")]
    data: Option<Vec<SuggestRow>>,
}

#[derive(Deserialize)]
struct SuggestRow {
    #[serde(rename = "Code", default)]
    code: String,
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "QuoteID", default)]
    quote_id: String,
    #[serde(rename = "SecurityTypeName", default)]
    type_name: String,
    #[serde(rename = "Classify", default)]
    classify: String,
    #[serde(rename = "TypeUS", default)]
    type_us: String,
}

/// 只留「能盯、值得盯」的品种。
///
/// 东财的 `Classify` 给出大类（AStock / HK / UsStock / Index / Fund / LSE / Futures / BK…），
/// 但港股窝轮的 `Classify` 也是 `HK`，要靠 `TypeUS == "6"` 区分掉，
/// 否则搜「宁德」会被一堆「宁德摩通六乙沽A」刷屏。
fn keep_classify(classify: &str, type_us: &str) -> bool {
    match classify {
        "AStock" | "UsStock" | "Index" | "Fund" => true,
        "HK" => type_us != "6",
        _ => false,
    }
}

/// 东财用 `"-"` 表示停牌 / 无数据
fn num(v: &Option<Value>) -> Option<f64> {
    match v {
        Some(Value::Num(n)) => Some(*n),
        Some(Value::Str(s)) => s.parse::<f64>().ok(),
        _ => None,
    }
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum Value {
    Num(f64),
    Str(String),
}

#[derive(Deserialize)]
struct EmResp {
    data: Option<EmData>,
}

#[derive(Deserialize)]
struct EmData {
    #[serde(default)]
    diff: Vec<EmRow>,
}

#[derive(Deserialize)]
struct EmRow {
    #[serde(default)]
    f2: Option<Value>,
    #[serde(default)]
    f3: Option<Value>,
    #[serde(default)]
    f4: Option<Value>,
    #[serde(default)]
    f12: String,
    #[serde(default)]
    f13: i64,
    #[serde(default)]
    f14: String,
    #[serde(default)]
    f15: Option<Value>,
    #[serde(default)]
    f16: Option<Value>,
    #[serde(default)]
    f17: Option<Value>,
    #[serde(default)]
    f18: Option<Value>,
}

/// 只为补名称（`tickerstrip add` 用）
pub fn resolve_name(client: &Client, secid: &str, cfg: &Config) -> Option<String> {
    client
        .fetch(&[secid.to_string()], cfg)
        .ok()
        .and_then(|o| o.quotes.into_iter().next())
        .map(|q| q.name)
        .filter(|n| !n.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trading_hours_shape() {
        // 只验证不 panic 且返回 bool（真实时段依赖当前时间）
        let _ = in_trading_hours();
    }

    #[test]
    fn symbol_mapping() {
        assert_eq!(tencent_symbol("1.600519").unwrap(), "sh600519");
        assert_eq!(tencent_symbol("0.300750").unwrap(), "sz300750");
        assert_eq!(tencent_symbol("0.430047").unwrap(), "bj430047");
        assert_eq!(tencent_symbol("116.00700").unwrap(), "hk00700");
        assert_eq!(tencent_symbol("105.AAPL").unwrap(), "usAAPL");
        assert!(tencent_symbol("999.X").is_none());
    }

    #[test]
    fn smartbox_parsing() {
        let text = r#"v_hint="sh~600519~贵州茅台~gzmt~GP-A^sz~300750~宁德时代~ndsd~GP-A^hk~00700~腾讯控股~txkg~GP^us~aapl.oq~苹果~pg~GP^us~baba.n~阿里巴巴~albb~GP^90~BK1189~腾讯云~txy~BK^2~H30263~腾讯济安~txja~24^hk~13112~宁德摩通六乙沽A~ndmtlyga~QZ""#;
        let list = parse_smartbox(text);
        assert_eq!(list.len(), 5); // 板块(90) 与中证指数(2) 被过滤掉
        assert_eq!(list[0].secid, "1.600519");
        assert_eq!(list[0].name, "贵州茅台");
        assert_eq!(list[2].secid, "116.00700");
        assert_eq!(list[3].secid, "105.AAPL"); // .oq → 纳斯达克
        assert_eq!(list[3].code, "AAPL");
        assert_eq!(list[4].secid, "106.BABA"); // .n → 纽交所
    }

    #[test]
    fn classify_filter() {
        assert!(keep_classify("AStock", "2"));
        assert!(keep_classify("Index", "1"));
        assert!(keep_classify("Fund", "9"));
        assert!(keep_classify("HK", "3")); // 港股正股
        assert!(!keep_classify("HK", "6")); // 港股窝轮
        assert!(!keep_classify("LSE", "7"));
        assert!(!keep_classify("Futures", "19"));
        assert!(!keep_classify("BK", "3"));
        assert!(!keep_classify("Bond", "4"));
    }

    #[test]
    fn market_filter() {
        assert!(is_supported_market("1.600519"));
        assert!(is_supported_market("116.00700"));
        assert!(is_supported_market("107.AAPW"));
        assert!(!is_supported_market("2.H30263")); // 中证指数
        assert!(!is_supported_market("90.BK1189")); // 板块
        assert!(!is_supported_market("115.AP611")); // 期货
        assert!(!is_supported_market("399001")); // 没带市场前缀
    }

    #[test]
    fn url_encode() {
        assert_eq!(urlencode("600519"), "600519");
        assert_eq!(urlencode("AAPL"), "AAPL");
        assert_eq!(urlencode("茅台"), "%E8%8C%85%E5%8F%B0");
    }

    #[test]
    fn parse_tencent_line() {
        let text = r#"v_sh600519="1~贵州茅台~600519~1356.61~1349.98~1343.88~31996~19548~12428~1356.60~8~1356.59~2~1356.58~9~1356.54~2~1356.53~8~1356.61~1~1356.63~2~1356.65~2~1356.68~5~1356.69~3~~20261027144137~6.63~0.49~1357.00~1338.01~1356.61/31996/4321666252";"#;
        let chunk = vec![("sh600519".to_string(), "1.600519".to_string())];
        let q = parse_tencent(text, &chunk);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].name, "贵州茅台");
        assert_eq!(q[0].price, Some(1356.61));
        assert_eq!(q[0].pct, Some(0.49));
        assert_eq!(q[0].change, Some(6.63));
        assert_eq!(q[0].high, Some(1357.00));
        assert_eq!(q[0].low, Some(1338.01));
        assert_eq!(q[0].prev_close, Some(1349.98));
    }
}
