use liteticker_native::{config::SettingsStore, model::*};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
};
fn d(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}
struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "liteticker-rust-config-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn symbols_preserve_markets_ascii_and_order() {
    for (raw, want) in [
        ("600519", "600519.SH"),
        ("000001", "000001.SZ"),
        ("000001.sh", "000001.SH"),
        ("159915", "159915.SZ"),
        ("430047", "430047.BJ"),
        ("920001", "920001.BJ"),
        ("800001", "800001.BJ"),
        ("900001", "900001.SH"),
    ] {
        assert_eq!(normalize_symbol(raw).unwrap(), want);
    }
    for raw in [
        "",
        "٦٠٠٥١٩",
        "600519.NY",
        "abc",
        "600519.SH.more",
        "600519.沪",
    ] {
        assert!(normalize_symbol(raw).is_err(), "{raw}");
    }
    let mut symbols = parse_symbols("600519 600519.SH,000001.SZ；920001\n000001.SH").unwrap();
    assert_eq!(
        symbols,
        ["600519.SH", "000001.SZ", "920001.BJ", "000001.SH"]
    );
    assert!(move_symbol(&mut symbols, "600519.SH", 1));
    assert_eq!(symbols[1], "600519.SH");
    assert!(!move_symbol(&mut symbols, "missing", 1));
    assert!(!move_symbol(&mut symbols, "600519.SH", 2));
    assert!(parse_symbols("600519 bad").is_err());
}
#[test]
fn watchlist_ends_preserve_other_stocks_and_saved_order() {
    let dir = Temporary::new();
    let store = SettingsStore::new(&dir.0);
    let mut settings = AppSettings {
        symbols: parse_symbols("600519 000001.SZ 920001 000001.SH").unwrap(),
        ..Default::default()
    };
    for (end, expected) in [
        (
            WatchlistEnd::Top,
            ["920001.BJ", "600519.SH", "000001.SZ", "000001.SH"],
        ),
        (
            WatchlistEnd::Bottom,
            ["600519.SH", "000001.SZ", "000001.SH", "920001.BJ"],
        ),
    ] {
        assert!(move_symbol_to_end(&mut settings.symbols, "920001.BJ", end));
        assert_eq!(settings.symbols, expected);
        store.save(&settings).unwrap();
        settings = SettingsStore::new(&dir.0).load().unwrap();
        assert_eq!(settings.symbols, expected);
    }
}
#[test]
fn watchlist_ends_ignore_missing_stocks_and_existing_boundaries() {
    for (raw, symbol, end) in [
        ("", "600519.SH", WatchlistEnd::Top),
        ("600519", "600519.SH", WatchlistEnd::Top),
        ("600519", "600519.SH", WatchlistEnd::Bottom),
        ("600519 000001.SZ", "920001.BJ", WatchlistEnd::Bottom),
        ("600519 000001.SZ", "600519.SH", WatchlistEnd::Top),
        ("600519 000001.SZ", "000001.SZ", WatchlistEnd::Bottom),
    ] {
        let original = parse_symbols(raw).unwrap();
        let mut symbols = original.clone();
        assert!(!move_symbol_to_end(&mut symbols, symbol, end));
        assert_eq!(symbols, original);
    }
}
#[test]
fn display_precision_units_and_overflow_match_v105() {
    for (n, want) in [
        ("9.24", "9.24"),
        ("3.270", "3.27"),
        ("2.383", "2.383"),
        ("13", "13.00"),
        ("2.3835", "2.384"),
    ] {
        assert_eq!(format_price(d(n)), want);
    }
    for (n, want) in [
        ("2308000", "230.8万"),
        ("270000000", "2.7亿"),
        ("9999", "9999"),
    ] {
        assert_eq!(format_volume_lots(d(n)), want);
    }
    assert_eq!(format_depth_amount(d("12.33"), d("100")), "12.33万");
    assert_eq!(format_depth_amount(d("2.383"), Decimal::ONE), "238.30");
    assert_eq!(format_depth_amount(Decimal::MAX, d("100")), "—");
    assert_eq!(format_depth_amount(Decimal::ZERO, Decimal::ONE), "—");
}
#[test]
fn trading_boundaries_and_closed_market_interval() {
    let settings = AppSettings {
        refresh_seconds: 2,
        ..Default::default()
    };
    for time in ["09:15", "11:35", "12:55", "15:05"] {
        assert!(is_trading_time(
            chrono::NaiveDateTime::parse_from_str(
                &format!("2026-11-16 {time}:00"),
                "%Y-%m-%d %H:%M:%S"
            )
            .unwrap()
        ));
    }
    for time in ["09:14", "11:36", "12:54", "15:06"] {
        let now = chrono::NaiveDateTime::parse_from_str(
            &format!("2026-11-16 {time}:00"),
            "%Y-%m-%d %H:%M:%S",
        )
        .unwrap();
        assert!(!is_trading_time(now));
        assert_eq!(interval_seconds(&settings, now), 30);
    }
    assert!(!is_trading_time(
        chrono::NaiveDateTime::parse_from_str("2026-11-14 10:00:00", "%Y-%m-%d %H:%M:%S").unwrap()
    ));
}
#[test]
fn pascal_case_numeric_enums_hotkeys_and_decimal_round_trip() {
    let settings = AppSettings::default();
    let value = serde_json::to_value(&settings).unwrap();
    assert_eq!(value["Source"], 0);
    assert_eq!(value["ToggleShortcut"], 0x60051);
    assert_eq!(value["TradeRefreshSeconds"], 3);
    assert_eq!(value["GlobalFontPercent"], 100);
    assert_eq!(value["QuoteFontPercent"], 0);
    assert_eq!(
        serde_json::from_value::<AppSettings>(value).unwrap(),
        settings
    );
    let quote:Quote=serde_json::from_str(r#"{"Symbol":"000001.SH","Name":"上证指数","Price":12.12345678901234567890123456,"Bids":[]}"#).unwrap();
    assert_eq!(quote.price, d("12.12345678901234567890123456"));
    assert_eq!(quote.symbol, "000001.SH");
    let encoded = serde_json::to_string(&quote).unwrap();
    assert!(encoded.contains("\"Price\":12.12345678901234567890123456"));
    assert_eq!(serde_json::from_str::<Quote>(&encoded).unwrap(), quote);
    assert!(serde_json::from_str::<MarketSource>("2").is_err());
}
#[test]
fn settings_are_atomic_backed_up_and_do_not_import_legacy_files() {
    let dir = Temporary::new();
    let store = SettingsStore::new(&dir.0);
    fs::write(dir.0.join("watchlist_local.json"), "{broken").unwrap();
    fs::write(dir.0.join("ticker_config.json"), "{broken").unwrap();
    assert_eq!(store.load().unwrap(), AppSettings::default());
    assert!(!store.file_path().exists());
    let mut settings = AppSettings {
        symbols: vec!["600519.SH".into()],
        auto_check_updates: false,
        ..Default::default()
    };
    store.save(&settings).unwrap();
    let first = fs::read(store.file_path()).unwrap();
    settings.symbols.push("000001.SH".into());
    settings.toggle_shortcut = 0;
    settings.trade_refresh_seconds = 2;
    settings.global_font_percent = 120;
    store.save(&settings).unwrap();
    assert_eq!(store.load().unwrap(), settings);
    assert_eq!(fs::read(dir.0.join("settings.json.bak")).unwrap(), first);
    store.save(&settings).unwrap();
    assert_eq!(fs::read(dir.0.join("settings.json.bak")).unwrap(), first);
    let second = fs::read(store.file_path()).unwrap();
    settings.x += 1;
    store.save(&settings).unwrap();
    assert_eq!(store.load().unwrap(), settings);
    assert_eq!(fs::read(dir.0.join("settings.json.bak")).unwrap(), second);
    let mut copy = settings.clone();
    copy.symbols.clear();
    assert_eq!(settings.symbols.len(), 2);
    assert_eq!(
        fs::read_to_string(dir.0.join("watchlist_local.json")).unwrap(),
        "{broken"
    );
    assert!(
        !fs::read_dir(&dir.0).unwrap().any(|e| e
            .unwrap()
            .path()
            .extension()
            .is_some_and(|e| e == "tmp"))
    );
    fs::write(store.file_path(), "{broken").unwrap();
    assert!(store.load().is_err());
    assert_eq!(fs::read_to_string(store.file_path()).unwrap(), "{broken");
}

#[test]
fn oversized_save_preserves_last_readable_configuration() {
    let dir = Temporary::new();
    let store = SettingsStore::new(&dir.0);
    let saved = AppSettings::default();
    store.save(&saved).unwrap();
    let oversized = AppSettings {
        symbols: vec!["600519.SH".into(); 200_000],
        ..saved.clone()
    };
    assert!(store.save(&oversized).is_err());
    assert_eq!(store.load().unwrap(), saved);
    assert!(!dir.0.join("settings.json.bak").exists());
}

#[test]
fn invalid_new_settings_fall_back_independently_without_erasing_other_fields() {
    let dir = Temporary::new();
    let store = SettingsStore::new(&dir.0);
    let base = AppSettings {
        symbols: vec!["000001.SH".into(), "600519.SH".into()],
        x: -123,
        y: 456,
        refresh_seconds: 5,
        always_on_top: false,
        ..Default::default()
    };
    for invalid in [
        json!(null),
        json!("100"),
        json!(true),
        json!(100.5),
        json!(-1),
        json!(i64::MAX),
    ] {
        let mut value = serde_json::to_value(&base).unwrap();
        for key in [
            "GlobalFontPercent",
            "QuoteFontPercent",
            "DetailsFontPercent",
            "TradeRefreshSeconds",
            "AutoCheckUpdates",
        ] {
            value[key] = invalid.clone();
        }
        fs::write(store.file_path(), serde_json::to_vec(&value).unwrap()).unwrap();
        let got = store.load().unwrap();
        assert_eq!(got.symbols, base.symbols);
        assert_eq!(got.x, -123);
        assert_eq!(got.y, 456);
        assert_eq!(got.refresh_seconds, 5);
        assert!(!got.always_on_top);
        assert_eq!(got.global_font_percent, 100);
        assert_eq!(got.quote_font_percent, 0);
        assert_eq!(got.details_font_percent, 0);
        assert_eq!(got.trade_refresh_seconds, 3);
        assert_eq!(got.auto_check_updates, invalid.as_bool().unwrap_or(true));
    }
    let mut value: Value = serde_json::to_value(&base).unwrap();
    for key in [
        "GlobalFontPercent",
        "QuoteFontPercent",
        "DetailsFontPercent",
        "TradeRefreshSeconds",
        "AutoCheckUpdates",
        "ToggleShortcut",
        "RefreshShortcut",
    ] {
        value.as_object_mut().unwrap().remove(key);
    }
    value["Source"] = json!(99);
    value["RefreshSeconds"] = json!(4);
    value["OpacityPercent"] = json!(20);
    value["Symbols"] = json!(["600519", "600519.SH", "000001.sh"]);
    fs::write(store.file_path(), serde_json::to_vec(&value).unwrap()).unwrap();
    let got = store.load().unwrap();
    assert_eq!(got.source, MarketSource::Tencent);
    assert_eq!(got.refresh_seconds, 1);
    assert_eq!(got.opacity_percent, 60);
    assert_eq!(got.symbols, ["600519.SH", "000001.SH"]);
    assert_eq!(got.toggle_shortcut, 0x60051);
}
#[test]
fn shortcut_disabled_duplicate_and_invalid_values_match_existing_contract() {
    for key in [0, 0x20051, 0x40052, 112, 135] {
        assert!(is_valid_shortcut(key), "{key}");
    }
    for key in [
        65, 0x10051, 0x20009, 0x4001b, 0x2002e, 0x20008, 0x200a2, 0x80051, 0x20007, 0x200ff,
    ] {
        assert!(!is_valid_shortcut(key), "{key}");
    }
    let dir = Temporary::new();
    let store = SettingsStore::new(&dir.0);
    let mut settings = AppSettings {
        toggle_shortcut: 65,
        refresh_shortcut: 0,
        ..Default::default()
    };
    store.save(&settings).unwrap();
    assert_eq!(store.load().unwrap().toggle_shortcut, 0x60051);
    assert_eq!(store.load().unwrap().refresh_shortcut, 0);
    settings.toggle_shortcut = 0x20051;
    settings.refresh_shortcut = 0x20051;
    store.save(&settings).unwrap();
    assert_eq!(store.load().unwrap().refresh_shortcut, 0);
}

#[test]
fn stable_uses_existing_settings_and_imports_a_preview_only_on_first_run() {
    let tmp = Temporary::new();
    let installed = SettingsStore::new(tmp.0.join("stable"));
    let preview = SettingsStore::new(tmp.0.join("preview"));
    assert_eq!(
        installed.load_or_import_preview(&preview).unwrap().symbols,
        AppSettings::default().symbols
    );
    assert!(!preview.file_path().exists());
    let mut settings = AppSettings {
        x: -200,
        symbols: vec!["600519.SH".into()],
        ..Default::default()
    };
    preview.save(&settings).unwrap();
    let before = fs::read(preview.file_path()).unwrap();
    assert_eq!(installed.load_or_import_preview(&preview).unwrap().x, -200);
    settings.x = 123;
    installed.save(&settings).unwrap();
    assert_eq!(installed.load_or_import_preview(&preview).unwrap().x, 123);
    assert_eq!(fs::read(preview.file_path()).unwrap(), before);
}
