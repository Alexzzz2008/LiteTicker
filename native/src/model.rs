use anyhow::{Result, bail};
use chrono::{Datelike, NaiveDateTime, Timelike, Weekday};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum MarketSource {
    #[default]
    Tencent,
    Eastmoney,
}
impl TryFrom<u8> for MarketSource {
    type Error = &'static str;
    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Tencent),
            1 => Ok(Self::Eastmoney),
            _ => Err("无效行情源"),
        }
    }
}
impl From<MarketSource> for u8 {
    fn from(value: MarketSource) -> Self {
        match value {
            MarketSource::Tencent => 0,
            MarketSource::Eastmoney => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AppSettings {
    pub source: MarketSource,
    pub refresh_seconds: i32,
    pub trade_refresh_seconds: i32,
    pub x: i32,
    pub y: i32,
    pub opacity_percent: i32,
    pub always_on_top: bool,
    pub auto_check_updates: bool,
    pub global_font_percent: i32,
    pub quote_font_percent: i32,
    pub details_font_percent: i32,
    pub toggle_shortcut: u32,
    pub refresh_shortcut: u32,
    pub symbols: Vec<String>,
}
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            source: MarketSource::Tencent,
            refresh_seconds: 1,
            trade_refresh_seconds: 3,
            x: 160,
            y: 160,
            opacity_percent: 100,
            always_on_top: true,
            auto_check_updates: true,
            global_font_percent: 100,
            quote_font_percent: 0,
            details_font_percent: 0,
            toggle_shortcut: 0x60051,
            refresh_shortcut: 0x60052,
            symbols: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Quote {
    pub symbol: String,
    pub name: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub previous_close: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub change_percent: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub volume_lots: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub amount_yuan: Decimal,
    pub timestamp: String,
    pub bids: Vec<DepthLevel>,
    pub asks: Vec<DepthLevel>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct DepthLevel {
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub volume_lots: Decimal,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct MinutePoint {
    pub time: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub average: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub volume_lots: Decimal,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct DailyBar {
    pub date: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub open: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub close: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub high: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub low: Decimal,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct TradeTick {
    pub time: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub volume_lots: Decimal,
    pub side: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct MarketDetails {
    pub symbol: String,
    pub minutes: Vec<MinutePoint>,
    pub daily: Vec<DailyBar>,
    pub trades: Vec<TradeTick>,
    pub error: Option<String>,
    pub trades_stale: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SymbolMatch {
    pub symbol: String,
    pub name: String,
    pub initials: String,
}

pub fn normalize_symbol(raw: &str) -> Result<String> {
    let mut value = raw.trim().to_ascii_uppercase();
    if value.len() == 6 && value.bytes().all(|b| b.is_ascii_digit()) {
        let market = if value.starts_with(['4', '8']) || value.starts_with("92") {
            "BJ"
        } else if value.starts_with(['5', '6', '9']) {
            "SH"
        } else {
            "SZ"
        };
        value.push('.');
        value.push_str(market);
    }
    let bytes = value.as_bytes();
    if bytes.len() != 9
        || !bytes[..6].iter().all(u8::is_ascii_digit)
        || bytes[6] != b'.'
        || !matches!(&value[7..], "SH" | "SZ" | "BJ")
    {
        bail!("无效代码：{raw}。请输入 600519.SH 或 600519。");
    }
    Ok(value)
}
pub fn parse_symbols(raw: &str) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for token in raw
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | ';' | '；'))
        .filter(|s| !s.is_empty())
    {
        let symbol = normalize_symbol(token)?;
        if seen.insert(symbol.clone()) {
            result.push(symbol);
        }
    }
    Ok(result)
}
pub fn move_symbol(symbols: &mut [String], symbol: &str, direction: i32) -> bool {
    if !matches!(direction, -1 | 1) {
        return false;
    }
    let Some(index) = symbols.iter().position(|s| s == symbol) else {
        return false;
    };
    let target = index as i64 + i64::from(direction);
    if target < 0 || target >= symbols.len() as i64 {
        return false;
    }
    symbols.swap(index, target as usize);
    true
}
fn fixed(value: Decimal, places: u32) -> String {
    format!(
        "{:.*}",
        places as usize,
        value.round_dp_with_strategy(places, RoundingStrategy::MidpointAwayFromZero)
    )
}
pub fn format_price(price: Decimal) -> String {
    let mut result = fixed(price, 3);
    if result.ends_with('0') {
        result.pop();
    }
    result
}
pub fn format_volume_lots(lots: Decimal) -> String {
    if lots >= Decimal::from(100_000_000) {
        format!("{}亿", fixed(lots / Decimal::from(100_000_000), 1))
    } else if lots >= Decimal::from(10_000) {
        format!("{}万", fixed(lots / Decimal::from(10_000), 1))
    } else {
        fixed(lots, 0)
    }
}
pub fn format_depth_amount(price: Decimal, lots: Decimal) -> String {
    if price <= Decimal::ZERO || lots < Decimal::ZERO {
        return "—".into();
    }
    let Some(amount) = price
        .checked_mul(lots)
        .and_then(|n| n.checked_mul(Decimal::from(100)))
    else {
        return "—".into();
    };
    if amount >= Decimal::from(100_000_000) {
        format!("{}亿", fixed(amount / Decimal::from(100_000_000), 2))
    } else if amount >= Decimal::from(10_000) {
        format!("{}万", fixed(amount / Decimal::from(10_000), 2))
    } else {
        fixed(amount, 2)
    }
}
pub fn is_trading_time(beijing: NaiveDateTime) -> bool {
    if matches!(beijing.weekday(), Weekday::Sat | Weekday::Sun) {
        return false;
    }
    let hm = beijing.hour() * 100 + beijing.minute();
    (915..=1135).contains(&hm) || (1255..=1505).contains(&hm)
}
pub fn interval_seconds(settings: &AppSettings, now: NaiveDateTime) -> i32 {
    if is_trading_time(now) {
        settings.refresh_seconds
    } else {
        30
    }
}
pub fn beijing_now() -> NaiveDateTime {
    chrono::Utc::now().naive_utc() + chrono::Duration::hours(8)
}

pub fn is_valid_shortcut(shortcut: u32) -> bool {
    if shortcut == 0 {
        return true;
    }
    if shortcut & !0x7ffff != 0 {
        return false;
    }
    let key = shortcut & 0xffff;
    let defined = matches!(key, 3 | 8 | 9 | 12 | 13 | 16..=21 | 23..=25 | 27..=31 | 32..=40 | 41..=47 | 48..=57 | 65..=93 | 95..=135 | 144 | 145 | 160..=183 | 186..=192 | 219..=223 | 226 | 229 | 231 | 246..=254);
    if !defined || matches!(key, 8 | 9 | 16..=18 | 27 | 46 | 91 | 92 | 160..=165) {
        return false;
    }
    (112..=135).contains(&key) || shortcut & 0x60000 != 0
}
