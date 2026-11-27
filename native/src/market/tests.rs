use super::*;
use crate::model::*;
use rust_decimal::Decimal;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Instant,
};
fn d(value: &str) -> Decimal {
    value.parse().unwrap()
}
fn sample() -> Quote {
    Quote {
        symbol: "600000.SH".into(),
        timestamp: "20261116142019".into(),
        ..Default::default()
    }
}
fn line(code: &str, name: &str, price: &str, close: &str) -> String {
    let mut fields = vec!["0"; 38];
    fields[1] = name;
    fields[2] = &code[2..];
    fields[3] = price;
    fields[4] = close;
    fields[6] = "1234";
    fields[30] = "20261116113000";
    fields[32] = "2.60";
    fields[37] = "800.25";
    fields[9] = "10.66";
    fields[10] = "123";
    fields[11] = "10.65";
    fields[12] = "bad";
    fields[13] = "10.64";
    fields[14] = "456";
    fields[19] = "10.67";
    fields[20] = "789";
    format!("v_{code}=\"{}\";", fields.join("~"))
}
struct Reply {
    status: u16,
    body: Vec<u8>,
    delay: Duration,
}
impl Reply {
    fn utf8(text: &str) -> Self {
        Self {
            status: 200,
            body: text.as_bytes().to_vec(),
            delay: Duration::ZERO,
        }
    }
    fn gbk(text: &str) -> Self {
        let (bytes, _, bad) = GBK.encode(text);
        assert!(!bad);
        Self {
            status: 200,
            body: bytes.into_owned(),
            delay: Duration::ZERO,
        }
    }
}
struct Server {
    origin: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(replies: Vec<Reply>) -> Self {
        Self::with_routes(replies.into_iter().map(|reply| ("", reply)).collect())
    }
    fn with_routes(mut replies: Vec<(&'static str, Reply)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let logged = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let handle = thread::spawn(move || {
            while !replies.is_empty() {
                let deadline = Instant::now() + Duration::from_secs(7);
                let mut stream = loop {
                    if stopping.load(Ordering::Relaxed) || Instant::now() > deadline {
                        return;
                    }
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2))
                        }
                        Err(e) => panic!("fixture accept: {e}"),
                    }
                };
                // Winsock accept inherits the listener's nonblocking mode.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 16384 {
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => request.extend_from_slice(&chunk[..n]),
                        Err(_) => break,
                    }
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let index = replies
                    .iter()
                    .position(|(path, _)| request.lines().next().unwrap_or_default().contains(path))
                    .expect("unexpected fixture route");
                let (_, reply) = replies.remove(index);
                logged.lock().unwrap().push(request);
                thread::sleep(reply.delay);
                let header = format!(
                    "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.status,
                    reply.body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&reply.body);
            }
        });
        Self {
            origin,
            requests,
            stop,
            handle: Some(handle),
        }
    }
    fn client(&self) -> MarketClient {
        let mut client = MarketClient::new().unwrap();
        client.test_origin = Some(self.origin.clone());
        client
    }
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            h.join().unwrap();
        }
    }
}

#[test]
fn decimal_precision_and_json_bad_root_are_preserved() {
    assert_eq!(
        number(&json!("12.12345678901234567890123456"))
            .unwrap()
            .to_string(),
        "12.12345678901234567890123456"
    );
    assert_eq!(number(&json!("1.25e2")), Some(d("125")));
    assert!(json_object("[]").is_err());
    assert!(json_object("{broken").is_err());
}
#[tokio::test]
async fn tencent_gbk_units_depth_and_prefixes_use_one_batch() {
    let body = line("sh600000", "浦发银行", "10.67", "10.40")
        + &line("sz159915", "创业板ETF", "2.383", "2.30")
        + &line("bj920001", "北京行", "20", "19");
    let server = Server::new(vec![Reply::gbk(&body)]);
    let client = server.client();
    let rows = client
        .fetch_quotes(
            &["600000.SH".into(), "159915.SZ".into(), "920001.BJ".into()],
            MarketSource::Tencent,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].starts_with("GET /q=sh600000,sz159915,bj920001 "));
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].name, "浦发银行");
    assert_eq!(rows[0].price, d("10.67"));
    assert_eq!(rows[0].amount_yuan, d("8002500"));
    assert_eq!(rows[0].volume_lots, d("1234"));
    assert_eq!(rows[0].bids.len(), 5);
    assert_eq!(rows[0].bids[0].volume_lots, d("123"));
    assert_eq!(rows[0].bids[1].price, Decimal::ZERO);
    assert_eq!(rows[0].bids[2].price, d("10.64"));
    assert_eq!(rows[0].asks[0].volume_lots, d("789"));
    assert_eq!(rows[1].price, d("2.383"));
    assert_eq!(rows[0].timestamp, "20261116113000");
}
#[tokio::test]
async fn older_quote_within_batch_cannot_replace_newer_duplicate() {
    let fresh = line("sh600000", "浦发银行", "10.67", "10.40");
    let older =
        line("sh600000", "浦发银行", "10.60", "10.40").replace("20261116113000", "20261115150000");
    let same_second = line("sh600000", "浦发银行", "10.68", "10.40");
    let server = Server::new(vec![Reply::gbk(&(fresh + &same_second + &older))]);
    let rows = server
        .client()
        .fetch_quotes(
            &["600000.SH".into()],
            MarketSource::Tencent,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].price, d("10.68"));
    assert_eq!(rows[0].timestamp, "20261116113000");
}

