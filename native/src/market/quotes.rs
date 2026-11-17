use super::{json_object, number, text};
use crate::model::{DepthLevel, MarketSource, Quote};
use anyhow::Result;
use chrono::NaiveDateTime;
use regex::Regex;
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde_json::Value;
use std::{collections::HashMap, sync::OnceLock};

pub(super) fn parse(
    payload: &str,
    source: MarketSource,
    mapping: &HashMap<String, String>,
) -> Result<Vec<Quote>> {
    if source == MarketSource::Tencent {
        return Ok(tencent(payload, mapping));
    }
    let payload = json_object(payload)?;
    let rows: Vec<&Value> = match &payload["data"]["diff"] {
        Value::Array(rows) => rows.iter().collect(),
        Value::Object(rows) => rows.values().collect(),
        _ => Vec::new(),
    };
    let mut quotes = Vec::new();
    for row in rows {
        let identity = format!("{}.{}", text(&row["f13"]), text(&row["f12"]));
        let Some(symbol) = mapping.get(&identity) else {
            continue;
        };
        let name = text(&row["f14"]);
        if name.trim().is_empty() {
            continue;
        }
        let Some(mut quote) = make_quote(
            symbol,
            &name,
            [&row["f2"], &row["f18"], &row["f3"], &row["f5"], &row["f6"]],
            Decimal::ONE,
        ) else {
            continue;
        };
        if let Some(timestamp) = number(&row["f124"])
            .filter(|n| *n > Decimal::ZERO && *n <= Decimal::from(253402271999_i64))
            .and_then(|n| n.to_i64())
            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
        {
            quote.timestamp = (timestamp.naive_utc() + chrono::Duration::hours(8))
                .format("%Y%m%d%H%M%S")
                .to_string();
        }
        quotes.push(quote);
    }
    Ok(quotes)
}
fn tencent(payload: &str, mapping: &HashMap<String, String>) -> Vec<Quote> {
    static ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
    let pattern =
        ASSIGNMENT.get_or_init(|| Regex::new(r#"v_((?:sh|sz|bj)[0-9]{6})="([^"]*)";"#).unwrap());
    let mut quotes = Vec::new();
    for capture in pattern.captures_iter(payload) {
        let Some(symbol) = mapping.get(&capture[1]) else {
            continue;
        };
        let fields: Vec<&str> = capture[2].split('~').collect();
        if fields.len() < 38 || fields[1].trim().is_empty() {
            continue;
        }
        let values: Vec<Value> = [3, 4, 32, 6, 37]
            .iter()
            .map(|i| Value::String(fields[*i].into()))
            .collect();
        let Some(mut quote) = make_quote(
            symbol,
            fields[1],
            [&values[0], &values[1], &values[2], &values[3], &values[4]],
            Decimal::from(10000),
        ) else {
            continue;
        };
        if fields[30].len() == 14
            && NaiveDateTime::parse_from_str(fields[30], "%Y%m%d%H%M%S").is_ok()
        {
            quote.timestamp = fields[30].into();
        }
        quote.bids = depth(&fields, 9);
        quote.asks = depth(&fields, 19);
        quotes.push(quote);
    }
    quotes
}
fn depth(fields: &[&str], offset: usize) -> Vec<DepthLevel> {
    let mut any = false;
    let levels = (0..5)
        .map(|i| {
            match (
                number(&Value::String(fields[offset + i * 2].into())),
                number(&Value::String(fields[offset + i * 2 + 1].into())),
            ) {
                (Some(price), Some(volume_lots))
                    if price > Decimal::ZERO && volume_lots >= Decimal::ZERO =>
                {
                    any = true;
                    DepthLevel { price, volume_lots }
                }
                _ => DepthLevel::default(),
            }
        })
        .collect();
    if any { levels } else { Vec::new() }
}
fn make_quote(symbol: &str, name: &str, values: [&Value; 5], multiplier: Decimal) -> Option<Quote> {
    let (mut price, close) = (number(values[0])?, number(values[1])?);
    if price < Decimal::ZERO || close < Decimal::ZERO {
        return None;
    }
    let suspended = price == Decimal::ZERO;
    if suspended {
        price = close;
    }
    if price <= Decimal::ZERO {
        return None;
    }
    let mut percent = if let Some(n) = number(values[2]) {
        n
    } else if close > Decimal::ZERO {
        price
            .checked_sub(close)?
            .checked_div(close)?
            .checked_mul(Decimal::from(100))?
    } else {
        Decimal::ZERO
    };
    if suspended {
        percent = Decimal::ZERO;
    }
    let volume = number(values[3]).unwrap_or_default();
    let amount = number(values[4]).unwrap_or_default();
    if volume < Decimal::ZERO || amount < Decimal::ZERO {
        return None;
    }
    Some(Quote {
        symbol: symbol.into(),
        name: name.into(),
        price,
        previous_close: close,
        change_percent: percent,
        volume_lots: volume,
        amount_yuan: amount.checked_mul(multiplier)?,
        ..Default::default()
    })
}
