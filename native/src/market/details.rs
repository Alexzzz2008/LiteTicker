use super::{decimal, number, text};
use crate::model::{DailyBar, MarketSource, MinutePoint, TradeTick, beijing_now};
use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use rust_decimal::Decimal;
use serde_json::Value;
use std::collections::BTreeMap;

fn em_data<'a>(payload: &'a Value, symbol: &str) -> Result<&'a Value> {
    let data = &payload["data"];
    if !data["code"].is_null() && text(&data["code"]) != symbol[..6] {
        bail!("行情接口返回了其他证券的数据。");
    }
    Ok(data)
}
pub(super) fn parse_minutes(
    payload: &Value,
    symbol: &str,
    code: &str,
    source: MarketSource,
) -> Result<Vec<MinutePoint>> {
    let tencent = source == MarketSource::Tencent;
    let (rows, tencent_index) = if tencent {
        let security = &payload["data"][code];
        (&security["data"]["data"], security["qt"][code][61] == "ZS")
    } else {
        (&em_data(payload, symbol)?["trends"], false)
    };
    let mut sorted = BTreeMap::new();
    for row in rows.as_array().into_iter().flatten() {
        let row = text(row);
        let fields: Vec<&str> = if tencent {
            row.split_whitespace().collect()
        } else {
            row.split(',').collect()
        };
        if fields.len() < if tencent { 3 } else { 6 } {
            continue;
        }
        let time = if tencent {
            if fields[0].len() != 4 {
                continue;
            }
            NaiveTime::parse_from_str(fields[0], "%H%M").ok()
        } else {
            NaiveDateTime::parse_from_str(fields[0], "%Y-%m-%d %H:%M")
                .ok()
                .map(|n| n.time())
        };
        let Some(time) = time else {
            continue;
        };
        if !is_session_minute(time) {
            continue;
        }
        let (Some(price), Some(volume)) = (
            decimal(fields[if tencent { 1 } else { 2 }]),
            decimal(fields[if tencent { 2 } else { 5 }]),
        ) else {
            continue;
        };
        if price <= Decimal::ZERO || volume < Decimal::ZERO {
            continue;
        }
        let average = if tencent_index {
            // Index turnover is in yuan; dividing by shares cannot produce index points.
            None
        } else if tencent {
            fields
                .get(3)
                .and_then(|amount| decimal(amount))
                .and_then(|amount| amount.checked_div(volume))
                .and_then(|per_lot| per_lot.checked_div(Decimal::from(100)))
        } else {
            fields.get(7).and_then(|value| decimal(value))
        };
        let minute = time.format("%H:%M").to_string();
        sorted.insert(
            minute.clone(),
            MinutePoint {
                time: minute,
                price,
                average: average
                    .filter(|value| *value > Decimal::ZERO)
                    .unwrap_or_default(),
                volume_lots: volume,
                leading: Decimal::ZERO,
            },
        );
    }
    let mut previous = Decimal::ZERO;
    Ok(sorted
        .into_values()
        .map(|mut p| {
            if tencent {
                let cumulative = p.volume_lots;
                p.volume_lots = cumulative
                    .checked_sub(previous)
                    .unwrap_or_default()
                    .max(Decimal::ZERO);
                previous = cumulative;
            }
            p
        })
        .collect())
}

fn is_session_minute(time: NaiveTime) -> bool {
    matches!(time.hour() * 100 + time.minute(), 930..=1130 | 1300..=1500)
}

pub(super) fn shanghai_session_day(
    payload: &Value,
    code: &str,
    source: MarketSource,
) -> Option<NaiveDate> {
    let data = &payload["data"];
    if source == MarketSource::Tencent {
        return NaiveDate::parse_from_str(data[code]["data"]["date"].as_str()?, "%Y%m%d").ok();
    }
    let (market, symbol) = code.split_once('.')?;
    if data["code"].as_str() != Some(symbol)
        || data
            .get("market")
            .is_some_and(|value| text(value) != market)
    {
        return None;
    }
    let mut days = data["trends"].as_array()?.iter().filter_map(|row| {
        let (timestamp, _) = row.as_str()?.split_once(',')?;
        let point = NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M").ok()?;
        is_session_minute(point.time()).then_some(point.date())
    });
    let day = days.next()?;
    days.all(|other| other == day).then_some(day)
}

fn sina_shanghai_open(snapshot: &str, day: NaiveDate) -> Option<Decimal> {
    let (variable, value) = snapshot.trim().split_once('=')?;
    if variable.trim() != "var hq_str_sh000001" {
        return None;
    }
    let value = value.trim();
    let fields: &str = serde_json::from_str(value.strip_suffix(';').unwrap_or(value)).ok()?;
    let fields = fields.split(',').collect::<Vec<_>>();
    let open = decimal(fields.get(1)?)?;
    let snapshot_day = NaiveDate::parse_from_str(fields.get(30)?, "%Y-%m-%d").ok()?;
    (snapshot_day == day && open > Decimal::ZERO).then_some(open)
}

