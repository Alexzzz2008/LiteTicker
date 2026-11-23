//! Toolkit-independent request lifetimes. The UI owns scheduling; each lane permits one flight.
use crate::model::{AppSettings, MarketDetails, MarketSource, Quote};
use chrono::{DateTime, FixedOffset};
use rust_decimal::Decimal;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct QuoteRequest {
    pub id: u64,
    pub generation: u64,
    pub symbols: Vec<String>,
    pub source: MarketSource,
    pub cancellation: CancellationToken,
}
#[derive(Clone, Debug)]
pub enum FetchFailure {
    Timeout,
    Failed(String),
}
#[derive(Clone, Debug, Default)]
pub struct QuoteSnapshot {
    pub quotes: HashMap<String, Arc<Quote>>,
    pub fresh_symbols: HashSet<String>,
    pub error: Option<String>,
    pub last_success: Option<DateTime<FixedOffset>>,
    pub busy: bool,
}
pub struct QuoteSession {
    settings: AppSettings,
    snapshot: QuoteSnapshot,
    generation: u64,
    serial: u64,
    active: Option<QuoteRequest>,
    disposed: bool,
}
impl QuoteSession {
    pub fn new(settings: AppSettings) -> Self {
        Self {
            settings,
            snapshot: QuoteSnapshot::default(),
            generation: 0,
            serial: 0,
            active: None,
            disposed: false,
        }
    }
    pub fn snapshot(&self) -> QuoteSnapshot {
        self.snapshot.clone()
    }
    pub fn snapshot_ref(&self) -> &QuoteSnapshot {
        &self.snapshot
    }
    pub fn update_settings(&mut self, value: AppSettings) {
        if self.disposed {
            return;
        }
        if value.source != self.settings.source {
            self.snapshot.quotes.clear();
            self.snapshot.last_success = None;
        }
        self.snapshot
            .quotes
            .retain(|symbol, _| value.symbols.contains(symbol));
        self.settings = value;
        self.generation = self.generation.wrapping_add(1);
        self.snapshot.fresh_symbols.clear();
        self.snapshot.error = None;
        if let Some(active) = &self.active {
            active.cancellation.cancel();
        }
    }
    pub fn begin_refresh(&mut self) -> Option<QuoteRequest> {
        if self.disposed || self.active.is_some() || self.settings.symbols.is_empty() {
            return None;
        }
        self.serial = self.serial.wrapping_add(1);
        let request = QuoteRequest {
            id: self.serial,
            generation: self.generation,
            symbols: self.settings.symbols.clone(),
            source: self.settings.source,
            cancellation: CancellationToken::new(),
        };
        self.active = Some(request.clone());
        self.snapshot.busy = true;
        Some(request)
    }
    pub fn complete(
        &mut self,
        request: &QuoteRequest,
        result: Result<Vec<Quote>, FetchFailure>,
        now: DateTime<FixedOffset>,
    ) -> bool {
        if self.active.as_ref().map(|active| active.id) != Some(request.id) {
            return false;
        }
        self.active = None;
        self.snapshot.busy = false;
        if self.disposed
            || request.generation != self.generation
            || request.cancellation.is_cancelled()
        {
            return false;
        }
        self.snapshot.fresh_symbols.clear();
        self.snapshot.error = match result {
            Ok(quotes) => {
                for quote in quotes {
                    if quote.price <= Decimal::ZERO || !request.symbols.contains(&quote.symbol) {
                        continue;
                    }
                    self.snapshot.fresh_symbols.insert(quote.symbol.clone());
                    self.snapshot
                        .quotes
                        .insert(quote.symbol.clone(), Arc::new(quote));
                }
                let count = self.snapshot.fresh_symbols.len();
                if count > 0 {
                    self.snapshot.last_success = Some(now);
                }
                if count == request.symbols.len() {
                    None
                } else if count == 0 {
                    Some("未收到有效行情，保留上次报价".into())
                } else {
                    Some("部分代码无新行情，灰色报价为上次数据".into())
                }
            }
            Err(FetchFailure::Timeout) => Some("请求超时，保留上次报价".into()),
            Err(FetchFailure::Failed(_)) => Some("请求失败，保留上次报价".into()),
        };
        true
    }
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.generation = self.generation.wrapping_add(1);
        if let Some(active) = &self.active {
            active.cancellation.cancel();
        }
    }
}
impl Drop for QuoteSession {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct DetailKey {
    pub source: MarketSource,
    pub symbol: String,
    pub daily: bool,
}
#[derive(Clone, Debug)]
pub struct DetailsRequest {
    pub id: u64,
    pub generation: u64,
    started_at: Duration,
    pub quote: Arc<Quote>,
    pub key: DetailKey,
    pub cancellation: CancellationToken,
}
#[derive(Clone, Debug, Default)]
pub struct DetailsSnapshot {
    pub data: Option<Arc<MarketDetails>>,
    pub error: Option<String>,
    pub stale: bool,
}
struct Entry {
    data: Option<Arc<MarketDetails>>,
    error: Option<String>,
    started_at: Duration,
    received_at: Duration,
    serial: u64,
}
pub struct DetailsSession {
    quote: Option<Arc<Quote>>,
    key: Option<DetailKey>,
    cache: HashMap<DetailKey, Entry>,
    generation: u64,
    serial: u64,
    active: Option<DetailsRequest>,
    disposed: bool,
}
impl Default for DetailsSession {
    fn default() -> Self {
        Self::new()
    }
}
impl DetailsSession {
    pub fn new() -> Self {
        Self {
            quote: None,
            key: None,
            cache: HashMap::new(),
            generation: 0,
            serial: 0,
            active: None,
            disposed: false,
        }
    }
    pub fn select(&mut self, quote: Option<Arc<Quote>>, source: MarketSource, daily: bool) {
        let next = quote.as_ref().map(|q| DetailKey {
            source,
            symbol: q.symbol.clone(),
            daily,
        });
        if self.key != next {
            self.generation = self.generation.wrapping_add(1);
            if let Some(active) = &self.active {
                active.cancellation.cancel();
            }
        }
        self.quote = quote;
        self.key = next;
    }
    pub fn deactivate(&mut self) {
        self.select(None, MarketSource::Tencent, false);
    }
    fn interval(&self, override_interval: Option<Duration>) -> Duration {
        override_interval.unwrap_or_else(|| {
            Duration::from_secs(if self.key.as_ref().is_some_and(|key| key.daily) {
                60
            } else {
                5
            })
        })
    }
    pub fn snapshot(&self, now: Duration, interval: Option<Duration>) -> DetailsSnapshot {
        if let Some(entry) = self.key.as_ref().and_then(|key| self.cache.get(key)) {
            DetailsSnapshot {
                data: entry.data.clone(),
                error: entry.error.clone(),
                stale: entry.error.is_some()
                    || now.saturating_sub(entry.received_at) >= self.interval(interval),
            }
        } else {
            DetailsSnapshot::default()
        }
    }
    pub fn begin_refresh(
        &mut self,
        force: bool,
        now: Duration,
        interval: Option<Duration>,
    ) -> Option<DetailsRequest> {
        if self.disposed || self.active.is_some() {
            return None;
        }
        let key = self.key.clone()?;
        if !force && let Some(entry) = self.cache.get(&key) {
            // Pace trade polling from dispatch, but give a received chart its full cache lifetime.
            let since = if interval.is_some() {
                entry.started_at
            } else {
                entry.received_at
            };
            if now.saturating_sub(since) < self.interval(interval) {
                return None;
            }
        }
        let quote = self.quote.clone()?;
        self.serial = self.serial.wrapping_add(1);
        let request = DetailsRequest {
            id: self.serial,
            generation: self.generation,
            started_at: now,
            quote,
            key,
            cancellation: CancellationToken::new(),
        };
        self.active = Some(request.clone());
        Some(request)
    }
    pub fn complete(
        &mut self,
        request: &DetailsRequest,
        result: Result<MarketDetails, String>,
        now: Duration,
    ) -> bool {
        if self.active.as_ref().map(|active| active.id) != Some(request.id) {
            return false;
        }
        self.active = None;
        if self.disposed
            || request.generation != self.generation
            || request.cancellation.is_cancelled()
        {
            return false;
        }
        let result = result.and_then(|data| {
            if data.symbol == request.quote.symbol {
                Ok(data)
            } else {
                Err("盘口返回的代码不匹配。".into())
            }
        });
        let (data, error) = match result {
            Ok(mut data) => {
                if data.error.is_some() {
                    if let Some(old) = self
                        .cache
                        .get(&request.key)
                        .and_then(|entry| entry.data.as_ref())
                    {
                        if data.minutes.is_empty() {
                            data.minutes = old.minutes.clone();
                        }
                        if data.daily.is_empty() {
                            data.daily = old.daily.clone();
                        }
                        if data.trades.is_empty() {
                            data.trades = old.trades.clone();
                        }
                    }
                }
                let error = data.error.clone();
                (Some(Arc::new(data)), error)
            }
            Err(error) => (
                self.cache
                    .get(&request.key)
                    .and_then(|entry| entry.data.clone()),
                Some(format!("详情获取失败，保留上次数据：{error}")),
            ),
        };
        if !self.cache.contains_key(&request.key) && self.cache.len() >= 16 {
            if let Some(key) = self
                .cache
                .iter()
                .min_by_key(|(_, entry)| (entry.received_at, entry.serial))
                .map(|(key, _)| key.clone())
            {
                self.cache.remove(&key);
            }
        }
        self.cache.insert(
            request.key.clone(),
            Entry {
                data,
                error,
                started_at: request.started_at,
                received_at: now,
                serial: request.id,
            },
        );
        true
    }
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.deactivate();
    }
}
impl Drop for DetailsSession {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MinutePoint;
    fn quote(symbol: &str, price: i64) -> Quote {
        Quote {
            symbol: symbol.into(),
            price: Decimal::from(price),
            ..Quote::default()
        }
    }
    fn settings() -> AppSettings {
        AppSettings {
            symbols: vec!["600000.SH".into(), "000001.SZ".into()],
            ..AppSettings::default()
        }
    }
    fn now() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-11-17T10:00:00+08:00").unwrap()
    }
    #[test]
    fn partial_quotes_preserve_prior_same_source_values_and_freshness() {
        let mut session = QuoteSession::new(settings());
        let first = session.begin_refresh().unwrap();
        assert!(session.begin_refresh().is_none());
        assert!(session.complete(
            &first,
            Ok(vec![quote("600000.SH", 10), quote("000001.SZ", 20)]),
            now()
        ));
        let second = session.begin_refresh().unwrap();
        session.complete(
            &second,
            Ok(vec![
                quote("600000.SH", 11),
                quote("999999.SH", 1),
                quote("000001.SZ", 0),
            ]),
            now(),
        );
        let snapshot = session.snapshot();
        assert_eq!(snapshot.quotes.len(), 2);
        assert_eq!(snapshot.quotes["000001.SZ"].price, Decimal::from(20));
        assert_eq!(snapshot.fresh_symbols, HashSet::from(["600000.SH".into()]));
        assert!(snapshot.error.is_some());
        let third = session.begin_refresh().unwrap();
        session.complete(&third, Err(FetchFailure::Timeout), now());
        let failed = session.snapshot();
        assert_eq!(failed.quotes["600000.SH"].price, Decimal::from(11));
        assert!(failed.fresh_symbols.is_empty());
        assert_eq!(failed.last_success, Some(now()));
    }
    #[test]
    fn source_switch_cancels_in_flight_and_never_commits_old_source() {
        let mut session = QuoteSession::new(settings());
        let request = session.begin_refresh().unwrap();
        let mut next = settings();
        next.source = MarketSource::Eastmoney;
        session.update_settings(next);
        assert!(request.cancellation.is_cancelled());
        assert!(session.begin_refresh().is_none());
        assert!(!session.complete(&request, Ok(vec![quote("600000.SH", 10)]), now()));
        assert!(session.snapshot().quotes.is_empty());
        let current = session.begin_refresh().unwrap();
        assert_eq!(current.source, MarketSource::Eastmoney);
        assert!(!session.complete(&request, Ok(vec![quote("600000.SH", 12)]), now()));
        assert!(session.snapshot().busy);
        session.complete(&current, Ok(vec![quote("600000.SH", 13)]), now());
        assert_eq!(
            session.snapshot().quotes["600000.SH"].price,
            Decimal::from(13)
        );
    }
    #[test]
    fn removed_symbols_and_disposed_sessions_cannot_receive_late_values() {
        let mut session = QuoteSession::new(settings());
        let request = session.begin_refresh().unwrap();
        session.complete(
            &request,
            Ok(vec![quote("600000.SH", 10), quote("000001.SZ", 20)]),
            now(),
        );
        let mut next = settings();
        next.symbols.remove(0);
        session.update_settings(next);
        assert!(!session.snapshot().quotes.contains_key("600000.SH"));
        let request = session.begin_refresh().unwrap();
        session.dispose();
        assert!(!session.complete(&request, Ok(vec![quote("000001.SZ", 21)]), now()));
        assert!(session.begin_refresh().is_none());
    }
    #[test]
    fn trade_refresh_interval_counts_from_dispatch_and_freshness_from_receipt() {
        let mut session = DetailsSession::new();
        let stock = Arc::new(quote("600000.SH", 10));
        session.select(Some(stock.clone()), MarketSource::Tencent, false);
        let interval = Some(Duration::from_secs(3));
        let request = session
            .begin_refresh(false, Duration::ZERO, interval)
            .unwrap();
        session.complete(
            &request,
            Ok(MarketDetails {
                symbol: stock.symbol.clone(),
                ..Default::default()
            }),
            Duration::from_millis(400),
        );

        assert!(
            session
                .begin_refresh(false, Duration::from_secs(2), interval)
                .is_none()
        );
        let next_tick = Duration::from_secs(3);
        assert!(!session.snapshot(next_tick, interval).stale);
        assert!(session.begin_refresh(false, next_tick, interval).is_some());
        assert!(
            session
                .snapshot(Duration::from_millis(3400), interval)
                .stale
        );
        assert!(
            session
                .begin_refresh(false, Duration::from_secs(6), interval)
                .is_none()
        );
    }

    #[test]
    fn detail_mode_cache_and_source_identity_remain_separate() {
        let mut session = DetailsSession::new();
        let stock = Arc::new(quote("600000.SH", 10));
        session.select(Some(stock.clone()), MarketSource::Tencent, false);
        let request = session.begin_refresh(false, Duration::ZERO, None).unwrap();
        session.complete(
            &request,
            Ok(MarketDetails {
                symbol: stock.symbol.clone(),
                ..MarketDetails::default()
            }),
            Duration::ZERO,
        );
        assert!(
            session
                .begin_refresh(false, Duration::from_secs(4), None)
                .is_none()
        );
        assert!(session.snapshot(Duration::from_secs(5), None).stale);
        let request = session
            .begin_refresh(false, Duration::from_secs(5), None)
            .unwrap();
        session.select(Some(stock.clone()), MarketSource::Tencent, true);
        assert!(request.cancellation.is_cancelled());
        assert!(!session.complete(
            &request,
            Ok(MarketDetails {
                symbol: stock.symbol.clone(),
                ..MarketDetails::default()
            }),
            Duration::from_secs(5)
        ));
        assert!(
            session
                .snapshot(Duration::from_secs(5), None)
                .data
                .is_none()
        );
        let daily = session
            .begin_refresh(false, Duration::from_secs(5), None)
            .unwrap();
        session.complete(
            &daily,
            Ok(MarketDetails {
                symbol: stock.symbol.clone(),
                ..MarketDetails::default()
            }),
            Duration::from_secs(7),
        );
        assert!(
            session
                .begin_refresh(false, Duration::from_secs(65), None)
                .is_none()
        );
        session.select(Some(stock), MarketSource::Eastmoney, true);
        assert!(
            session
                .snapshot(Duration::from_secs(65), None)
                .data
                .is_none()
        );
    }
    #[test]
    fn failed_detail_refresh_keeps_last_good_chart_and_wrong_symbol_is_rejected() {
        let mut session = DetailsSession::new();
        let stock = Arc::new(quote("600000.SH", 10));
        session.select(Some(stock.clone()), MarketSource::Tencent, false);
        let first = session.begin_refresh(false, Duration::ZERO, None).unwrap();
        session.complete(
            &first,
            Ok(MarketDetails {
                symbol: stock.symbol.clone(),
                minutes: vec![MinutePoint {
                    time: "09:30".into(),
                    price: Decimal::from(10),
                    ..MinutePoint::default()
                }],
                ..MarketDetails::default()
            }),
            Duration::ZERO,
        );
        let second = session
            .begin_refresh(true, Duration::from_secs(1), None)
            .unwrap();
        session.complete(
            &second,
            Ok(MarketDetails {
                symbol: stock.symbol.clone(),
                error: Some("请求失败".into()),
                ..MarketDetails::default()
            }),
            Duration::from_secs(1),
        );
        assert_eq!(
            session
                .snapshot(Duration::from_secs(1), None)
                .data
                .unwrap()
                .minutes
                .len(),
            1
        );
        let third = session
            .begin_refresh(true, Duration::from_secs(2), None)
            .unwrap();
        session.complete(
            &third,
            Ok(MarketDetails {
                symbol: "000001.SZ".into(),
                ..MarketDetails::default()
            }),
            Duration::from_secs(2),
        );
        let snapshot = session.snapshot(Duration::from_secs(2), None);
        assert!(snapshot.stale);
        assert_eq!(snapshot.data.unwrap().symbol, stock.symbol);
        let request = session
            .begin_refresh(true, Duration::from_secs(3), None)
            .unwrap();
        session.deactivate();
        assert!(request.cancellation.is_cancelled());
        assert!(!session.complete(&request, Err("late".into()), Duration::from_secs(3)));
        assert!(
            session
                .snapshot(Duration::from_secs(3), None)
                .data
                .is_none()
        );
    }
}
