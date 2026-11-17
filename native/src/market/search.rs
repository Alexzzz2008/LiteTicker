use super::{MarketClient, check_cancelled};
use crate::model::{MarketSource, SymbolMatch, normalize_symbol};
use anyhow::{Context, Result, bail};
use regex::Regex;
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};
use tokio_util::sync::CancellationToken;

pub(super) async fn search(
    client: &MarketClient,
    query: &str,
    token: CancellationToken,
) -> Result<Vec<SymbolMatch>> {
    check_cancelled(&token)?;
    let value = query.trim();
    if value.is_empty() {
        return Ok(Vec::new());
    }
    if value.encode_utf16().count() > 256
        || value.chars().any(
            |c| matches!(c, '\0'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}'),
        )
    {
        bail!("查询最多256个字符，请去掉无效控制字符。");
    }
    let matches = if value.chars().any(separator) {
        let mut symbols = Vec::new();
        let mut seen = HashSet::new();
        for part in value.split(separator).filter(|s| !s.is_empty()) {
            let upper = part.to_ascii_uppercase();
            let symbol = normalize_symbol(&upper).context(
                "批量查询请使用6位代码，可带 .SH、.SZ 或 .BJ 后缀，以逗号、空格或换行分隔。",
            )?;
            if seen.insert(symbol.clone()) {
                symbols.push(symbol);
            }
        }
        if symbols.len() > 20 {
            bail!("一次最多查询20个不同代码。");
        }
        if symbols.is_empty() {
            return Ok(Vec::new());
        }
        let codes = symbols
            .iter()
            .map(|s| format!("{}{}", s[7..].to_ascii_lowercase(), &s[..6]))
            .collect::<Vec<_>>();
        let mapping: HashMap<String, String> =
            codes.iter().cloned().zip(symbols.iter().cloned()).collect();
        let text = client
            .get_text(
                &format!("https://qt.gtimg.cn/q={}", codes.join(",")),
                MarketSource::Tencent,
                true,
                256 * 1024,
                &token,
            )
            .await?;
        batch(&text, &symbols, &mapping)?
    } else {
        if value.encode_utf16().count() > 40 {
            bail!("单个查询最多40个字符，请输入股票代码或拼音首字母。");
        }
        let exact = if value.len() == 9 {
            normalize_symbol(value).ok()
        } else {
            None
        };
        let query = exact.as_ref().map(|s| &s[..6]).unwrap_or(value);
        let query = if query.bytes().all(|b| b.is_ascii_alphabetic()) {
            query.to_ascii_lowercase()
        } else {
            query.to_owned()
        };
        let code = (query.len() == 6 && query.bytes().all(|b| b.is_ascii_digit()))
            .then_some(query.as_str());
        let mut url = reqwest::Url::parse("https://smartbox.gtimg.cn/s3/")?;
        url.query_pairs_mut()
            .append_pair("q", &query)
            .append_pair("t", "all");
        let text = client
            .get_text(
                url.as_str(),
                MarketSource::Tencent,
                true,
                256 * 1024,
                &token,
            )
            .await?;
        suggestions(&text, exact.as_deref(), code)?
    };
    check_cancelled(&token)?;
    Ok(matches)
}
pub(super) fn suggestions(
    text: &str,
    exact: Option<&str>,
    code: Option<&str>,
) -> Result<Vec<SymbolMatch>> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let assignment = PATTERN
        .get_or_init(|| Regex::new(r#"^\s*v_hint\s*=\s*("(?:[^"\\]|\\.)*")\s*;?\s*$"#).unwrap())
        .captures(text)
        .context("证券查询接口返回了无法识别的数据。")
        .map(|a| a[1].to_owned())?;
    let hint: String = serde_json::from_str(&assignment).context("证券查询名称解码失败。")?;
    if hint.is_empty() || hint == "N" {
        return Ok(Vec::new());
    }
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for row in hint.split('^') {
        let fields: Vec<_> = row.split('~').collect();
        if fields.len() < 4 {
            continue;
        }
        let market = fields[0].to_ascii_uppercase();
        if !matches!(market.as_str(), "SH" | "SZ" | "BJ")
            || fields[1].len() != 6
            || !fields[1].bytes().all(|b| b.is_ascii_digit())
            || !valid_name(fields[2])
            || !valid_initials(fields[3])
        {
            continue;
        }
        let symbol = format!("{}.{}", fields[1], market);
        if exact.is_some_and(|s| symbol != s)
            || code.is_some_and(|c| fields[1] != c)
            || !seen.insert(symbol.clone())
        {
            continue;
        }
        result.push(SymbolMatch {
            symbol,
            name: fields[2].trim().into(),
            initials: fields[3].trim().into(),
        });
        if result.len() == 20 {
            break;
        }
    }
    Ok(result)
}
pub(super) fn batch(
    text: &str,
    symbols: &[String],
    mapping: &HashMap<String, String>,
) -> Result<Vec<SymbolMatch>> {
    static RECOGNIZED: OnceLock<Regex> = OnceLock::new();
    static ROW: OnceLock<Regex> = OnceLock::new();
    if !RECOGNIZED
        .get_or_init(|| {
            Regex::new(r#"\bv_(?:(?:sh|sz|bj)[0-9]{6}|pv_none_match)\s*=\s*""#).unwrap()
        })
        .is_match(text)
    {
        bail!("证券查询接口返回了无法识别的数据。");
    }
    let mut matches = HashMap::new();
    for assignment in ROW
        .get_or_init(|| Regex::new(r#"\bv_((?:sh|sz|bj)[0-9]{6})\s*=\s*"([^"]*)"\s*;?"#).unwrap())
        .captures_iter(text)
    {
        let Some(symbol) = mapping.get(&assignment[1]) else {
            continue;
        };
        let fields: Vec<_> = assignment[2].split('~').collect();
        if fields.len() < 3 || fields[2] != &symbol[..6] || !valid_name(fields[1]) {
            continue;
        }
        matches
            .entry(symbol.clone())
            .or_insert_with(|| SymbolMatch {
                symbol: symbol.clone(),
                name: fields[1].trim().into(),
                initials: String::new(),
            });
    }
    Ok(symbols.iter().filter_map(|s| matches.remove(s)).collect())
}
fn separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, ',' | '，' | ';' | '；')
}
fn valid_name(value: &str) -> bool {
    !value.trim().is_empty() && valid_initials(value)
}
fn valid_initials(value: &str) -> bool {
    value.encode_utf16().count() <= 80
        && !value.contains('\u{fffd}')
        && !value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}
