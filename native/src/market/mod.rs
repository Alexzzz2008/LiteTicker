mod details;
mod quotes;
mod search;

use crate::model::{
    MarketDetails, MarketSource, Quote, SymbolMatch, beijing_now, normalize_symbol,
};
use anyhow::{Context, Result, bail};
use encoding_rs::GBK;
use reqwest::Client;
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct MarketClient {
    http: Client,
    #[cfg(test)]
    test_origin: Option<String>,
}
impl MarketClient {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .user_agent("LiteTicker/1.0")
            .timeout(Duration::from_secs(4))
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .build()?;
        Ok(Self {
            http,
            #[cfg(test)]
            test_origin: None,
        })
    }
    pub async fn fetch_quotes(
        &self,
        symbols: &[String],
        source: MarketSource,
        token: CancellationToken,
    ) -> Result<Vec<Quote>> {
        check_cancelled(&token)?;
        let mut requested = Vec::new();
        let mut seen = HashSet::new();
        for raw in symbols {
            let symbol = normalize_symbol(raw)?;
            if seen.insert(symbol.clone()) {
                requested.push(symbol);
            }
        }
        let mut identities = HashMap::new();
        let mut conflicts = HashSet::new();
        for symbol in &requested {
            let code = source_code(symbol, source);
            if identities
                .insert(code.clone(), symbol)
                .is_some_and(|previous| previous != symbol)
            {
                conflicts.insert(code);
            }
        }
        if !conflicts.is_empty() {
            requested.retain(|symbol| !conflicts.contains(&source_code(symbol, source)));
            if requested.is_empty() {
                bail!("自选代码的交易所后缀存在冲突，所选行情源无法区分，请检查代码。");
            }
        }
        let mut result = HashMap::new();
        let mut last_error = None;
        for batch in requested.chunks(60) {
            check_cancelled(&token)?;
            let mapping: HashMap<_, _> = batch
                .iter()
                .map(|symbol| (source_code(symbol, source), symbol.clone()))
                .collect();
            let codes = batch
                .iter()
                .map(|symbol| source_code(symbol, source))
                .collect::<Vec<_>>()
                .join(",");
            let url = match source {
                MarketSource::Tencent => format!("https://qt.gtimg.cn/q={codes}"),
                MarketSource::Eastmoney => format!(
                    "https://push2.eastmoney.com/api/qt/ulist.np/get?fltt=2&secids={codes}&fields=f12,f13,f14,f2,f18,f3,f5,f6,f124"
                ),
            };
            let parsed = match self
                .get_text(
                    &url,
                    source,
                    source == MarketSource::Tencent,
                    2 * 1024 * 1024,
                    &token,
                )
                .await
            {
                Ok(text) => quotes::parse(&text, source, &mapping),
                Err(error) => Err(error),
            };
            match parsed {
                Ok(rows) => {
                    for quote in rows {
                        result.insert(quote.symbol.clone(), quote);
                    }
                }
                Err(error) => {
                    check_cancelled(&token)?;
                    last_error = Some(error);
                }
            }
        }
        check_cancelled(&token)?;
        if !requested.is_empty() && result.is_empty() {
            bail!(
                "所选行情源未返回有效行情，请稍后刷新或切换数据源。{}",
                last_error.map(|e| e.to_string()).unwrap_or_default()
            );
        }
        Ok(requested
            .into_iter()
            .filter_map(|symbol| result.remove(&symbol))
            .collect())
    }
    pub async fn fetch_details(
        &self,
        quote: &Quote,
        source: MarketSource,
        daily: bool,
        token: CancellationToken,
    ) -> Result<MarketDetails> {
        check_cancelled(&token)?;
        let symbol = normalize_symbol(&quote.symbol)?;
        let code = source_code(&symbol, source);
        let mut details = MarketDetails {
            symbol: symbol.clone(),
            ..Default::default()
        };
        let url = match (source, daily) {
            (MarketSource::Tencent, true) => format!(
                "https://web.ifzq.gtimg.cn/appstock/app/fqkline/get?param={code},day,,,60,qfq"
            ),
            (MarketSource::Tencent, false) => {
                format!("https://web.ifzq.gtimg.cn/appstock/app/minute/query?code={code}")
            }
            (MarketSource::Eastmoney, true) => format!(
                "https://push2his.eastmoney.com/api/qt/stock/kline/get?secid={code}&klt=101&fqt=1&beg={}&end=20500101&lmt=60&fields1=f1,f2,f3,f4,f5,f6&fields2=f51,f52,f53,f54,f55",
                (beijing_now() - chrono::Duration::days(365)).format("%Y%m%d")
            ),
            (MarketSource::Eastmoney, false) => format!(
                "https://push2his.eastmoney.com/api/qt/stock/trends2/get?secid={code}&fields1=f1,f2,f3,f4,f5,f6,f7,f8&fields2=f51,f52,f53,f54,f55,f56,f57,f58&ndays=1&iscr=0"
            ),
        };
        let result: Result<bool> = async {
            let text = self
                .get_text(&url, source, false, 2 * 1024 * 1024, &token)
                .await?;
            let payload = json_object(&text)?;
            if daily {
                details.daily = details::parse_daily(&payload, &symbol, &code, source)?;
                Ok(!details.daily.is_empty())
            } else {
                details.minutes = details::parse_minutes(&payload, &symbol, &code, source)?;
                Ok(!details.minutes.is_empty())
            }
        }
        .await;
        check_cancelled(&token)?;
        let label = if daily { "日K" } else { "分时" };
        details.error = match result {
            Ok(true) => None,
            Ok(false) => Some(format!("{label}暂无有效数据")),
            Err(error) => Some(format!("{label}请求失败：{error}")),
        };
        Ok(details)
    }
    pub async fn fetch_recent_trades(
        &self,
        quote: &Quote,
        source: MarketSource,
        token: CancellationToken,
    ) -> Result<MarketDetails> {
        check_cancelled(&token)?;
        let symbol = normalize_symbol(&quote.symbol)?;
        let code = source_code(&symbol, source);
        let work = async {
            let trades = if source == MarketSource::Tencent {
                let text = self.get_text(&format!("https://stock.gtimg.cn/data/index.php?appn=detail&action=recent_data&c={code}"), source, false, 2 * 1024 * 1024, &token).await.context("腾讯近期成交页时间表请求失败")?;
                let metadata = details::tencent_array(&text, &format!("v_detail_time_{code}"))?;
                let expected = details::quote_date(&quote.timestamp);
                let (page, start) = details::recent_page(&metadata, expected)?;
                if let Some(page) = page {
                    let text = self.get_text(&format!("https://stock.gtimg.cn/data/index.php?appn=detail&action=data&c={code}&p={page}"), source, false, 2 * 1024 * 1024, &token).await.context("腾讯近期成交末页请求失败")?;
                    let records = details::tencent_array(&text, &format!("v_detail_data_{code}"))?;
                    details::parse_recent_page(&records, page, start)?
                } else {
                    Vec::new()
                }
            } else {
                let text = self.get_text(&format!("https://push2.eastmoney.com/api/qt/stock/details/get?ut=fa5fd1943c7b386f172d6893dbfba10b&secid={code}&fields1=f1,f2,f3,f4&fields2=f51,f52,f53,f54,f55&pos=-20"), source, false, 2 * 1024 * 1024, &token).await?;
                details::parse_em_trades(&json_object(&text)?, &symbol)?
            };
            Ok(MarketDetails {
                symbol,
                trades,
                ..Default::default()
            })
        };
        tokio::select! { biased;
            _ = token.cancelled() => bail!("请求已取消"),
            result = tokio::time::timeout(Duration::from_secs(4), work) => result.context("近期成交请求超过4秒，请稍后重试。")?,
        }
    }
    pub async fn search(&self, query: &str, token: CancellationToken) -> Result<Vec<SymbolMatch>> {
        search::search(self, query, token).await
    }

    async fn get_text(
        &self,
        url: &str,
        source: MarketSource,
        gbk: bool,
        limit: usize,
        token: &CancellationToken,
    ) -> Result<String> {
        check_cancelled(token)?;
        #[cfg(test)]
        let rewritten = self.test_origin.as_ref().map(|origin| {
            let original = reqwest::Url::parse(url).unwrap();
            format!(
                "{origin}{}{}",
                original.path(),
                original
                    .query()
                    .map(|q| format!("?{q}"))
                    .unwrap_or_default()
            )
        });
        #[cfg(test)]
        let url = rewritten.as_deref().unwrap_or(url);
        let request = async {
            let referer = match source {
                MarketSource::Tencent => "https://gu.qq.com/",
                MarketSource::Eastmoney => "https://quote.eastmoney.com/",
            };
            let mut response = self
                .http
                .get(url)
                .header(reqwest::header::REFERER, referer)
                .send()
                .await?
                .error_for_status()?;
            if response.content_length().is_some_and(|n| n > limit as u64) {
                bail!("行情接口正文超过大小上限");
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if body.len() + chunk.len() > limit {
                    bail!("行情接口正文超过大小上限");
                }
                body.extend_from_slice(&chunk);
            }
            check_cancelled(token)?;
            if gbk {
                GBK.decode_without_bom_handling_and_without_replacement(&body)
                    .map(|s| s.into_owned())
                    .context("接口返回了损坏的 GBK 数据")
            } else {
                String::from_utf8(body).context("行情接口返回了损坏的 UTF-8 数据")
            }
        };
        tokio::select! { biased;
            _ = token.cancelled() => bail!("请求已取消"),
            result = tokio::time::timeout(Duration::from_secs(4), request) => result.context("请求超过4秒，请稍后重试。")?,
        }
    }
}
fn check_cancelled(token: &CancellationToken) -> Result<()> {
    if token.is_cancelled() {
        bail!("请求已取消");
    }
    Ok(())
}
fn source_code(symbol: &str, source: MarketSource) -> String {
    if source == MarketSource::Tencent {
        format!("{}{}", symbol[7..].to_ascii_lowercase(), &symbol[..6])
    } else {
        format!(
            "{}.{}",
            if &symbol[7..] == "SH" { 1 } else { 0 },
            &symbol[..6]
        )
    }
}
fn json_object(text: &str) -> Result<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(text).context("行情接口返回了无效 JSON")?;
    if !value.is_object() {
        bail!("行情接口返回的 JSON 不是对象");
    }
    Ok(value)
}
fn number(value: &serde_json::Value) -> Option<rust_decimal::Decimal> {
    match value {
        serde_json::Value::String(value) => decimal(value),
        serde_json::Value::Number(value) => decimal(value.as_str()),
        _ => None,
    }
}
fn decimal(value: &str) -> Option<rust_decimal::Decimal> {
    let value = value.trim();
    value
        .parse()
        .or_else(|_| rust_decimal::Decimal::from_scientific(value))
        .ok()
}
fn text(value: &serde_json::Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        if value.is_null() {
            String::new()
        } else {
            value.to_string()
        }
    })
}

#[cfg(test)]
mod tests;
