//! 证券代码 → 东财 secid。
//!
//! 市场前缀：`1.` 上交所 / `0.` 深交所+北交所 / `116.` 港股 /
//! `105.` `106.` `107.` 美股（纳斯达克 / 纽交所 / 美交所）。

/// 把用户输入规范成 secid。
///
/// 支持：`600519`、`000001`、`00700`（港）、`AAPL`、`1.600519`、
/// `sh600519`、`sz000001`、`hk00700`、`bj430047`、`usAAPL`。
pub fn infer(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("代码为空".into());
    }

    // 已经是 secid 形式
    if let Some((market, code)) = s.split_once('.')
        && !market.is_empty()
        && market.chars().all(|c| c.is_ascii_digit())
        && !code.is_empty()
        && code.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Ok(format!("{market}.{}", code.to_ascii_uppercase()));
    }

    let lower = s.to_ascii_lowercase();
    for (prefix, market) in [
        ("hk", "116."),
        ("sh", "1."),
        ("sz", "0."),
        ("bj", "0."),
        ("us", "105."),
    ] {
        if let Some(rest) = lower.strip_prefix(prefix)
            && !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return Ok(format!("{market}{}", rest.to_ascii_uppercase()));
        }
    }

    // 纯字母 → 美股（默认纳斯达克，可用 105./106./107. 精确指定）
    if s.chars().all(|c| c.is_ascii_alphabetic()) {
        return Ok(format!("105.{}", s.to_ascii_uppercase()));
    }

    // 纯数字 → 按位数与首位猜市场
    if s.chars().all(|c| c.is_ascii_digit()) {
        return match s.len() {
            6 => {
                // 6/9/5 开头在沪市（含沪市基金、B股），其余在深市/北交所
                let market = if s.starts_with(['5', '6', '9']) {
                    '1'
                } else {
                    '0'
                };
                Ok(format!("{market}.{s}"))
            }
            5 => Ok(format!("116.{s}")), // 港股
            3 | 4 => Ok(format!("0.{s}")),
            _ => Err(format!(
                "无法识别的代码：{raw}（6 位 A 股 / 5 位港股 / 字母美股）"
            )),
        };
    }

    Err(format!("无法识别的代码：{raw}"))
}

/// secid → 纯代码（用于显示）
pub fn code_of(secid: &str) -> String {
    secid
        .split_once('.')
        .map(|(_, c)| c.to_string())
        .unwrap_or_else(|| secid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_works() {
        assert_eq!(infer("600519").unwrap(), "1.600519");
        assert_eq!(infer("000001").unwrap(), "0.000001");
        assert_eq!(infer("300750").unwrap(), "0.300750");
        assert_eq!(infer("512880").unwrap(), "1.512880");
        assert_eq!(infer("430047").unwrap(), "0.430047");
        assert_eq!(infer("00700").unwrap(), "116.00700");
        assert_eq!(infer("aapl").unwrap(), "105.AAPL");
        assert_eq!(infer("106.BABA").unwrap(), "106.BABA");
        assert_eq!(infer("sh000001").unwrap(), "1.000001");
        assert!(infer("").is_err());
    }
}
