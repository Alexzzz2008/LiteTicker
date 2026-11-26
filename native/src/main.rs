#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Result;
use chrono::{FixedOffset, Utc};
#[cfg(windows)]
use liteticker_native::platform::{DesktopCommand, DesktopPlatform, InstanceGuard};
use liteticker_native::{
    VERSION, config::SettingsStore, market::MarketClient, model::*, session::*,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use slint::{Color, ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};
use std::{
    cell::{Cell, RefCell},
    env,
    path::PathBuf,
    rc::{Rc, Weak},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;
slint::include_modules!();
mod chart;
mod dialogs;
mod replay_check;
mod updates_ui;
use liteticker_native::update::{self, DownloadedUpdate, Release};

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Replay {
    quotes: Vec<Quote>,
    details: MarketDetails,
}

enum Completion {
    Quotes(QuoteRequest, Result<Vec<Quote>, FetchFailure>),
    Details(DetailsRequest, bool, Result<MarketDetails, String>),
    Search(u64, Result<Vec<SymbolMatch>, String>),
    Update(u64, Result<Option<Release>, String>),
    Progress(u64, u8),
    Download(u64, Result<DownloadedUpdate, String>),
}
thread_local! {static ACTIVE: RefCell<Weak<App>> = RefCell::new(Weak::new());}
fn deliver(message: Completion) {
    let _ = slint::invoke_from_event_loop(move || {
        ACTIVE.with(|slot| {
            if let Some(app) = slot.borrow().upgrade() {
                app.complete(message);
            }
        })
    });
}
struct App {
    window: MainWindow,
    rows: Rc<VecModel<QuoteRow>>,
    depth: RefCell<Option<DetailsWindow>>,
    settings_view: RefCell<Option<SettingsWindow>>,
    search_view: RefCell<Option<SearchWindow>>,
    search_generation: Cell<u64>,
    search_token: RefCell<Option<CancellationToken>>,
    search_rows: RefCell<Vec<(SymbolMatch, bool)>>,
    search_text: RefCell<String>,
    debounce: Timer,
    recorded: Cell<[u32; 2]>,
    update_gen: Cell<u64>,
    update_token: RefCell<Option<CancellationToken>>,
    available: RefCell<Option<Release>>,
    update_message: RefCell<String>,
    update_latest: RefCell<String>,
    settings: RefCell<AppSettings>,
    store: SettingsStore,
    quotes: RefCell<QuoteSession>,
    details: RefCell<DetailsSession>,
    trades: RefCell<DetailsSession>,
    selection: RefCell<Option<String>>,
    detail_symbol: RefCell<Option<String>>,
    daily: Cell<bool>,
    amount: Cell<bool>,
    pinned: Cell<bool>,
    in_depth: Cell<bool>,
    chart_data: RefCell<Option<Arc<MarketDetails>>>,
    hovered_symbol: RefCell<Option<String>>,
    hover: Timer,
    leave: Timer,
    start: Instant,
    quote_timer: Timer,
    detail_timer: Timer,
    pending: Cell<bool>,
    quote_started: Cell<Instant>,
    platform_message: RefCell<String>,
    visible: Cell<bool>,
    fitted_scale: Cell<f32>,
    replay: Option<Replay>,
    tick: Cell<u64>,
    runtime: Runtime,
    market: MarketClient,
    #[cfg(windows)]
    platform: RefCell<Option<DesktopPlatform>>,
}
fn ink(delta: Decimal, fresh: bool) -> Color {
    let rgb = if !fresh {
        (147, 146, 156)
    } else if delta > Decimal::ZERO {
        (236, 55, 66)
    } else if delta < Decimal::ZERO {
        (36, 165, 88)
    } else {
        (222, 220, 224)
    };
    Color::from_rgb_u8(rgb.0, rgb.1, rgb.2)
}
fn signed(n: Decimal) -> String {
    if n.is_zero() {
        return "0.00%".into();
    }
    format!(
        "{:+.2}%",
        n.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
    )
}
fn percent(local: i32, global: i32) -> f32 {
    (if local == 0 { global } else { local }) as f32 / 100.
}
fn main() {
    if let Some(code) = update::run_helper_if_requested() {
        std::process::exit(code);
    }
    if let Err(e) = launch() {
        eprintln!("{e:#}");
        let mut args = env::args_os().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--after-native-update" {
                std::process::exit(1);
            }
            if arg == "--verify-replay" {
                if let Some(out) = args.next().map(PathBuf::from) {
                    if std::fs::create_dir(&out).is_ok() {
                        let _ =
                            std::fs::write(out.join("result.txt"), format!("FAIL startup: {e:#}"));
                    }
                }
                std::process::exit(1);
            }
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::*;
            let message = format!("轻量看盘启动失败\n{e:#}\0")
                .encode_utf16()
                .collect::<Vec<_>>();
            unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    message.as_ptr(),
                    windows_sys::w!("轻量看盘"),
                    MB_ICONERROR,
                );
            }
        }
        std::process::exit(1);
    }
}
fn launch() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let mut file = None;
    let mut verify = None;
    while let Some(arg) = args.next() {
        if arg == "--replay" {
            file = Some(PathBuf::from(
                args.next().ok_or_else(|| anyhow::anyhow!("缺少回放文件"))?,
            ));
        } else if arg == "--verify-replay" {
            verify = Some(PathBuf::from(
                args.next()
                    .ok_or_else(|| anyhow::anyhow!("缺少验收输出目录"))?,
            ));
        } else if arg != "--after-native-update" {
            anyhow::bail!("未知参数 {:?}", arg);
        }
    }
    anyhow::ensure!(
        verify.is_none() || file.is_some(),
        "验收命令需要指定回放文件"
    );
    #[cfg(windows)]
    let _instance = if file.is_some() {
        None
    } else {
        let Some(instance) = InstanceGuard::acquire()? else {
            return Ok(());
        };
        Some(instance)
    };
    let replay: Option<Replay> = file
        .as_ref()
        .map(|p| -> Result<_> { Ok(serde_json::from_reader(std::fs::File::open(p)?)?) })
        .transpose()?;
    let dir = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    let store = SettingsStore::new(dir.join("LiteTicker"));
    let mut settings = if replay.is_some() {
        AppSettings::default()
    } else {
        store.load_or_import_preview(&SettingsStore::new(dir.join("LiteTicker-preview")))?
    };
    if let Some(r) = &replay {
        settings.symbols = r.quotes.iter().map(|q| q.symbol.clone()).collect();
        settings.always_on_top = false;
        settings.auto_check_updates = false;
    }
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(if cfg!(feature = "renderer-skia") {
            "skia".into()
        } else if cfg!(feature = "renderer-software") {
            "software".into()
        } else {
            "femtovg-wgpu".into()
        })
        .select()?;
    let window = MainWindow::new()?;
    let rows = Rc::new(VecModel::default());
    window.set_rows(ModelRc::from(rows.clone()));
    let app = Rc::new(App {
        window,
        rows,
        depth: Default::default(),
        settings_view: Default::default(),
        search_view: Default::default(),
        search_generation: Default::default(),
        search_token: Default::default(),
        search_rows: Default::default(),
        search_text: Default::default(),
        debounce: Default::default(),
        recorded: Default::default(),
        update_gen: Default::default(),
        update_token: Default::default(),
        available: Default::default(),
        update_message: Default::default(),
        update_latest: RefCell::new("尚未检查".into()),
        settings: RefCell::new(settings.clone()),
        quotes: RefCell::new(QuoteSession::new(settings)),
        store,
        details: Default::default(),
        trades: Default::default(),
        selection: Default::default(),
        detail_symbol: Default::default(),
        daily: Default::default(),
        amount: Default::default(),
        pinned: Default::default(),
        in_depth: Default::default(),
        chart_data: Default::default(),
        hovered_symbol: Default::default(),
        hover: Default::default(),
        leave: Default::default(),
        start: Instant::now(),
        quote_timer: Default::default(),
        detail_timer: Default::default(),
        pending: Default::default(),
        quote_started: Cell::new(Instant::now()),
        platform_message: Default::default(),
        visible: Cell::new(true),
        fitted_scale: Cell::new(0.),
        replay,
        tick: Default::default(),
        market: MarketClient::new()?,
        runtime: tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?,
        #[cfg(windows)]
        platform: Default::default(),
    });
    ACTIVE.with(|slot| *slot.borrow_mut() = Rc::downgrade(&app));
    app.wire();
    app.render();
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
        let dpi = unsafe { GetDpiForSystem() }.max(96) as f32 / 96.;
        let settings = app.settings.borrow();
        app.window
            .set_q_scale(percent(settings.quote_font_percent, settings.global_font_percent) / dpi);
        app.window
            .set_g_scale(percent(0, settings.global_font_percent) / dpi);
    }
    app.window.show()?;
    let next = app.clone();
    when_mapped(&app.window, move |_| {
        next.fit();
        next.restore_position();
        #[cfg(windows)]
        {
            let init = (|| -> Result<()> {
                let p = DesktopPlatform::new(
                    next.window.window(),
                    move |cmd| with_app(|a| a.desktop(cmd)),
                    next.replay.is_none(),
                )?;
                let settings = next.settings.borrow();
                if let Err(e) = p.apply_settings(&settings) {
                    let mut fallback = settings.clone();
                    fallback.toggle_shortcut = 0;
                    fallback.refresh_shortcut = 0;
                    p.apply_settings(&fallback)?;
                    next.platform_message.replace(e.to_string());
                }
                next.platform.replace(Some(p));
                Ok(())
            })();
            if let Err(error) = init {
                next.platform_message.replace(error.to_string());
                next.render();
            }
        }
        next.refresh();
        if next.replay.is_none() && next.settings.borrow().auto_check_updates {
            slint::Timer::single_shot(Duration::from_secs(3), || {
                with_app(|a| {
                    if a.settings.borrow().auto_check_updates && a.update_gen.get() == 0 {
                        a.check_update();
                    }
                })
            });
        }
    });
    let _verifier = verify
        .map(|out| replay_check::start(app.clone(), out))
        .transpose()?;
    slint::run_event_loop_until_quit()?;
    Ok(())
}
// winit creates the HWND on its event loop, after Slint's show() returns.
fn when_mapped<C: ComponentHandle + 'static>(component: &C, ready: impl FnOnce(C) + 'static) {
    poll_mapped(component.as_weak(), 100, ready);
}
fn poll_mapped<C: ComponentHandle + 'static>(
    weak: slint::Weak<C>,
    remaining: usize,
    ready: impl FnOnce(C) + 'static,
) {
    slint::Timer::single_shot(Duration::from_millis(25), move || {
        let Some(component) = weak.upgrade() else {
            return;
        };
        #[cfg(windows)]
        if hwnd(component.window()).is_none() {
            if remaining > 0 {
                poll_mapped(weak, remaining - 1, ready);
            } else {
                with_app(|a| a.window.set_footer("窗口未完成初始化，请重新打开。".into()));
            }
            return;
        }
        if component.window().is_visible() {
            ready(component);
        }
    });
}
fn with_app(f: impl FnOnce(&Rc<App>)) {
    ACTIVE.with(|slot| {
        if let Some(a) = slot.borrow().upgrade() {
            f(&a);
        }
    });
}
impl App {
    fn modal(&self) -> bool {
        self.settings_view.borrow().is_some() || self.search_view.borrow().is_some()
    }
    fn wire(self: &Rc<Self>) {
        self.window.on_select_row(|i| {
            with_app(|a| {
                let id = a.settings.borrow().symbols.get(i as usize).cloned();
                if let Some(id) = id {
                    a.selection.replace(Some(id.clone()));
                    a.window.set_selection(i);
                }
            })
        });
        self.window.on_open_row(|i| {
            with_app(|a| {
                let id = a.settings.borrow().symbols.get(i as usize).cloned();
                if let Some(id) = id {
                    a.show_depth(&id, true);
                }
            })
        });
        self.window
            .on_hover_row(|i, inside| with_app(|a| a.hover_row(i, inside)));
        self.window
            .on_cancel_hover(|| with_app(|a| a.cancel_hover()));
        self.window
            .on_key_action(|key| with_app(|a| a.command(&key)));
        #[cfg(windows)]
        self.window.on_context(|row| {
            with_app(|a| {
                if let Some(p) = a.platform.borrow().as_ref() {
                    p.show_quote_menu(row, a.rows.row_count());
                }
            })
        });
        self.window.on_drag(|| {
            with_app(|a| {
                a.cancel_hover();
                #[cfg(windows)]
                if let Some(p) = a.platform.borrow().as_ref() {
                    p.start_drag();
                }
            })
        });
        self.window.window().on_close_requested(|| {
            with_app(|a| {
                #[cfg(windows)]
                if a.platform.borrow().is_some() {
                    a.hide();
                    return;
                }
                a.command("exit");
            });
            slint::CloseRequestResponse::KeepWindowShown
        });
    }
    fn restore_position(&self) {
        let settings = self.settings.borrow();
        let (mut x, mut y) = (settings.x, settings.y);
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::{Foundation::POINT, Graphics::Gdi::*};
            let mut monitor: MONITORINFO = std::mem::zeroed();
            monitor.cbSize = std::mem::size_of_val(&monitor) as u32;
            if GetMonitorInfoW(
                MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            ) != 0
            {
                let area = monitor.rcWork;
                let size = self.window.window().size();
                x = x.clamp(area.left, (area.right - size.width as i32).max(area.left));
                y = y.clamp(area.top, (area.bottom - size.height as i32).max(area.top));
            }
        }
        self.window
            .window()
            .set_position(slint::PhysicalPosition::new(x, y));
    }
    fn fit(&self) {
        let s = self.settings.borrow();
        let q = percent(s.quote_font_percent, s.global_font_percent);
        let g = percent(0, s.global_font_percent);
        let dpi = self.window.window().scale_factor().max(1.);
        self.window.set_q_scale(q / dpi);
        self.window.set_g_scale(g / dpi);
        let previous = self.fitted_scale.replace(q);
        let old = self.window.window().size();
        let width = if old.width == 0 || previous == 0. {
            550.0 * q
        } else {
            old.width as f32 * q / previous
        };
        self.window.window().set_size(slint::PhysicalSize::new(
            width.max(480. * q) as u32,
            (14. * q + 34. * g + 43. * q * s.symbols.len().clamp(1, 12) as f32) as u32,
        ));
    }
    fn render(&self) {
        let s = self.settings.borrow();
        let quotes = self.quotes.borrow();
        let snap = quotes.snapshot_ref();
        let scale = self.window.window().scale_factor().max(1.);
        self.window
            .set_q_scale(percent(s.quote_font_percent, s.global_font_percent) / scale);
        self.window
            .set_g_scale(percent(0, s.global_font_percent) / scale);
        let rows = s.symbols.iter().enumerate().map(|(i, id)| {
            let row = if let Some(q) = snap.quotes.get(id) {
                QuoteRow {
                    name: q.name.as_str().into(),
                    price: format_price(q.price).into(),
                    change: signed(q.change_percent).into(),
                    volume: format_volume_lots(q.volume_lots).into(),
                    ink: ink(q.change_percent, snap.fresh_symbols.contains(id)),
                }
            } else {
                QuoteRow {
                    name: id.as_str().into(),
                    price: "—".into(),
                    change: "—".into(),
                    volume: "—".into(),
                    ink: ink(Decimal::ZERO, false),
                }
            };
            (i, row)
        });
        for (i, row) in rows {
            if self.rows.row_count() <= i {
                self.rows.push(row);
            } else if self.rows.row_data(i).as_ref() != Some(&row) {
                self.rows.set_row_data(i, row);
            }
        }
        while self.rows.row_count() > s.symbols.len() {
            self.rows.remove(self.rows.row_count() - 1);
        }
        self.window.set_selection(
            self.selection
                .borrow()
                .as_ref()
                .and_then(|id| s.symbols.iter().position(|v| v == id))
                .map_or(-1, |i| i as i32),
        );
        let now = beijing_now();
        let mut line = if s.symbols.is_empty() {
            "右键添加自选 · 托盘可打开设置".into()
        } else {
            format!(
                "{} 只 · {} · {}s",
                s.symbols.len(),
                if is_trading_time(now) {
                    "交易中"
                } else {
                    "休市"
                },
                interval_seconds(&s, now)
            )
        };
        if let Some(ts) = snap.last_success {
            line.push_str(&format!(" · 行情 {}", ts.format("%H:%M:%S")));
        }
        if snap.error.is_some() {
            line.push_str(" · 更新失败");
        }
        let warning = self.platform_message.borrow();
        if !warning.is_empty() {
            line.push_str(" · ");
            line.push_str(&warning);
        }
        self.window.set_footer(line.into());
    }
    fn refresh(self: &Rc<Self>) {
        if !self.visible.get() {
            return;
        }
        self.quote_timer.stop();
        let request = self.quotes.borrow_mut().begin_refresh();
        let Some(r) = request else {
            if self.quotes.borrow().snapshot_ref().busy {
                self.pending.set(true);
            }
            return;
        };
        self.quote_started.set(Instant::now());
        if let Some(replay) = &self.replay {
            let mut q = replay.quotes.clone();
            if let Some(first) = q.first_mut() {
                first.price += Decimal::new((self.tick.get() % 2) as i64, 2);
            }
            self.tick.set(self.tick.get().wrapping_add(1));
            self.complete(Completion::Quotes(r, Ok(q)));
        } else {
            let client = self.market.clone();
            self.runtime.spawn(async move {
                let result = client
                    .fetch_quotes(&r.symbols, r.source, r.cancellation.clone())
                    .await
                    .map_err(|e| {
                        if e.downcast_ref::<reqwest::Error>()
                            .is_some_and(|e| e.is_timeout())
                        {
                            FetchFailure::Timeout
                        } else {
                            FetchFailure::Failed(e.to_string())
                        }
                    });
                deliver(Completion::Quotes(r, result));
            });
        }
    }
    fn complete(self: &Rc<Self>, msg: Completion) {
        match msg {
            Completion::Quotes(r, v) => {
                let now = Utc::now().with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap());
                let changed = self.quotes.borrow_mut().complete(&r, v, now);
                if !self.visible.get() {
                    return;
                }
                if changed {
                    self.render();
                    if self.replay.is_some() && self.tick.get() == 2 {
                        eprintln!("REPLAY_READY quotes={} ticks=2", self.rows.row_count());
                    }
                    self.render_depth();
                    if !self.hover.running()
                        && *self.hovered_symbol.borrow() != *self.detail_symbol.borrow()
                    {
                        self.show_hovered();
                    }
                }
                let secs = if self.pending.replace(false) {
                    0
                } else if self.replay.is_some() {
                    1
                } else {
                    interval_seconds(&self.settings.borrow(), beijing_now()) as u64
                };
                let wait = if secs == 0 {
                    Duration::ZERO
                } else {
                    Duration::from_secs(secs)
                        .saturating_sub(self.quote_started.get().elapsed())
                        .max(Duration::from_millis(100))
                };
                self.quote_timer
                    .start(TimerMode::SingleShot, wait, || with_app(|a| a.refresh()));
            }
            Completion::Details(r, trades, v) => {
                let changed = (if trades { &self.trades } else { &self.details })
                    .borrow_mut()
                    .complete(&r, v, self.start.elapsed());
                if changed {
                    self.render_depth();
                } else if self.detail_symbol.borrow().is_some() {
                    self.refresh_details(false);
                }
            }
            Completion::Search(g, v) => self.finish_search(g, v),
            Completion::Update(g, v) => self.checked_update(g, v),
            Completion::Progress(g, pct) if g == self.update_gen.get() => {
                self.set_update_message(format!("正在下载更新… {pct}%"))
            }
            Completion::Download(g, result) => self.downloaded_update(g, result),
            Completion::Progress(..) => {}
        }
    }
    fn hide(&self) {
        if self.modal() {
            return;
        }
        if let Err(error) = self.window.hide() {
            self.window.set_footer(error.to_string().into());
            return;
        }
        self.visible.set(false);
        self.quote_timer.stop();
        self.pending.set(false);
        self.quotes
            .borrow_mut()
            .update_settings(self.settings.borrow().clone());
        self.close_depth();
    }
    fn show(self: &Rc<Self>) {
        if let Err(error) = self.window.show() {
            self.window.set_footer(error.to_string().into());
            return;
        }
        let was_visible = self.visible.replace(true);
        #[cfg(windows)]
        if let Some(p) = self.platform.borrow().as_ref() {
            p.show_and_activate();
        }
        if !was_visible {
            self.refresh();
        }
    }
    fn command(self: &Rc<Self>, cmd: &str) {
        if self.modal() {
            return;
        }
        match cmd {
            "refresh" => {
                self.refresh();
                self.refresh_details(true);
            }
            "toggle" => {
                if self.visible.get() {
                    self.hide()
                } else {
                    self.show()
                }
            }
            "settings" => self.open_settings(),
            "add" => self.open_search(),
            "depth" => {
                let id = self.selection.borrow().clone();
                if let Some(id) = id {
                    self.show_depth(&id, true);
                }
            }
            "up" | "down" | "top" | "bottom" | "delete" => {
                let Some(id) = self.selection.borrow().clone() else {
                    return;
                };
                let mut s = self.settings.borrow().clone();
                let changed = match cmd {
                    "delete" => {
                        let before = s.symbols.len();
                        s.symbols.retain(|symbol| symbol != &id);
                        s.symbols.len() != before
                    }
                    "top" => move_symbol_to_end(&mut s.symbols, &id, WatchlistEnd::Top),
                    "bottom" => move_symbol_to_end(&mut s.symbols, &id, WatchlistEnd::Bottom),
                    "up" => move_symbol(&mut s.symbols, &id, -1),
                    "down" => move_symbol(&mut s.symbols, &id, 1),
                    _ => unreachable!(),
                };
                if !changed {
                    return;
                }
                if let Err(e) = self.save(s) {
                    self.window.set_footer(e.to_string().into());
                }
            }
            "exit" => {
                if self.replay.is_none() {
                    let mut s = self.settings.borrow().clone();
                    let p = self.window.window().position();
                    s.x = p.x;
                    s.y = p.y;
                    if let Err(e) = self.store.save(&s) {
                        eprintln!("未能保存退出时的窗口位置：{e:#}");
                    }
                }
                self.quote_timer.stop();
                self.close_depth();
                self.quotes.borrow_mut().dispose();
                let _ = slint::quit_event_loop();
            }
            _ => {}
        }
    }
    #[cfg(windows)]
    fn desktop(self: &Rc<Self>, cmd: DesktopCommand) {
        use DesktopCommand::*;
        self.command(match cmd {
            ToggleVisibility => "toggle",
            ShowSettings => "settings",
            PositionDetails => {
                if self.detail_symbol.borrow().is_some() {
                    if let Some(depth) = self.depth.borrow().as_ref() {
                        adjust_depth_window(
                            self.window.window(),
                            depth.window(),
                            &self.settings.borrow(),
                            true,
                        );
                    }
                }
                return;
            }
            ShowUpdates => {
                if !self.modal() {
                    self.open_settings();
                }
                if let Some(w) = self.settings_view.borrow().as_ref() {
                    w.set_tab(2);
                }
                return;
            }
            AddSymbols => "add",
            Refresh => "refresh",
            ShowDetails => "depth",
            RemoveSelected => "delete",
            MoveUp => "up",
            MoveDown => "down",
            MoveToTop => "top",
            MoveToBottom => "bottom",
            Exit => "exit",
        })
    }
    fn save(self: &Rc<Self>, mut s: AppSettings) -> Result<()> {
        let pos = self.window.window().position();
        s.x = pos.x;
        s.y = pos.y;
        #[cfg(windows)]
        if let Some(p) = self.platform.borrow().as_ref() {
            p.apply_settings(&s)?;
        }
        if self.replay.is_none() {
            if let Err(e) = self.store.save(&s) {
                #[cfg(windows)]
                if let Some(p) = self.platform.borrow().as_ref() {
                    p.apply_settings(&self.settings.borrow())?;
                }
                return Err(e);
            }
        }
        if self
            .detail_symbol
            .borrow()
            .as_ref()
            .is_some_and(|id| !s.symbols.contains(id))
            || self.settings.borrow().source != s.source
        {
            self.close_depth();
        }
        self.platform_message.borrow_mut().clear();
        self.settings.replace(s.clone());
        self.quotes.borrow_mut().update_settings(s);
        self.render();
        self.fit();
        self.refresh();
        Ok(())
    }
}
impl App {
    fn cancel_hover(&self) {
        self.hovered_symbol.take();
        self.hover.stop();
        self.leave.stop();
        if !self.pinned.get() {
            self.close_depth();
        }
    }
    fn pointer_pressed(&self) -> bool {
        if self.window.get_pointer_down() {
            return true;
        }
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
            return GetAsyncKeyState(i32::from(VK_LBUTTON)) < 0;
        }
        #[cfg(not(windows))]
        false
    }
    fn hover_row(self: &Rc<Self>, row: i32, inside: bool) {
        if self.modal() || self.pinned.get() || !self.visible.get() {
            return;
        }
        let Some(symbol) = self.settings.borrow().symbols.get(row as usize).cloned() else {
            return;
        };
        let same = self.hovered_symbol.borrow().as_ref() == Some(&symbol);
        if !inside {
            // A previous row's delayed leave can arrive after the next row's move.
            if same {
                self.hovered_symbol.take();
                self.hover.stop();
                self.schedule_leave();
            }
            return;
        }
        if self.pointer_pressed() {
            self.cancel_hover();
            return;
        }
        self.leave.stop();
        if same {
            return;
        }
        self.hovered_symbol.replace(Some(symbol));
        self.hover
            .start(TimerMode::SingleShot, Duration::from_millis(180), || {
                with_app(|a| a.show_hovered())
            });
    }
    fn show_hovered(self: &Rc<Self>) {
        if self.pinned.get() {
            return;
        }
        let Some(symbol) = self.hovered_symbol.borrow().clone() else {
            return;
        };
        if self.pointer_pressed() {
            self.cancel_hover();
            return;
        }
        self.show_depth(&symbol, false);
    }
    fn schedule_leave(&self) {
        self.leave
            .start(TimerMode::SingleShot, Duration::from_millis(280), || {
                with_app(|a| {
                    if a.hovered_symbol.borrow().is_none() && !a.in_depth.get() && !a.pinned.get() {
                        a.close_depth();
                    }
                })
            });
    }
    fn close_depth(&self) {
        self.hovered_symbol.take();
        self.hover.stop();
        self.leave.stop();
        self.detail_timer.stop();
        self.details.borrow_mut().deactivate();
        self.trades.borrow_mut().deactivate();
        self.detail_symbol.take();
        self.chart_data.take();
        self.pinned.set(false);
        self.in_depth.set(false);
        if let Some(d) = self.depth.borrow().as_ref() {
            let _ = d.hide();
        }
    }
    fn show_depth(self: &Rc<Self>, id: &str, pin: bool) {
        if self.modal()
            || !self.visible.get()
            || !self.quotes.borrow().snapshot_ref().quotes.contains_key(id)
        {
            return;
        }
        let new_window = self.depth.borrow().is_none();
        if new_window {
            let Ok(d) = DetailsWindow::new() else { return };
            d.on_command(|c| {
                with_app(|a| match c.as_str() {
                    "close" => a.close_depth(),
                    "pin" => {
                        a.pinned.set(!a.pinned.get());
                        a.render_depth();
                    }
                    "unit" => {
                        a.amount.set(!a.amount.get());
                        a.render_depth();
                    }
                    "daily" | "minute" => {
                        a.daily.set(c == "daily");
                        a.chart_data.take();
                        a.refresh_details(false);
                        a.render_depth();
                    }
                    _ => {}
                })
            });
            d.on_hover(|inside| {
                with_app(|a| {
                    a.in_depth.set(inside);
                    if inside {
                        a.leave.stop();
                    } else {
                        a.schedule_leave();
                    }
                })
            });
            d.window().on_close_requested(|| {
                with_app(|a| a.close_depth());
                slint::CloseRequestResponse::KeepWindowShown
            });
            self.depth.replace(Some(d));
        }
        let first = self.detail_symbol.borrow().is_none();
        if self.detail_symbol.borrow().as_deref() != Some(id) {
            self.chart_data.take();
        }
        self.detail_symbol.replace(Some(id.to_owned()));
        if pin {
            self.pinned.set(true);
        }
        if first {
            let borrow = self.depth.borrow();
            let d = borrow.as_ref().unwrap();
            let scale = {
                let s = self.settings.borrow();
                percent(s.details_font_percent, s.global_font_percent)
            };
            d.set_scale(scale / self.window.window().scale_factor().max(1.));
            let size = slint::PhysicalSize::new((960. * scale) as u32, (840. * scale) as u32);
            #[cfg(windows)]
            if let Some((position, fitted)) = depth_placement(self.window.window(), size) {
                d.window().set_position(position);
                d.window().set_size(fitted);
            } else {
                d.window().set_size(size);
            }
            #[cfg(not(windows))]
            d.window().set_size(size);
            let _ = d.show();
            when_mapped(d, move |d| {
                with_app(|app| {
                    d.set_scale(scale / d.window().scale_factor().max(1.));
                    d.window().set_size(size);
                    #[cfg(windows)]
                    adjust_depth_window(
                        app.window.window(),
                        d.window(),
                        &app.settings.borrow(),
                        true,
                    );
                })
            });
        }
        self.refresh_details(false);
        self.render_depth();
        self.arm_details_timer();
    }
    fn trade_refresh_interval(&self) -> Duration {
        let seconds = if self.replay.is_some() || is_trading_time(beijing_now()) {
            self.settings.borrow().trade_refresh_seconds
        } else {
            30
        };
        Duration::from_secs(seconds as u64)
    }
    fn arm_details_timer(&self) {
        self.detail_timer
            .start(TimerMode::SingleShot, self.trade_refresh_interval(), || {
                with_app(|a| {
                    if a.detail_symbol.borrow().is_some() {
                        a.refresh_details(false);
                        a.arm_details_timer();
                    }
                })
            });
    }
    fn refresh_details(self: &Rc<Self>, force: bool) {
        let Some(id) = self.detail_symbol.borrow().clone() else {
            return;
        };
        let Some(q) = self.quotes.borrow().snapshot_ref().quotes.get(&id).cloned() else {
            return;
        };
        let source = self.settings.borrow().source;
        let trade_interval = self.trade_refresh_interval();
        for trades in [false, true] {
            let lane = if trades { &self.trades } else { &self.details };
            let daily = !trades && self.daily.get();
            lane.borrow_mut().select(Some(q.clone()), source, daily);
            let r = lane.borrow_mut().begin_refresh(
                force,
                self.start.elapsed(),
                trades.then_some(trade_interval),
            );
            if let Some(r) = r {
                if let Some(replay) = &self.replay {
                    let v = if replay.details.symbol == id {
                        replay.details.clone()
                    } else {
                        MarketDetails {
                            symbol: id.clone(),
                            error: Some("此回放无该标的详情".into()),
                            ..Default::default()
                        }
                    };
                    self.complete(Completion::Details(r, trades, Ok(v)));
                } else {
                    let client = self.market.clone();
                    self.runtime.spawn(async move {
                        let data = if trades {
                            client
                                .fetch_recent_trades(&r.quote, r.key.source, r.cancellation.clone())
                                .await
                        } else {
                            client
                                .fetch_details(
                                    &r.quote,
                                    r.key.source,
                                    r.key.daily,
                                    r.cancellation.clone(),
                                )
                                .await
                        };
                        deliver(Completion::Details(
                            r,
                            trades,
                            data.map_err(|e| e.to_string()),
                        ));
                    });
                }
            }
        }
    }
    fn render_depth(&self) {
        const TRADE_ROW_COUNT: usize = 10;
        let Some(id) = self.detail_symbol.borrow().clone() else {
            return;
        };
        let quotes = self.quotes.borrow();
        let snap = quotes.snapshot_ref();
        let fresh = snap.fresh_symbols.contains(&id);
        let Some(q) = snap.quotes.get(&id) else {
            return;
        };
        let depth = self.depth.borrow();
        let Some(d) = depth.as_ref() else { return };
        d.set_heading(format!("{}  {}", q.name, id).into());
        d.set_price(format!("{}　{}", format_price(q.price), signed(q.change_percent)).into());
        d.set_ink(ink(q.change_percent, fresh));
        d.set_pinned(self.pinned.get());
        d.set_amount(self.amount.get());
        d.set_daily(self.daily.get());
        d.set_shanghai_index(id == SHANGHAI_COMPOSITE);
        d.set_has_depth(
            q.asks
                .iter()
                .chain(q.bids.iter())
                .any(|level| level.price > Decimal::ZERO),
        );
        let mut levels = vec![];
        for (is_ask, index) in (0..5)
            .rev()
            .map(|i| (true, i))
            .chain((0..5).map(|i| (false, i)))
        {
            let point = if is_ask {
                q.asks.get(index)
            } else {
                q.bids.get(index)
            }
            .filter(|p| p.price > Decimal::ZERO);
            levels.push(LadderRow {
                label: format!("{}{}", if is_ask { "卖" } else { "买" }, index + 1).into(),
                price: point
                    .map(|p| format_price(p.price))
                    .unwrap_or_else(|| "—".into())
                    .into(),
                size: point
                    .map(|p| {
                        if self.amount.get() {
                            format_depth_amount(p.price, p.volume_lots)
                        } else {
                            format_volume_lots(p.volume_lots)
                        }
                    })
                    .unwrap_or_else(|| "—".into())
                    .into(),
                ink: point
                    .map(|p| ink(p.price - q.previous_close, fresh))
                    .unwrap_or_else(|| ink(Decimal::ZERO, false)),
            });
        }
        if !d.get_ladder().iter().eq(levels.iter().cloned()) {
            d.set_ladder(ModelRc::new(VecModel::from(levels)));
        }
        let chart = self.details.borrow().snapshot(self.start.elapsed(), None);
        d.set_has_chart(chart.data.as_ref().is_some_and(|data| {
            if self.daily.get() {
                !data.daily.is_empty()
            } else {
                !data.minutes.is_empty()
            }
        }));
        if let Some(data) = &chart.data {
            if !self
                .chart_data
                .borrow()
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, data))
            {
                let (image, axis) =
                    chart::render(data, self.daily.get(), q.previous_close, q.price);
                d.set_has_leading(
                    !self.daily.get()
                        && data.symbol == SHANGHAI_COMPOSITE
                        && data.minutes.iter().any(|m| m.leading > Decimal::ZERO),
                );
                let overlay = data
                    .minutes
                    .last()
                    .map(|point| point.overlay(&data.symbol))
                    .filter(|price| !self.daily.get() && *price > Decimal::ZERO);
                d.set_overlay_value(overlay.map(format_price).unwrap_or_default().into());
                d.set_chart(image);
                d.set_axis(ModelRc::new(VecModel::from(axis)));
                d.set_first_date(
                    data.daily
                        .first()
                        .map(|b| b.date.as_str())
                        .unwrap_or("")
                        .into(),
                );
                d.set_last_date(
                    data.daily
                        .last()
                        .map(|b| b.date.as_str())
                        .unwrap_or("")
                        .into(),
                );
                self.chart_data.replace(Some(data.clone()));
            }
        } else {
            self.chart_data.take();
            d.set_has_leading(false);
            d.set_overlay_value(slint::SharedString::default());
            d.set_chart(slint::Image::default());
        }
        let trade_interval = self.trade_refresh_interval();
        let recent = self
            .trades
            .borrow()
            .snapshot(self.start.elapsed(), Some(trade_interval));
        let ticks = recent
            .data
            .as_ref()
            .map(|t| t.trades.as_slice())
            .unwrap_or(&[]);
        let s = self.settings.borrow();
        let mut rows = Vec::new();
        if !ticks.is_empty() {
            d.set_trades_title(format!("分笔成交 · {}s", trade_interval.as_secs()).into());
            let mut ticks = ticks.iter().collect::<Vec<_>>();
            ticks.sort_by(|a, b| b.time.cmp(&a.time));
            for t in ticks.into_iter().take(TRADE_ROW_COUNT) {
                rows.push(TradeRow {
                    time: t.time.as_str().into(),
                    price: format_price(t.price).into(),
                    size: format_volume_lots(t.volume_lots).into(),
                    ink: ink(
                        t.price - q.previous_close,
                        fresh
                            && !recent.stale
                            && !recent.data.as_ref().is_some_and(|v| v.trades_stale),
                    ),
                });
            }
        } else {
            d.set_trades_title("成交暂无数据".into());
            if let Some(data) = chart.data.as_ref().filter(|d| !d.minutes.is_empty()) {
                d.set_trades_title("分钟成交 · 非逐笔".into());
                let mut minutes = data.minutes.iter().collect::<Vec<_>>();
                minutes.sort_by(|a, b| b.time.cmp(&a.time));
                for m in minutes.into_iter().take(TRADE_ROW_COUNT) {
                    rows.push(TradeRow {
                        time: m.time.as_str().into(),
                        price: format_price(m.price).into(),
                        size: format_volume_lots(m.volume_lots).into(),
                        ink: ink(m.price - q.previous_close, fresh && !chart.stale),
                    });
                }
            }
        }
        while rows.len() < TRADE_ROW_COUNT {
            rows.push(TradeRow {
                time: "—".into(),
                price: "—".into(),
                size: "—".into(),
                ink: ink(Decimal::ZERO, false),
            });
        }
        if !d.get_trades().iter().eq(rows.iter().cloned()) {
            d.set_trades(ModelRc::new(VecModel::from(rows)));
        }
        let source = if s.source == MarketSource::Tencent {
            "腾讯行情"
        } else {
            "东方财富"
        };
        let error = chart
            .error
            .or(recent.error)
            .map_or_else(String::new, |e| format!(" · {e}"));
        let stale = if fresh { "" } else { " · 上次数据" };
        d.set_status(format!("{source} · {}{stale}{error}", q.timestamp).into());
    }
}
#[cfg(windows)]
fn hwnd(window: &slint::Window) -> Option<windows_sys::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window.window_handle();
    let raw = handle.window_handle().ok()?.as_raw();
    if let RawWindowHandle::Win32(handle) = raw {
        Some(handle.hwnd.get() as _)
    } else {
        None
    }
}
#[cfg(windows)]
fn depth_placement(
    main: &slint::Window,
    size: slint::PhysicalSize,
) -> Option<(slint::PhysicalPosition, slint::PhysicalSize)> {
    use windows_sys::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::GetWindowRect};
    let m = hwnd(main)?;
    unsafe {
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of_val(&info) as u32;
        if GetMonitorInfoW(MonitorFromWindow(m, MONITOR_DEFAULTTONEAREST), &mut info) == 0 {
            return None;
        }
        let area = info.rcWork;
        let mut owner = std::mem::zeroed();
        if GetWindowRect(m, &mut owner) == 0 {
            return None;
        }
        let w = (size.width as i32).min(area.right - area.left);
        let h = (size.height as i32).min(area.bottom - area.top);
        let x = if owner.right + 8 + w > area.right {
            owner.left - w - 8
        } else {
            owner.right + 8
        };
        Some((
            slint::PhysicalPosition::new(
                x.clamp(area.left, area.right - w),
                owner.top.clamp(area.top, area.bottom - h),
            ),
            slint::PhysicalSize::new(w as u32, h as u32),
        ))
    }
}
#[cfg(windows)]
fn adjust_depth_window(
    main: &slint::Window,
    depth: &slint::Window,
    s: &AppSettings,
    reposition: bool,
) {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let Some(d) = hwnd(depth) else {
        return;
    };
    let Some((position, size)) = depth_placement(main, depth.size()) else {
        return;
    };
    unsafe {
        let z = if s.always_on_top {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        SetWindowPos(
            d,
            z,
            position.x,
            position.y,
            size.width as i32,
            size.height as i32,
            SWP_NOACTIVATE
                | if reposition {
                    0
                } else {
                    SWP_NOMOVE | SWP_NOSIZE
                },
        );
        let style = GetWindowLongPtrW(d, GWL_EXSTYLE);
        SetWindowLongPtrW(d, GWL_EXSTYLE, style | WS_EX_LAYERED as isize);
        SetLayeredWindowAttributes(d, 0, (s.opacity_percent * 255 / 100) as u8, LWA_ALPHA);
    }
}
