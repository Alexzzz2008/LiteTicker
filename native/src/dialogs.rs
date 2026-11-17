use super::*;
use slint::platform::Key;
const INTERVALS: [i32; 5] = [1, 2, 3, 5, 10];
fn shortcut_label(n: u32) -> String {
    if n == 0 {
        return "已禁用".into();
    }
    let mut parts = Vec::new();
    for (bit, name) in [(0x20000, "Ctrl"), (0x40000, "Alt"), (0x10000, "Shift")] {
        if n & bit != 0 {
            parts.push(name.to_string());
        }
    }
    let k = n & 65535;
    parts.push(match k {
        112..=135 => format!("F{}", k - 111),
        48..=57 | 65..=90 => char::from_u32(k).unwrap().to_string(),
        _ => format!("键 {k}"),
    });
    parts.join(" + ")
}
fn shortcut_value(text: &str, ctrl: bool, alt: bool, shift: bool) -> Option<u32> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if c == char::from(Key::Delete) {
        return Some(0);
    }
    let vk = if c.is_ascii_alphanumeric() {
        c.to_ascii_uppercase() as u32
    } else if (char::from(Key::F1)..=char::from(Key::F24)).contains(&c) {
        112 + (c as u32 - char::from(Key::F1) as u32)
    } else {
        [
            (Key::UpArrow, 38),
            (Key::DownArrow, 40),
            (Key::LeftArrow, 37),
            (Key::RightArrow, 39),
            (Key::Home, 36),
            (Key::End, 35),
            (Key::PageUp, 33),
            (Key::PageDown, 34),
            (Key::Return, 13),
        ]
        .into_iter()
        .find(|(key, _)| char::from(*key) == c)?
        .1
    };
    let n = vk
        | if ctrl { 0x20000 } else { 0 }
        | if alt { 0x40000 } else { 0 }
        | if shift { 0x10000 } else { 0 };
    is_valid_shortcut(n).then_some(n)
}
fn font_index(value: i32, local: bool) -> i32 {
    if local && value == 0 {
        0
    } else {
        (value - 80) / 10 + i32::from(local)
    }
}
fn scale_dialog(window: &slint::Window, base: (u32, u32), global: i32) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::*};
        if let Some(h) = hwnd(window) {
            unsafe {
                let mut mon: MONITORINFO = std::mem::zeroed();
                mon.cbSize = std::mem::size_of_val(&mon) as u32;
                if GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mon) != 0 {
                    let area = mon.rcWork;
                    let w = (base.0 as i32 * global / 100)
                        .min(area.right - area.left - 16)
                        .max(1);
                    let hgt = (base.1 as i32 * global / 100)
                        .min(area.bottom - area.top - 16)
                        .max(1);
                    SetWindowPos(
                        h,
                        HWND_TOP,
                        (area.right + area.left - w) / 2,
                        (area.bottom + area.top - hgt) / 2,
                        w,
                        hgt,
                        SWP_SHOWWINDOW,
                    );
                }
            }
        }
    }
}
impl App {
    pub(super) fn open_settings(self: &Rc<Self>) {
        self.close_depth();
        let Ok(w) = SettingsWindow::new() else { return };
        let s = self.settings.borrow();
        w.set_version(VERSION.into());
        w.set_source(i32::from(u8::from(s.source)));
        w.set_interval(
            INTERVALS
                .iter()
                .position(|v| *v == s.refresh_seconds)
                .unwrap_or(0) as i32,
        );
        w.set_trade(s.trade_refresh_seconds - 2);
        w.set_topmost(s.always_on_top);
        w.set_opacity_pct(s.opacity_percent as f32);
        w.set_automatic(s.auto_check_updates);
        w.set_global_index(font_index(s.global_font_percent, false));
        w.set_quote_index(font_index(s.quote_font_percent, true));
        w.set_depth_index(font_index(s.details_font_percent, true));
        self.recorded.set([s.toggle_shortcut, s.refresh_shortcut]);
        w.set_toggle_label(shortcut_label(s.toggle_shortcut).into());
        w.set_refresh_label(shortcut_label(s.refresh_shortcut).into());
        drop(s);
        w.on_key(|idx, text, ctrl, alt, shift| {
            with_app(|a| {
                if let Some(w) = &*a.settings_view.borrow() {
                    if let Some(n) = shortcut_value(&text, ctrl, alt, shift) {
                        let mut keys = a.recorded.get();
                        keys[idx as usize] = n;
                        a.recorded.set(keys);
                        w.set_recording(-1);
                        w.set_toggle_label(shortcut_label(keys[0]).into());
                        w.set_refresh_label(shortcut_label(keys[1]).into());
                        w.set_message("".into());
                    } else {
                        w.set_message(
                            "请输入带 Ctrl 或 Alt 的快捷键，或功能键。Delete 可禁用。".into(),
                        );
                    }
                }
            })
        });
        w.on_action(|c| with_app(|a| a.settings_action(&c)));
        w.window().on_close_requested(|| {
            with_app(|a| a.close_settings());
            slint::CloseRequestResponse::HideWindow
        });
        if w.show().is_ok() {
            scale_dialog(
                w.window(),
                (836, 915),
                self.settings.borrow().global_font_percent,
            );
            self.settings_view.replace(Some(w));
        }
    }
    fn close_settings(&self) {
        self.update_gen.set(self.update_gen.get().wrapping_add(1));
        if let Some(t) = self.update_token.take() {
            t.cancel();
        }
        if let Some(w) = self.settings_view.take() {
            let _ = w.hide();
        }
    }
    fn settings_action(self: &Rc<Self>, cmd: &str) {
        let current = self.settings_view.borrow();
        let Some(w) = current.as_ref() else { return };
        match cmd {
            "drag" => drag_window(w.window()),
            "close" => {
                drop(current);
                self.close_settings();
            }
            "save" => {
                let mut s = self.settings.borrow().clone();
                s.source = if w.get_source() == 0 {
                    MarketSource::Tencent
                } else {
                    MarketSource::Eastmoney
                };
                s.refresh_seconds = INTERVALS[w.get_interval().clamp(0, 4) as usize];
                s.trade_refresh_seconds = 2 + w.get_trade().clamp(0, 1);
                s.always_on_top = w.get_topmost();
                s.opacity_percent = w.get_opacity_pct().round() as i32;
                s.auto_check_updates = w.get_automatic();
                // Keep valid v1 percentages that do not land on a ten-percent step unless the user changes the choice.
                for (value, index, local) in [
                    (&mut s.global_font_percent, w.get_global_index(), false),
                    (&mut s.quote_font_percent, w.get_quote_index(), true),
                    (&mut s.details_font_percent, w.get_depth_index(), true),
                ] {
                    if font_index(*value, local) != index {
                        *value = if local && index == 0 {
                            0
                        } else {
                            80 + (index - i32::from(local)) * 10
                        };
                    }
                }
                [s.toggle_shortcut, s.refresh_shortcut] = self.recorded.get();
                if s.toggle_shortcut != 0 && s.toggle_shortcut == s.refresh_shortcut {
                    w.set_message("两个快捷键不能相同。".into());
                    return;
                }
                match self.save(s) {
                    Ok(()) => {
                        drop(current);
                        self.close_settings();
                    }
                    Err(e) => w.set_message(e.to_string().into()),
                }
            }
            "check" => {
                if let Some(t) = self.update_token.take() {
                    t.cancel();
                }
                let g = self.update_gen.get().wrapping_add(1);
                self.update_gen.set(g);
                let ct = CancellationToken::new();
                self.update_token.replace(Some(ct.clone()));
                w.set_message("正在检查更新…".into());
                self.runtime.spawn(async move {
                    let result = check_preview(ct).await.map_err(|e| e.to_string());
                    deliver(Completion::Update(g, result));
                });
            }
            _ => {}
        }
    }
    pub(super) fn open_search(self: &Rc<Self>) {
        self.close_depth();
        let Ok(w) = SearchWindow::new() else { return };
        w.on_edited(|| {
            with_app(|a| {
                a.cancel_search();
                a.search_rows.borrow_mut().clear();
                a.draw_matches();
                a.debounce
                    .start(TimerMode::SingleShot, Duration::from_millis(250), || {
                        with_app(|a| a.start_search())
                    });
            })
        });
        w.on_toggle(|i| {
            with_app(|a| {
                if let Some(row) = a.search_rows.borrow_mut().get_mut(i as usize) {
                    row.1 = !row.1;
                }
                a.draw_matches();
            })
        });
        w.on_action(|c| {
            with_app(|a| match c.as_str() {
                "drag" => {
                    if let Some(w) = a.search_view.borrow().as_ref() {
                        drag_window(w.window());
                    }
                }
                "close" => a.close_search(),
                "search" => {
                    a.debounce.stop();
                    a.start_search();
                }
                "add" => {
                    let mut s = a.settings.borrow().clone();
                    for (row, chosen) in a.search_rows.borrow().iter() {
                        if *chosen && !s.symbols.contains(&row.symbol) {
                            s.symbols.push(row.symbol.clone());
                        }
                    }
                    match a.save(s) {
                        Ok(()) => a.close_search(),
                        Err(e) => {
                            if let Some(w) = &*a.search_view.borrow() {
                                w.set_message(e.to_string().into());
                            }
                        }
                    }
                }
                _ => {}
            })
        });
        w.window().on_close_requested(|| {
            with_app(|a| a.close_search());
            slint::CloseRequestResponse::HideWindow
        });
        if w.show().is_ok() {
            scale_dialog(
                w.window(),
                (720, 660),
                self.settings.borrow().global_font_percent,
            );
            self.search_view.replace(Some(w));
        }
    }
    fn cancel_search(&self) {
        self.debounce.stop();
        self.search_generation
            .set(self.search_generation.get().wrapping_add(1));
        if let Some(token) = self.search_token.take() {
            token.cancel();
        }
        self.search_text.take();
    }
    fn close_search(&self) {
        self.cancel_search();
        self.search_rows.borrow_mut().clear();
        if let Some(w) = self.search_view.take() {
            let _ = w.hide();
        }
    }
    fn start_search(self: &Rc<Self>) {
        let v = self.search_view.borrow();
        let Some(w) = v.as_ref() else { return };
        let query = w.get_query().trim().to_string();
        if query.is_empty() {
            w.set_message("请输入股票编号或首字母。".into());
            return;
        }
        if self.search_token.borrow().is_some() && *self.search_text.borrow() == query {
            return;
        }
        self.cancel_search();
        let g = self.search_generation.get();
        let token = CancellationToken::new();
        self.search_token.replace(Some(token.clone()));
        self.search_text.replace(query.clone());
        w.set_message("正在搜索…".into());
        if let Some(r) = &self.replay {
            let rows = r
                .quotes
                .iter()
                .filter(|q| q.symbol.starts_with(&query) || q.name.contains(&query))
                .map(|q| SymbolMatch {
                    symbol: q.symbol.clone(),
                    name: q.name.clone(),
                    initials: String::new(),
                })
                .collect();
            drop(v);
            self.finish_search(g, Ok(rows));
        } else {
            let market = self.market.clone();
            self.runtime.spawn(async move {
                let rows = market
                    .search(&query, token)
                    .await
                    .map_err(|e| e.to_string());
                deliver(Completion::Search(g, rows));
            });
        }
    }
    pub(super) fn finish_search(&self, g: u64, result: Result<Vec<SymbolMatch>, String>) {
        if g != self.search_generation.get() {
            return;
        }
        self.search_token.take();
        if let Some(w) = self.search_view.borrow().as_ref() {
            match result {
                Ok(rows) => {
                    w.set_message(
                        format!(
                            "找到 {} 个匹配，请核对名称、编号和市场后选择添加。",
                            rows.len()
                        )
                        .into(),
                    );
                    self.search_rows
                        .replace(rows.into_iter().map(|m| (m, false)).collect());
                    self.draw_matches();
                }
                Err(e) => w.set_message(e.into()),
            }
        }
    }
    fn draw_matches(&self) {
        if let Some(w) = self.search_view.borrow().as_ref() {
            let rows = self.search_rows.borrow();
            let display = rows
                .iter()
                .map(|(m, yes)| {
                    let (code, market) = m.symbol.split_once('.').unwrap_or((&m.symbol, ""));
                    SearchResult {
                        name: m.name.as_str().into(),
                        code: code.into(),
                        market: match market {
                            "SH" => "沪市",
                            "SZ" => "深市",
                            "BJ" => "北市",
                            _ => "—",
                        }
                        .into(),
                        chosen: *yes,
                    }
                })
                .collect::<Vec<_>>();
            w.set_matches(ModelRc::new(VecModel::from(display)));
            w.set_count(rows.iter().filter(|r| r.1).count() as i32);
        }
    }
}
async fn check_preview(token: CancellationToken) -> Result<String> {
    // The public stable feed currently serves the .NET product under a different asset identity.
    let client = reqwest::Client::builder()
        .user_agent("LiteTicker-preview")
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let metadata = async {
        let response = client
            .get("https://api.github.com/repos/Alexzzz2008/LiteTicker/releases/latest")
            .send()
            .await?
            .error_for_status()?;
        let json: serde_json::Value = response.json().await?;
        let tag = json["tag_name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("官方版本信息无效"))?;
        Ok(format!(
            "当前公开正式版为 {tag}。原生预览版使用独立更新渠道。"
        ))
    };
    tokio::select! {v=metadata=>v,_=token.cancelled()=>Err(anyhow::anyhow!("检查已取消"))}
}