#[tokio::test]
async fn em_units_suspension_bad_rows_and_timestamp_match_v105() {
    let payload = r#"{"data":{"diff":[{"f12":"600000","f13":1,"f14":"浦发银行","f2":10.67,"f18":10.40,"f3":2.60,"f5":1234,"f6":8002500,"f124":1794799800},{"f12":"159915","f13":0,"f14":"创业板ETF","f2":0,"f18":2.15,"f3":"-","f5":"-","f6":"-"},{"f12":"920001","f13":0,"f14":"坏行","f2":"NaN","f18":10}]}}"#;
    let server = Server::new(vec![Reply::utf8(payload)]);
    let rows = server
        .client()
        .fetch_quotes(
            &["600000.SH".into(), "159915.SZ".into(), "920001.BJ".into()],
            MarketSource::Eastmoney,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].amount_yuan, d("8002500"));
    assert_eq!(rows[0].volume_lots, d("1234"));
    assert!(rows[0].bids.is_empty());
    assert_eq!(rows[0].timestamp, "20261116113000");
    assert_eq!(rows[1].price, d("2.15"));
    assert_eq!(rows[1].change_percent, Decimal::ZERO);
    assert!(server.requests()[0].contains("fltt=2&secids=1.600000,0.159915,0.920001"));
}
#[tokio::test]
async fn failed_batch_retains_other_batch_and_never_switches_source() {
    let mut failure = Reply::utf8("");
    failure.status = 503;
    let server = Server::new(vec![
        failure,
        Reply::gbk(&line("sz000061", "深圳自选", "15.20", "15")),
    ]);
    let symbols = (1..=61).map(|i| format!("{i:06}.SZ")).collect::<Vec<_>>();
    let rows = server
        .client()
        .fetch_quotes(&symbols, MarketSource::Tencent, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 2);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].symbol, "000061.SZ");
    assert_eq!(
        server.requests()[0]
            .split_whitespace()
            .nth(1)
            .unwrap()
            .split(',')
            .count(),
        60
    );
    let mut failure = Reply::utf8("");
    failure.status = 502;
    let server = Server::new(vec![failure]);
    assert!(
        server
            .client()
            .fetch_quotes(
                &["600000.SH".into()],
                MarketSource::Eastmoney,
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].contains("/api/qt/ulist.np/get"));
}
#[tokio::test]
async fn aliases_are_removed_before_batching_and_empty_or_invalid_never_request() {
    let server = Server::new(vec![]);
    let client = server.client();
    assert!(
        client
            .fetch_quotes(&[], MarketSource::Tencent, CancellationToken::new())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        client
            .fetch_quotes(
                &["bad".into()],
                MarketSource::Tencent,
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert!(
        client
            .fetch_quotes(
                &["000001.SZ".into(), "000001.BJ".into()],
                MarketSource::Eastmoney,
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
    let mut symbols = vec!["000001.SZ".into()];
    symbols.extend((2..=60).map(|i| format!("{i:06}.SZ")));
    symbols.push("000001.BJ".into());
    symbols.push("600000.SH".into());
    let server = Server::new(vec![Reply::utf8(
        r#"{"data":{"diff":[{"f12":"600000","f13":1,"f14":"浦发银行","f2":10.67,"f18":10.4}]}}"#,
    )]);
    let rows = server
        .client()
        .fetch_quotes(&symbols, MarketSource::Eastmoney, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(server.requests().iter().all(|r| !r.contains("0.000001")));
}
#[test]
fn bad_duplicate_overflowed_rows_cannot_pollute_other_quotes() {
    let body = line("sh600000", "溢出", "79228162514264337593543950335", "0.01")
        .replace("~2.60~", "~-~")
        + &line("sh600000", "错配代码", "9", "9").replace("~600000~", "~600519~")
        + &line("sz000001", "平安银行", "0", "12.30")
        + &line("sz000001", "平安银行", "12.50", "12.30");
    let map = HashMap::from([
        ("sh600000".into(), "600000.SH".into()),
        ("sz000001".into(), "000001.SZ".into()),
    ]);
    let parsed = quotes::parse(&body, MarketSource::Tencent, &map).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].price, d("12.30"));
    assert_eq!(parsed[1].price, d("12.50"));
}
#[test]
fn tencent_index_points_do_not_use_constituent_turnover_as_an_average() {
    let parse = |symbol: &str, code: &str, kind: &str, rows: &[&str]| {
        let mut quote_fields = vec![""; 62];
        quote_fields[61] = kind;
        let payload = json!({"data": {(code): {
            "qt": {(code): quote_fields},
            "data": {"data": rows}
        }}});
        details::parse_minutes(&payload, symbol, code, MarketSource::Tencent).unwrap()
    };
    let index = parse(
        "000001.SH",
        "sh000001",
        "ZS",
        &[
            "0930 3490.14 4730979 5408591854.20",
            "0931 3485.92 22438250 27623180891.90",
        ],
    );
    assert_eq!(index.len(), 2);
    assert_eq!(index[1].price, d("3485.92"));
    assert_eq!(index[1].volume_lots, d("17707271"));
    assert!(index.iter().all(|point| point.average.is_zero()));

    let bank = parse(
        "000001.SZ",
        "sz000001",
        "GP-A",
        &["0930 12 100 120000", "0931 12.20 200 242000"],
    );
    assert_eq!(bank[1].average, d("12.10"));
}

fn shanghai_snapshot(day: &str, open: &str) -> String {
    let mut fields = vec![""; 33];
    fields[0] = "上证指数";
    fields[1] = open;
    fields[30] = day;
    fields[31] = "15:00:00";
    format!(r#"var hq_str_sh000001="{}";"#, fields.join(","))
}

#[tokio::test]
async fn shanghai_overlay_joins_matching_minutes_without_replacing_prices_or_averages() {
    let tencent = json!({"data":{"sh000001":{"data":{"date":"20261126","data":[
        "0930 3490.14 4730979", "0931 3485.92 22438250", "0932 3488.33 31565773"
    ]}}}});
    let eastmoney = json!({"data":{"code":"000001","trends":[
        "2026-11-26 09:30,3490.14,3490.14,3490.14,3490.14,4730979,5408591872,3504.776",
        "2026-11-26 09:31,3487.05,3485.92,3487.77,3481.19,17707271,22214588928,3489.388",
        "2026-11-26 09:32,3487,3488.33,3490,3486,9127523,11810454000,3492"
    ]}});
    let sina = json!({"result":{"status":{"code":0},"data":[
        {"m":"09:32:00","p":"3487.724","lx":"-","avg_p":"3486.923"},
        {"m":"09:30:00","p":"3490.145","lx":"3506.401","avg_p":"3490.145"},
        {"m":"09:31:00","p":"3486.088","lx":"3492.102","avg_p":"3486.145"},
        {"m":"09:33:00","p":"3487.3","lx":"3491.2"}
    ]}});
    for (source, path, data) in [
        (MarketSource::Tencent, "/minute/query", tencent),
        (MarketSource::Eastmoney, "/stock/trends2/get", eastmoney),
    ] {
        let server = Server::with_routes(vec![
            (path, Reply::utf8(&data.to_string())),
            ("/minline/getMinlineData", Reply::utf8(&sina.to_string())),
            (
                "/list=sh000001",
                Reply::gbk(&shanghai_snapshot("2026-11-26", "3490.1446")),
            ),
        ]);
        let quote = Quote {
            symbol: "000001.SH".into(),
            ..Default::default()
        };
        let chart = server
            .client()
            .fetch_details(&quote, source, false, CancellationToken::new())
            .await
            .unwrap();
        assert!(chart.error.is_none());
        assert_eq!(chart.minutes.len(), 3);
        assert_eq!(chart.minutes[1].price, d("3485.92"));
        assert_eq!(
            chart.minutes.iter().map(|p| p.leading).collect::<Vec<_>>(),
            [d("3506.401"), d("3492.102"), Decimal::ZERO]
        );
    }
}

#[tokio::test]
async fn shanghai_price_line_survives_an_unavailable_leading_feed() {
    let chart = json!({"data":{"sh000001":{"data":{"date":"20261126","data":[
        "0930 3490.14 4730979"
    ]}}}});
    let server = Server::with_routes(vec![
        ("/minute/query", Reply::utf8(&chart.to_string())),
        ("/minline/getMinlineData", Reply::utf8("null")),
        (
            "/list=sh000001",
            Reply::gbk(&shanghai_snapshot("2026-11-26", "3490.1446")),
        ),
    ]);
    let quote = Quote {
        symbol: SHANGHAI_COMPOSITE.into(),
        ..Default::default()
    };
    let chart = server
        .client()
        .fetch_details(
            &quote,
            MarketSource::Tencent,
            false,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(chart.error.is_none());
    assert_eq!(chart.minutes[0].price, d("3490.14"));
    assert!(chart.minutes[0].leading.is_zero());
    let server = Server::new(vec![Reply::utf8(
        r#"{"data":{"sh000001":{"day":[["2026-11-26","3490.14","3476.88","3497.71","3450.22"]]}}}"#,
    )]);
    let chart = server
        .client()
        .fetch_details(
            &quote,
            MarketSource::Tencent,
            true,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(chart.daily.len(), 1);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn shanghai_leading_rejects_stale_batches_and_keeps_legacy_average_separate() {
    let minute: MinutePoint =
        serde_json::from_value(json!({"Time":"09:30","Price":3490.14,"Average":11.43})).unwrap();
    assert_eq!(minute.overlay("000001.SH"), Decimal::ZERO);
    assert_eq!(minute.overlay("000001.SZ"), d("11.43"));
    let sina = json!({"result":{"status":{"code":0},"data":[{"m":"09:30:00","p":"3490.145","lx":"3506.401"}]}});
    let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
    for snapshot in [
        shanghai_snapshot("2026-11-25", "3490.1446"),
        shanghai_snapshot("2026-11-26", "3500.2"),
        shanghai_snapshot("2026-11-26", "3490.1446").replace("sh000001", "sz000001"),
    ] {
        let mut points = vec![minute.clone()];
        details::apply_shanghai_leading(&mut points, day, &snapshot, &sina);
        assert_eq!(points[0], minute);
    }
}

#[test]
fn shanghai_mixed_session_dates_do_not_produce_a_leading_line() {
    let data = json!({"data":{"market":1,"code":"000001","trends":[
        "2026-11-25 09:30,3480,3480,3480,3480,100,348000", "2026-11-26 09:31,3490,3490,3490,3490,200,698000"
    ]}});
    assert!(details::minute_session_day(&data, "1.000001", MarketSource::Eastmoney).is_err());
    let minute = MinutePoint {
        time: "09:30".into(),
        price: d("3490.14"),
        ..Default::default()
    };
    let mut points = vec![minute.clone()];
    let data = json!({"result":{"status":{"code":0},"data":[
        {"m":"09:30:00","p":"3490.145","lx":"3506.401"},
        {"m":"09:31:00","d":"2026-11-25","p":"3486.088","lx":"3492.102"}
    ]}});
    details::apply_shanghai_leading(
        &mut points,
        chrono::NaiveDate::from_ymd_opt(2026, 11, 26).unwrap(),
        &shanghai_snapshot("2026-11-26", "3490.1446"),
        &data,
    );
    assert_eq!(points, [minute]);
}

#[test]
fn eastmoney_index_keeps_the_average_provided_in_points() {
    let payload = json!({"data":{"code":"000001","trends":[
        "2026-11-26 09:30,3490.14,3490.14,3490.14,3490.14,4730979,5408591872.00,3504.776"
    ]}});
    let minutes =
        details::parse_minutes(&payload, "000001.SH", "1.000001", MarketSource::Eastmoney).unwrap();
    assert_eq!(minutes[0].price, d("3490.14"));
    assert_eq!(minutes[0].average, d("3504.776"));
}

#[test]
fn missing_minute_average_does_not_substitute_the_last_price() {
    let tencent = json!({"data":{"sh600000":{"data":{"data":[
        "0930 10 0 0", "0931 10 100", "0932 10 120 0"
    ]}}}});
    let eastmoney = json!({"data":{"code":"600000","trends":[
        "2026-11-16 09:30,10,10,10,10,30,30000,-",
        "2026-11-16 09:31,10,10,10,10,40,40000",
        "2026-11-16 09:32,10,10,10,10,40,40000,0"
    ]}});
    for (source, payload) in [
        (MarketSource::Tencent, tencent),
        (MarketSource::Eastmoney, eastmoney),
    ] {
        let minutes = details::parse_minutes(&payload, "600000.SH", "sh600000", source).unwrap();
        assert_eq!(minutes.len(), 3);
        assert!(
            minutes
                .iter()
                .all(|m| m.price == d("10") && m.average.is_zero())
        );
    }
}

#[tokio::test]
async fn minute_details_one_request_units_sort_and_identity() {
    let payload = r#"{"data":{"sh600000":{"data":{"date":"20261116","data":["0931 10.2 150 150500","0930 10 100 100000","1200 20 200 200000","bad","0932 NaN 200 200000"]}}}}"#;
    let server = Server::new(vec![Reply::utf8(payload)]);
    let details = server
        .client()
        .fetch_details(
            &sample(),
            MarketSource::Tencent,
            false,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].contains("/minute/query"));
    assert_eq!(details.minutes.len(), 2);
    assert_eq!(details.minutes[0].volume_lots, d("100"));
    assert_eq!(details.minutes[1].volume_lots, d("50"));
    assert_eq!(
        details.minutes[1].average,
        d("150500") / d("150") / d("100")
    );
    assert!(details.trades.is_empty());
    assert!(details.error.is_none());
    let server = Server::new(vec![Reply::utf8(
        r#"{"data":{"code":"600000","trends":["2026-11-16 09:30,10,10.1,10.2,9.9,30,30300,10.1","2026-11-16 09:31,10,10.2,10.3,10,40,40800,10.16"]}}"#,
    )]);
    let details = server
        .client()
        .fetch_details(
            &sample(),
            MarketSource::Eastmoney,
            false,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 1);
    assert_eq!(details.minutes[1].volume_lots, d("40"));
    assert_eq!(details.minutes[1].average, d("10.16"));
    let server = Server::new(vec![Reply::utf8(
        r#"{"data":{"code":"600519","trends":[]}}"#,
    )]);
    let details = server
        .client()
        .fetch_details(
            &sample(),
            MarketSource::Eastmoney,
            false,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(details.error.is_some());
    assert!(details.minutes.is_empty());
}
#[tokio::test]
async fn quote_day_keeps_intraday_prices_from_mixing_sessions() {
    let current = r#"{"data":{"sh600000":{"data":{"date":"20261116","data":["0930 10 100"]}}}}"#;
    let old = current.replace("20261116", "20261113");
    let mixed = r#"{"data":{"code":"600000","trends":["2026-11-13 09:30,10,10,10,10,10","2026-11-16 09:31,11,11,11,11,20"]}}"#;
    let missing = r#"{"data":{"sh600000":{"data":{"data":["0930 10 100"]}}}}"#;
    let old_em =
        r#"{"data":{"code":"600000","market":1,"trends":["2026-11-13 09:30,10,10,10,10,10"]}}"#;
    for (source, body, quote_dated, valid) in [
        (MarketSource::Tencent, current, true, true),
        (MarketSource::Tencent, old.as_str(), true, false),
        (MarketSource::Tencent, missing, true, false),
        (MarketSource::Tencent, old.as_str(), false, true),
        (MarketSource::Tencent, missing, false, true),
        (MarketSource::Eastmoney, old_em, true, false),
        (MarketSource::Eastmoney, old_em, false, true),
        (MarketSource::Eastmoney, mixed, true, false),
        (MarketSource::Eastmoney, mixed, false, false),
    ] {
        let mut stock = sample();
        if !quote_dated {
            stock.timestamp.clear();
        }
        let server = Server::new(vec![Reply::utf8(body)]);
        let chart = server
            .client()
            .fetch_details(&stock, source, false, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(!chart.minutes.is_empty(), valid);
        assert_eq!(chart.error.is_none(), valid);
    }
}

#[test]
fn daily_fallback_fields_invalid_rows_and_sixty_limit() {
    let payload = json!({"data":{"sh600000":{"qfqday":[],"day":[["2026-11-13","10","10.5","11","9.5"],["2026-11-16","10","bad","11","9.5"]]}}});
    let rows =
        details::parse_daily(&payload, "600000.SH", "sh600000", MarketSource::Tencent).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].close, d("10.5"));
    let rows = (0..70)
        .map(|i| {
            json!([
                (chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(i))
                    .to_string(),
                "10",
                "10.5",
                "11",
                "9.5"
            ])
        })
        .collect::<Vec<_>>();
    let payload = json!({"data":{"sh600000":{"qfqday":rows}}});
    assert_eq!(
        details::parse_daily(&payload, "600000.SH", "sh600000", MarketSource::Tencent)
            .unwrap()
            .len(),
        60
    );
}
fn metadata(code: &str, date: &str, ranges: &str) -> String {
    format!("v_detail_time_{code}=[{date},\"{ranges}\"];")
}
fn page(code: &str, n: usize, rows: &str) -> String {
    format!("v_detail_data_{code}=[{n},\"{rows}\"];")
}
const RANGES: &str = "09:25:00~09:33:24|09:33:27~14:18:45|14:18:48~14:20:15";
#[test]
fn tencent_same_second_trades_keep_the_latest_sequence_numbers() {
    let rows = (1..=25)
        .map(|sequence| format!("{sequence}/14:20:15/10/0/{sequence}/0/B"))
        .collect::<Vec<_>>()
        .join("|");
    let trades =
        details::parse_recent_page(&[json!(0), json!(rows)], 0, chrono::NaiveTime::MIN).unwrap();
    let trades = details::latest(trades.into_values().rev().collect());
    let lots: Vec<_> = trades.iter().map(|trade| trade.volume_lots).collect();
    assert_eq!(lots, (6..=25).rev().map(Decimal::from).collect::<Vec<_>>());
}
#[tokio::test]
async fn recent_trades_first_page_units_and_direction() {
    let rows = "140/14:20:09/10.70/0.01/2/2140/B|141/14:20:12/10.69/-0.01/3/3207/S|142/14:20:15/10.70/0.01/5/5350/M|bad|143/14:20:18/NaN/0/10/100/B";
    let server = Server::new(vec![
        Reply::utf8(&metadata("sh600000", "20261116", "14:18:48~14:20:15")),
        Reply::utf8(&page("sh600000", 0, rows)),
    ]);
    let result = server
        .client()
        .fetch_recent_trades(&sample(), MarketSource::Tencent, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 2);
    assert!(server.requests()[0].contains("action=recent_data"));
    assert!(server.requests()[1].contains("action=data&c=sh600000&p=0"));
    assert_eq!(result.trades.len(), 3);
    assert_eq!(result.trades[0].time, "14:20:15");
    assert_eq!(result.trades[0].volume_lots, d("5"));
    assert_eq!(
        result.trades.iter().map(|t| t.side).collect::<Vec<_>>(),
        [0, 2, 1]
    );
}
#[tokio::test]
async fn tencent_recent_backfills_a_short_tail_by_sequence_across_pages() {
    let ranges = "09:30:00~14:36:57|14:36:57~14:36:57";
    let previous = (4271..=4340)
        .map(|id| format!("{id}/14:36:57/133.91/0.00/35/466944/S"))
        .collect::<Vec<_>>()
        .join("|");
    let server = Server::new(vec![
        Reply::utf8(&metadata("sh600000", "20261116", ranges)),
        Reply::utf8(&page(
            "sh600000",
            1,
            "4340/14:36:57/133.94/0.03/48/645967/B",
        )),
        Reply::utf8(&page("sh600000", 0, &previous)),
    ]);
    let details = server
        .client()
        .fetch_recent_trades(&sample(), MarketSource::Tencent, CancellationToken::new())
        .await
        .unwrap();
    assert!(details.trades.len() >= 10 && details.trades.len() <= 20);
    assert_eq!(details.trades[0].price, d("133.94"));
    assert!(
        details.trades[1..]
            .iter()
            .all(|row| row.price == d("133.91"))
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].contains("&p=0"));
}

#[tokio::test]
async fn tencent_recent_skips_unneeded_backfill_and_keeps_tail_on_backfill_failure() {
    let ranges = "09:30:00~14:36:54|14:36:57~14:36:57";
    for tail_size in [1, 10] {
        let rows = (1..=tail_size)
            .map(|i| format!("{i}/14:36:57/133.94/0.03/48/645967/B"))
            .collect::<Vec<_>>()
            .join("|");
        let mut replies = vec![
            Reply::utf8(&metadata("sh600000", "20261116", ranges)),
            Reply::utf8(&page("sh600000", 1, &rows)),
        ];
        if tail_size == 1 {
            let mut failure = Reply::utf8("");
            failure.status = 503;
            replies.push(failure);
        }
        let server = Server::new(replies);
        let stock = Quote {
            timestamp: String::new(),
            ..sample()
        };
        let details = server
            .client()
            .fetch_recent_trades(&stock, MarketSource::Tencent, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(details.trades.len(), tail_size);
        assert_eq!(server.requests().len(), if tail_size == 1 { 3 } else { 2 });
    }
}

#[tokio::test]
async fn tencent_recent_empty_shanghai_index_is_optional_but_stock_errors_survive() {
    for (symbol, ok) in [
        ("000001.SH", true),
        ("000688.SH", true),
        ("000001.SZ", false),
        ("600000.SH", false),
    ] {
        let server = Server::new(vec![Reply::utf8("  ")]);
        let stock = Quote {
            symbol: symbol.into(),
            ..sample()
        };
        let result = server
            .client()
            .fetch_recent_trades(&stock, MarketSource::Tencent, CancellationToken::new())
            .await;
        assert_eq!(result.is_ok(), ok, "{symbol}");
        if ok {
            assert!(result.unwrap().trades.is_empty());
        }
    }
}

#[tokio::test]
async fn recent_metadata_page_failure_and_empty_remain_distinct_no_fallback() {
    for bad in [
        metadata("sz600000", "20261116", RANGES),
        metadata("sh600000", "20261115", RANGES),
        metadata("sh600000", "20261316", RANGES),
    ] {
        let server = Server::new(vec![Reply::utf8(&bad)]);
        assert!(
            server
                .client()
                .fetch_recent_trades(&sample(), MarketSource::Tencent, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 1);
    }
    for bad in [
        page("sh600519", 2, "142/14:20:15/10.7/0/5/5350/B"),
        page("sh600000", 0, "142/14:20:15/10.7/0/5/5350/B"),
        page("sh600000", 2, "0/09:30:00/10.7/0/5/5350/B"),
        page("sh600000", 2, ""),
    ] {
        let server = Server::new(vec![
            Reply::utf8(&metadata("sh600000", "20261116", RANGES)),
            Reply::utf8(&bad),
        ]);
        assert!(
            server
                .client()
                .fetch_recent_trades(&sample(), MarketSource::Tencent, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 2);
    }
    let server = Server::new(vec![Reply::utf8(&metadata("sh600000", "20261116", ""))]);
    assert!(
        server
            .client()
            .fetch_recent_trades(&sample(), MarketSource::Tencent, CancellationToken::new())
            .await
            .unwrap()
            .trades
            .is_empty()
    );
    assert_eq!(server.requests().len(), 1);
    let mut failure = Reply::utf8("");
    failure.status = 503;
    let server = Server::new(vec![
        Reply::utf8(&metadata("sh600000", "20261116", RANGES)),
        failure,
    ]);
    assert!(
        server
            .client()
            .fetch_recent_trades(&sample(), MarketSource::Tencent, CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 2);
}
#[tokio::test]
async fn em_recent_single_source_and_latest_twenty() {
    let server = Server::new(vec![Reply::utf8(
        r#"{"rc":0,"data":{"code":"600000","details":["14:20:12,10.69,3,1,2","14:20:15,10.70,5,1,1","bad"]}}"#,
    )]);
    let result = server
        .client()
        .fetch_recent_trades(&sample(), MarketSource::Eastmoney, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].contains("/details/get"));
    assert_eq!(result.trades[0].side, 1);
    assert_eq!(result.trades[0].volume_lots, d("5"));
    for bad in [
        json!({"rc":1,"data":null}),
        json!({"data":{"code":"600519","details":[]}}),
        json!({"data":{"code":"600000"}}),
        json!({"data":{"details":["bad"]}}),
    ] {
        assert!(details::parse_em_trades(&bad, "600000.SH").is_err());
    }
    assert!(
        details::parse_em_trades(&json!({"data":null}), "600000.SH")
            .unwrap()
            .is_empty()
    );
    let rows = (0..30)
        .map(|i| format!("{i}/14:18:{i:02}/10.7/0/1/1070/B"))
        .collect::<Vec<_>>()
        .join("|");
    let records =
        details::tencent_array(&page("sh600000", 0, &rows), "v_detail_data_sh600000").unwrap();
    let result = details::parse_recent_page(&records, 0, chrono::NaiveTime::MIN).unwrap();
    let result = details::latest(result.into_values().rev().collect());
    assert_eq!(result.len(), 20);
    assert_eq!(result[0].time, "14:18:29");
}
#[tokio::test]
async fn search_names_exact_markets_batch_empty_invalid_gbk_and_http_failures() {
    let hint = r#"v_hint="sh~000001~上证指数~szzs^sz~000001~平安银行~payh^sh~000001~重复~dup^us~AAPL~苹果~pg^bj~920001~北京行~bjh^sz~000002~~blank";"#;
    let server = Server::new(vec![Reply::gbk(hint), Reply::gbk(hint)]);
    let client = server.client();
    let result = client
        .search("000001", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].name, "上证指数");
    assert_eq!(result[1].symbol, "000001.SZ");
    assert_eq!(
        client
            .search("000001.SH", CancellationToken::new())
            .await
            .unwrap()
            .len(),
        1
    );
    let server = Server::new(vec![Reply::gbk(
        &(line("sh600519", "贵州茅台", "1500", "1490")
            + &line("sz000001", "平安银行", "12.3", "12")),
    )]);
    let result = server
        .client()
        .search("600519 000001,600519.SH", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 1);
    assert_eq!(
        result.iter().map(|s| s.symbol.as_str()).collect::<Vec<_>>(),
        ["600519.SH", "000001.SZ"]
    );
    let server = Server::new(vec![]);
    assert!(
        server
            .client()
            .search("", CancellationToken::new())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        server
            .client()
            .search("600519 bad", CancellationToken::new())
            .await
            .is_err()
    );
    assert!(
        server
            .client()
            .search(&"a".repeat(41), CancellationToken::new())
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
    assert!(
        search::suggestions("v_hint=\"N\";", None, None)
            .unwrap()
            .is_empty()
    );
    assert!(search::suggestions("<html>", None, None).is_err());
    let server = Server::new(vec![Reply {
        status: 200,
        body: vec![0x81, 0x20],
        delay: Duration::ZERO,
    }]);
    assert!(
        server
            .client()
            .search("gzmt", CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("GBK")
    );
    let server = Server::new(vec![Reply {
        status: 502,
        body: Vec::new(),
        delay: Duration::ZERO,
    }]);
    assert!(
        server
            .client()
            .search("gzmt", CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 1);
}
#[tokio::test]
async fn search_encoding_cap_and_quote_empty_bodies_fail_explicitly() {
    let server = Server::new(vec![Reply::gbk("v_hint=\"N\";")]);
    assert!(
        server
            .client()
            .search("a&b", CancellationToken::new())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(server.requests()[0].contains("q=a%26b&t=all"));
    let server = Server::new(vec![Reply {
        status: 200,
        body: vec![b' '; 256 * 1024 + 1],
        delay: Duration::ZERO,
    }]);
    assert!(
        server
            .client()
            .search("gzmt", CancellationToken::new())
            .await
            .is_err()
    );
    let server = Server::new(vec![Reply::utf8("")]);
    assert!(
        server
            .client()
            .fetch_quotes(
                &["600000.SH".into()],
                MarketSource::Tencent,
                CancellationToken::new()
            )
            .await
            .is_err()
    );
}
#[tokio::test]
async fn cancellation_reaches_every_network_api_and_precancel_makes_zero_requests() {
    for mode in 0..5 {
        let mut response = Reply::utf8("");
        response.delay = Duration::from_millis(150);
        let server = Server::new(vec![response]);
        let client = server.client();
        let token = CancellationToken::new();
        let cancel = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            cancel.cancel();
        });
        let result = match mode {
            0 => client
                .fetch_quotes(&["600000.SH".into()], MarketSource::Tencent, token.clone())
                .await
                .map(|_| ()),
            1 => client
                .fetch_details(&sample(), MarketSource::Tencent, false, token.clone())
                .await
                .map(|_| ()),
            2 => client
                .fetch_recent_trades(&sample(), MarketSource::Tencent, token.clone())
                .await
                .map(|_| ()),
            3 => client
                .fetch_recent_trades(&sample(), MarketSource::Eastmoney, token.clone())
                .await
                .map(|_| ()),
            _ => client.search("gzmt", token.clone()).await.map(|_| ()),
        };
        assert!(result.unwrap_err().to_string().contains("取消"));
    }
    let server = Server::new(vec![]);
    let token = CancellationToken::new();
    token.cancel();
    assert!(
        server
            .client()
            .fetch_quotes(&[], MarketSource::Tencent, token)
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
}
#[tokio::test]
async fn em_recent_timeout_preserves_four_second_limit() {
    let mut reply = Reply::utf8(r#"{"data":{"code":"600000","details":[]}}"#);
    reply.delay = Duration::from_millis(4700);
    let server = Server::new(vec![reply]);
    let client = server.client();
    let start = Instant::now();
    let err = client
        .fetch_recent_trades(&sample(), MarketSource::Eastmoney, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("4秒"));
    assert!(start.elapsed() < Duration::from_millis(4500));
}

#[tokio::test]
async fn recent_page_timeout_preserves_the_successful_tail_within_the_shared_deadline() {
    let mut tail = Reply::utf8(&page("sh600000", 2, "142/14:20:15/10.7/0/5/5350/B"));
    tail.delay = Duration::from_millis(1800);
    let mut previous = Reply::utf8(&page("sh600000", 1, "141/14:18:45/10.6/0/5/5300/S"));
    previous.delay = Duration::from_millis(2900);
    let server = Server::new(vec![
        Reply::utf8(&metadata("sh600000", "20261116", RANGES)),
        tail,
        previous,
    ]);
    let client = server.client();
    let start = Instant::now();
    let details = client
        .fetch_recent_trades(&sample(), MarketSource::Tencent, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(server.requests().len(), 3);
    assert_eq!(details.trades.len(), 1);
    assert_eq!(details.trades[0].price, d("10.7"));
    assert!(start.elapsed() < Duration::from_millis(4500));
}

#[tokio::test]
async fn recent_overall_four_second_timeout_bounds_both_requests() {
    let mut first = Reply::utf8(&metadata("sh600000", "20261116", RANGES));
    first.delay = Duration::from_millis(2300);
    let mut second = Reply::utf8(&page("sh600000", 2, "142/14:20:15/10.7/0/5/5350/B"));
    second.delay = Duration::from_millis(2300);
    let server = Server::new(vec![first, second]);
    let client = server.client();
    let start = Instant::now();
    assert!(
        client
            .fetch_recent_trades(&sample(), MarketSource::Tencent, CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("4秒")
    );
    assert!(start.elapsed() < Duration::from_millis(4500));
    assert_eq!(server.requests().len(), 2);
}
