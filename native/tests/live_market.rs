//! Opt-in verification against public market services, independent of the user's watchlist.
use liteticker_native::{market::MarketClient, model::MarketSource};
use tokio_util::sync::CancellationToken;

async fn check(source: MarketSource) -> anyhow::Result<()> {
    let client = MarketClient::new()?;
    let symbol = "600519.SH".to_owned();
    let token = CancellationToken::new();
    let quotes = client
        .fetch_quotes(std::slice::from_ref(&symbol), source, token.clone())
        .await?;
    let quote = quotes
        .iter()
        .find(|q| q.symbol == symbol)
        .ok_or_else(|| anyhow::anyhow!("未返回请求的市场和代码"))?;
    anyhow::ensure!(quote.price.is_sign_positive() && !quote.name.is_empty());
    let (minutes, daily, recent) = tokio::join!(
        client.fetch_details(quote, source, false, token.clone()),
        client.fetch_details(quote, source, true, token.clone()),
        client.fetch_recent_trades(quote, source, token)
    );
    let (minutes, daily, recent) = (minutes?, daily?, recent?);
    println!(
        "{source:?}: 股票 {}，分时 {}，日 K {}，分笔 {}",
        quote.symbol,
        minutes.minutes.len(),
        daily.daily.len(),
        recent.trades.len()
    );
    anyhow::ensure!(
        !minutes.minutes.is_empty() && !daily.daily.is_empty() && !recent.trades.is_empty(),
        "本次实测返回不完整"
    );
    Ok(())
}
#[tokio::test]
#[ignore = "requires public Tencent market services"]
async fn live_tencent() -> anyhow::Result<()> {
    check(MarketSource::Tencent).await
}
#[tokio::test]
#[ignore = "requires public Eastmoney market services"]
async fn live_eastmoney() -> anyhow::Result<()> {
    check(MarketSource::Eastmoney).await
}
#[tokio::test]
#[ignore = "requires public stock search"]
async fn live_initials() -> anyhow::Result<()> {
    let client = MarketClient::new()?;
    for query in ["gzmt", "600519"] {
        let rows = client.search(query, CancellationToken::new()).await?;
        anyhow::ensure!(
            rows.iter()
                .any(|r| r.symbol == "600519.SH" && r.name.contains("茅台")),
            "搜索 {query} 未找到贵州茅台"
        );
    }
    Ok(())
}