pub(super) fn apply_shanghai_leading(
    minutes: &mut [MinutePoint],
    day: NaiveDate,
    snapshot: &str,
    payload: &Value,
) {
    let result = &payload["result"];
    if result["status"]["code"].as_i64() != Some(0) {
        return;
    }
    let (Some(open), Some(rows)) = (sina_shanghai_open(snapshot, day), result["data"].as_array())
    else {
        return;
    };
    let iso_day = day.to_string();
    let compact_day = day.format("%Y%m%d").to_string();
    let mut opening_matches = false;
    let mut leading = BTreeMap::new();
    for row in rows {
        for key in ["d", "date"] {
            if let Some(date) = row.get(key)
                && date.as_str() != Some(iso_day.as_str())
                && date.as_str() != Some(compact_day.as_str())
            {
                return;
            }
        }
        let Some(time) = row["m"].as_str().and_then(|s| s.strip_suffix(":00")) else {
            continue;
        };
        if time == "09:30" {
            // Sina's snapshot has four decimals; its opening minute price has three.
            opening_matches = number(&row["p"])
                .filter(|price| *price > Decimal::ZERO)
                .is_some_and(|price| (price - open).abs() <= Decimal::new(5, 4));
        }
        if let Some(value) = number(&row["lx"]).filter(|value| *value > Decimal::ZERO) {
            leading.insert(time, value);
        }
    }
    if !opening_matches {
        return;
    }
    // The primary feed defines the trading minutes on the chart.
    for point in minutes {
        if let Some(&value) = leading.get(point.time.as_str()) {
            point.leading = value;
        }
    }
}

pub(super) fn parse_daily(
    payload: &Value,
    symbol: &str,
    code: &str,
    source: MarketSource,
) -> Result<Vec<DailyBar>> {
    let rows = if source == MarketSource::Tencent {
        let data = &payload["data"][code];
        if data["qfqday"].as_array().is_some_and(|a| !a.is_empty()) {
            &data["qfqday"]
        } else {
            &data["day"]
        }
    } else {
        &em_data(payload, symbol)?["klines"]
    };
    let mut sorted = BTreeMap::new();
    for row in rows.as_array().into_iter().flatten() {
        let fields: Vec<String> = if source == MarketSource::Tencent {
            row.as_array()
                .map(|r| r.iter().map(text).collect())
                .unwrap_or_default()
        } else {
            text(row).split(',').map(str::to_owned).collect()
        };
        if fields.len() < 5
            || fields[0].len() != 10
            || NaiveDate::parse_from_str(&fields[0], "%Y-%m-%d").is_err()
        {
            continue;
        }
        let (Some(open), Some(close), Some(high), Some(low)) = (
            decimal(&fields[1]),
            decimal(&fields[2]),
            decimal(&fields[3]),
            decimal(&fields[4]),
        ) else {
            continue;
        };
        if low <= Decimal::ZERO || high < low || high < open.max(close) || low > open.min(close) {
            continue;
        }
        sorted.insert(
            fields[0].clone(),
            DailyBar {
                date: fields[0].clone(),
                open,
                close,
                high,
                low,
            },
        );
    }
    let skip = sorted.len().saturating_sub(60);
    Ok(sorted.into_values().skip(skip).collect())
}
pub(super) fn tencent_array(text: &str, variable: &str) -> Result<Vec<Value>> {
    let (_, payload) = text
        .split_once('=')
        .filter(|(name, _)| name.trim() == variable)
        .context("腾讯近期成交证券变量或响应格式不匹配。")?;
    let payload = payload.trim_end();
    let payload = payload.strip_suffix(';').unwrap_or(payload).trim();
    let values: Vec<Value> =
        serde_json::from_str(payload).context("腾讯近期成交返回了无效数据。")?;
    if values.len() != 2 || !values[1].is_string() {
        bail!("腾讯近期成交响应结构无效。");
    }
    Ok(values)
}
pub(super) fn quote_date(timestamp: &str) -> NaiveDate {
    if timestamp.len() == 14 {
        NaiveDateTime::parse_from_str(timestamp, "%Y%m%d%H%M%S")
            .ok()
            .map(|d| d.date())
    } else if timestamp.len() == 8 {
        NaiveDate::parse_from_str(timestamp, "%Y%m%d").ok()
    } else {
        None
    }
    .unwrap_or_else(|| beijing_now().date())
}
pub(super) fn recent_page(
    metadata: &[Value],
    expected: NaiveDate,
) -> Result<(Option<usize>, NaiveTime)> {
    let date = text(&metadata[0]);
    if date.len() != 8 || NaiveDate::parse_from_str(&date, "%Y%m%d").ok() != Some(expected) {
        bail!("腾讯近期成交日期与报价日期不匹配。");
    }
    let ranges = metadata[1].as_str().context("腾讯近期成交页时间表无效。")?;
    if ranges.is_empty() {
        return Ok((None, NaiveTime::MIN));
    }
    let ranges: Vec<_> = ranges.split('|').collect();
    if ranges.len() > 4096 {
        bail!("腾讯近期成交页数超出上限。");
    }
    let mut start = NaiveTime::MIN;
    let mut end = NaiveTime::MIN;
    for range in &ranges {
        let bounds: Vec<_> = range.split('~').collect();
        if bounds.len() != 2 {
            bail!("腾讯近期成交页时间表无效。");
        }
        let first = trade_time(bounds[0]).context("腾讯近期成交页时间表无效。")?;
        let last = trade_time(bounds[1]).context("腾讯近期成交页时间表无效。")?;
        if first > last || first < end {
            bail!("腾讯近期成交页时间表无效。");
        }
        start = first;
        end = last;
    }
    Ok((Some(ranges.len() - 1), start))
}
pub(super) fn parse_recent_page(
    records: &[Value],
    expected: usize,
    start: NaiveTime,
) -> Result<Vec<TradeTick>> {
    if text(&records[0]).parse::<usize>().ok() != Some(expected) {
        bail!("腾讯近期成交返回页号与末页不匹配。");
    }
    let mut parsed = BTreeMap::new();
    for row in records[1]
        .as_str()
        .context("腾讯近期成交响应结构无效。")?
        .split('|')
    {
        let fields: Vec<_> = row.split('/').collect();
        if fields.len() < 6 {
            continue;
        }
        let (Ok(sequence), Some(time), Some(price), Some(volume)) = (
            fields[0].parse::<u64>(),
            trade_time(fields[1]),
            decimal(fields[2]),
            decimal(fields[4]),
        ) else {
            continue;
        };
        if time < start || price <= Decimal::ZERO || volume <= Decimal::ZERO {
            continue;
        }
        parsed.insert(
            sequence,
            TradeTick {
                time: fields[1].into(),
                price,
                volume_lots: volume,
                side: match fields.get(6) {
                    Some(&"B") => 1,
                    Some(&"S") => 2,
                    _ => 0,
                },
            },
        );
    }
    if parsed.is_empty() {
        bail!("腾讯近期成交末页为空、损坏或时间区间不匹配。");
    }
    // Within one second, larger sequence numbers identify newer trades.
    Ok(latest(parsed.into_values().rev().collect()))
}
pub(super) fn parse_em_trades(payload: &Value, symbol: &str) -> Result<Vec<TradeTick>> {
    if !payload["rc"].is_null() && number(&payload["rc"]) != Some(Decimal::ZERO) {
        bail!("东方财富近期成交接口返回错误。");
    }
    let data = em_data(payload, symbol)?;
    if data.is_null() {
        if payload.as_object().is_some_and(|o| o.contains_key("data")) {
            return Ok(Vec::new());
        }
        bail!("东方财富近期成交响应数据字段无效。");
    }
    if !data.is_object() || !data.as_object().unwrap().contains_key("details") {
        bail!("东方财富近期成交响应缺少成交字段。");
    }
    let rows = &data["details"];
    if !rows.is_null() && !rows.is_array() {
        bail!("东方财富近期成交数据格式无效。");
    }
    let mut trades = Vec::new();
    for row in rows.as_array().into_iter().flatten() {
        let row = text(row);
        let fields: Vec<_> = row.split(',').collect();
        if fields.len() < 3 {
            continue;
        }
        let (Some(_time), Some(price), Some(volume)) = (
            trade_time(fields[0]),
            decimal(fields[1]),
            decimal(fields[2]),
        ) else {
            continue;
        };
        if price <= Decimal::ZERO || volume <= Decimal::ZERO {
            continue;
        }
        let side = fields
            .get(4)
            .and_then(|s| decimal(s))
            .filter(|s| *s == Decimal::ONE || *s == Decimal::from(2))
            .map(|s| if s == Decimal::ONE { 1 } else { 2 })
            .unwrap_or_default();
        trades.push(TradeTick {
            time: fields[0].into(),
            price,
            volume_lots: volume,
            side,
        });
    }
    if rows.as_array().is_some_and(|r| !r.is_empty()) && trades.is_empty() {
        bail!("东方财富近期成交没有可解析的有效记录。");
    }
    Ok(latest(trades))
}
fn latest(mut trades: Vec<TradeTick>) -> Vec<TradeTick> {
    trades.sort_by(|a, b| b.time.cmp(&a.time));
    trades.truncate(20);
    trades
}
fn trade_time(value: &str) -> Option<NaiveTime> {
    if value.len() != 8 {
        return None;
    }
    NaiveTime::parse_from_str(value, "%H:%M:%S").ok()
}
