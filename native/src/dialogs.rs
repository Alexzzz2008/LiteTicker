use super::*;
use slint::platform::Key;
const INTERVALS: [i32; 5] = [1, 2, 3, 5, 10];

fn shortcut_label(n: u32) -> String {
    if n == 0 {
        return "点击录入快捷键".into();
    }
    let mut parts = Vec::new();
    for (bit, name) in [(0x20000, "Ctrl"), (0x40000, "Alt"), (0x10000, "Shift")] {
        if n & bit != 0 {
            parts.push(name.to_string());
        }
    }
    let key = n & 65535;
    parts.push(match key {
        112..=135 => format!("F{}", key - 111),
        48..=57 | 65..=90 => char::from_u32(key).unwrap().to_string(),
        13 => "Enter".into(),
        32 => "Space".into(),
        33 => "PageUp".into(),
        34 => "PageDown".into(),
        35 => "End".into(),
        36 => "Home".into(),
        37 => "Left".into(),
        38 => "Up".into(),
        39 => "Right".into(),
        40 => "Down".into(),
        45 => "Insert".into(),
        186 => ";".into(),
        187 => "=".into(),
        188 => ",".into(),
        189 => "-".into(),
        190 => ".".into(),
        191 => "/".into(),
        192 => "`".into(),
        219 => "[".into(),
        220 => "\\".into(),
        221 => "]".into(),
        222 => "'".into(),
        _ => format!("键 {key}"),
    });
    parts.join(" + ")
}
fn shortcut_value(text: &str, ctrl: bool, alt: bool, shift: bool) -> Option<u32> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if c == char::from(Key::Delete) || c == char::from(Key::Backspace) {
        return Some(0);
    }
    let key = if c.is_ascii_alphanumeric() {
        c.to_ascii_uppercase() as u32
    } else if (char::from(Key::F1)..=char::from(Key::F24)).contains(&c) {
        112 + (c as u32 - char::from(Key::F1) as u32)
    } else if let Some((_, key)) = [
        (Key::UpArrow, 38),
        (Key::DownArrow, 40),
        (Key::LeftArrow, 37),
        (Key::RightArrow, 39),
        (Key::Home, 36),
        (Key::End, 35),
        (Key::PageUp, 33),
        (Key::PageDown, 34),
        (Key::Return, 13),
        (Key::Insert, 45),
    ]
    .into_iter()
    .find(|(key, _)| char::from(*key) == c)
    {
        key
    } else {
        character_virtual_key(c)?
    };
    let value = key
        | if ctrl { 0x20000 } else { 0 }
        | if alt { 0x40000 } else { 0 }
        | if shift { 0x10000 } else { 0 };
    is_valid_shortcut(value).then_some(value)
}
fn character_virtual_key(c: char) -> Option<u32> {
    #[cfg(windows)]
    {
        if c as u32 > u16::MAX as u32 {
            return None;
        }
        let key = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::VkKeyScanW(c as u16) };
        (key != -1).then_some(key as u32 & 255)
    }
    #[cfg(not(windows))]
    {
        Some(match c {
            ' ' => 32,
            ';' | ':' => 186,
            '=' | '+' => 187,
            ',' | '<' => 188,
            '-' | '_' => 189,
            '.' | '>' => 190,
            '/' | '?' => 191,
            '`' | '~' => 192,
            '[' | '{' => 219,
            '\\' | '|' => 220,
            ']' | '}' => 221,
            '\'' | '"' => 222,
            '!' => 49,
            '@' => 50,
            '#' => 51,
            '$' => 52,
            '%' => 53,
            '^' => 54,
            '&' => 55,
            '*' => 56,
            '(' => 57,
            ')' => 48,
            _ => return None,
        })
    }
}
fn font_choices(value: i32, maximum: i32, local: bool) -> Vec<i32> {
    let mut values = if local { vec![0] } else { Vec::new() };
    values.extend((80..=maximum).step_by(10));
    if value >= 80 && value <= maximum && !values.contains(&value) {
        values.push(value);
    }
    values
}
fn font_index(value: i32, maximum: i32, local: bool) -> i32 {
    font_choices(value, maximum, local)
        .iter()
        .position(|choice| *choice == value)
        .unwrap_or(0) as i32
}
fn font_options(value: i32, maximum: i32, local: bool) -> ModelRc<slint::SharedString> {
    ModelRc::new(VecModel::from(
        font_choices(value, maximum, local)
            .into_iter()
            .map(|n| {
                if n == 0 {
                    "跟随全局".into()
                } else {
                    format!("{n}%").into()
                }
            })
            .collect::<Vec<_>>(),
    ))
}
fn selection_after_click(
    previous: &[i32],
    index: i32,
    anchor: i32,
    count: usize,
    ctrl: bool,
    shift: bool,
    keyboard: bool,
) -> Vec<i32> {
    if index < 0 || index as usize >= count {
        return previous.to_vec();
    }
    if keyboard && ctrl && !shift {
        return previous.to_vec();
    }
    if shift && anchor >= 0 && (anchor as usize) < count {
        let mut selected = if ctrl { previous.to_vec() } else { vec![] };
        selected.extend(anchor.min(index)..=anchor.max(index));
        selected.sort_unstable();
        selected.dedup();
        return selected;
    }
    if ctrl {
        let mut selected = previous.to_vec();
        if let Some(p) = selected.iter().position(|i| *i == index) {
            selected.remove(p);
        } else {
            selected.push(index);
        }
        selected.sort_unstable();
        selected
    } else {
        vec![index]
    }
}
fn selected_indices(checked: &[bool], selected: &[i32]) -> Vec<usize> {
    if checked.iter().any(|yes| *yes) {
        checked
            .iter()
            .enumerate()
            .filter_map(|(i, yes)| yes.then_some(i))
            .collect()
    } else {
        selected
            .iter()
            .filter_map(|i| usize::try_from(*i).ok().filter(|i| *i < checked.len()))
            .collect()
    }
}

fn limit_query(text: &str) -> String {
    let mut units = 0;
    text.chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 256
        })
        .collect()
}
fn search_failure_message(error: &str) -> String {
    if error.contains("超时") || error.contains("timed out") || error.contains("取消") {
        return "查询超时，请重试。".into();
    }
    if error.contains("最多") || error.starts_with("批量查询") {
        return error
            .lines()
            .next()
            .unwrap_or("查询输入无效，请检查后重试。")
            .chars()
            .take(120)
            .collect();
    }
    "查询失败，请检查网络后重试。".into()
}

fn prepare_dialog(
    window: &slint::Window,
    owner: &slint::Window,
    base: (u32, u32),
    desired: f32,
    apply_scale: impl FnOnce(f32),
) -> f32 {
    let mut physical_scale = desired;
    let mut center = None;
    #[cfg(windows)]
    if let Some(handle) = hwnd(owner) {
        use windows_sys::Win32::Graphics::Gdi::*;
        unsafe {
            let mut monitor: MONITORINFO = std::mem::zeroed();
            monitor.cbSize = std::mem::size_of_val(&monitor) as u32;
            if GetMonitorInfoW(
                MonitorFromWindow(handle, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            ) != 0
            {
                let area = monitor.rcWork;
                physical_scale = desired
                    .min((area.right - area.left - 32).max(1) as f32 / base.0 as f32)
                    .min((area.bottom - area.top - 32).max(1) as f32 / base.1 as f32);
                center = Some((
                    (area.left + area.right - (base.0 as f32 * physical_scale).round() as i32) / 2,
                    (area.top + area.bottom - (base.1 as f32 * physical_scale).round() as i32) / 2,
                ));
            }
        }
    }
    if let Some((x, y)) = center {
        window.set_position(slint::PhysicalPosition::new(x, y));
    }
    // Only run after the native mapping: the child's actual DPI is now available.
    apply_scale(physical_scale / window.scale_factor().max(1.));
    window.set_size(slint::PhysicalSize::new(
        (base.0 as f32 * physical_scale).round().max(1.) as u32,
        (base.1 as f32 * physical_scale).round().max(1.) as u32,
    ));
    physical_scale
}

#[cfg(windows)]
mod modality {
    use super::*;
    use anyhow::Context;
    use windows_sys::Win32::{
        Foundation::*,
        UI::{
            Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled},
            Shell::*,
            WindowsAndMessaging::*,
        },
    };
    const ID: usize = 0x4c54444c;
    struct State {
        owner: HWND,
        dialog: Cell<HWND>,
        was_enabled: bool,
        alive: Cell<bool>,
    }
    impl State {
        fn present(&self) -> Result<()> {
            let dialog = self.dialog.get();
            if dialog.is_null() {
                return Ok(());
            }
            unsafe {
                if IsIconic(dialog) != 0 {
                    ShowWindow(dialog, SW_RESTORE);
                }
                // Activating an ordinary tool window does not lift it above a topmost ticker.
                let topmost =
                    GetWindowLongPtrW(self.owner, GWL_EXSTYLE) & WS_EX_TOPMOST as isize != 0;
                let after = if topmost { HWND_TOPMOST } else { HWND_TOP };
                if SetWindowPos(
                    dialog,
                    after,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
                ) == 0
                {
                    return Err(std::io::Error::last_os_error())
                        .context("无法将弹窗置于行情列表前面");
                }
                SetForegroundWindow(dialog);
            }
            Ok(())
        }
    }
    pub(super) struct Guard(Box<State>);
    impl Guard {
        pub fn new(owner: &slint::Window) -> Result<Self> {
            let owner = hwnd(owner).ok_or_else(|| anyhow::anyhow!("主窗句柄尚未就绪"))?;
            let state = Box::new(State {
                owner,
                dialog: Cell::new(std::ptr::null_mut()),
                was_enabled: unsafe { IsWindowEnabled(owner) != 0 },
                alive: Cell::new(true),
            });
            unsafe {
                if SetWindowSubclass(owner, Some(block), ID, state.as_ref() as *const _ as usize)
                    == 0
                {
                    anyhow::bail!("无法启用弹窗模态保护");
                }
                EnableWindow(owner, 0);
            }
            Ok(Self(state))
        }
        pub fn attached(&self, window: &slint::Window) -> bool {
            self.0.alive.get()
                && !self.0.dialog.get().is_null()
                && hwnd(window) == Some(self.0.dialog.get())
        }
        pub fn attach(&self, window: &slint::Window) -> Result<()> {
            if !self.0.alive.get() {
                anyhow::bail!("主窗已关闭");
            }
            let dialog = hwnd(window).ok_or_else(|| anyhow::anyhow!("弹窗句柄尚未就绪"))?;
            self.0.dialog.set(dialog);
            unsafe {
                SetWindowLongPtrW(dialog, GWLP_HWNDPARENT, self.0.owner as isize);
                let style = GetWindowLongPtrW(dialog, GWL_EXSTYLE);
                SetWindowLongPtrW(
                    dialog,
                    GWL_EXSTYLE,
                    (style | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize),
                );
            }
            self.0.present()
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                if self.0.alive.get() {
                    RemoveWindowSubclass(self.0.owner, Some(block), ID);
                    if self.0.was_enabled {
                        EnableWindow(self.0.owner, 1);
                    }
                    SetForegroundWindow(self.0.owner);
                }
            }
        }
    }
    unsafe extern "system" fn block(
        hwnd: HWND,
        message: u32,
        wp: WPARAM,
        lp: LPARAM,
        _: usize,
        data: usize,
    ) -> LRESULT {
        unsafe {
            let state = &*(data as *const State);
            if matches!(message, WM_HOTKEY | WM_CLOSE | 0x8001 | 0x8002 | 0x8003) {
                let _ = state.present();
                return 0;
            }
            if message == WM_NCDESTROY {
                state.alive.set(false);
                RemoveWindowSubclass(hwnd, Some(block), ID);
            }
            DefSubclassProc(hwnd, message, wp, lp)
        }
    }
}
#[cfg(windows)]
thread_local! {static MODAL_GUARD:RefCell<Option<modality::Guard>>=const{RefCell::new(None)};}

impl App {
    fn begin_dialog(&self) -> Result<()> {
        #[cfg(windows)]
        {
            if let Some(platform) = self.platform.borrow().as_ref() {
                platform.apply_hotkeys(0, 0)?;
            }
            MODAL_GUARD.with(|slot| {
                slot.take();
            });
            let guard = modality::Guard::new(self.window.window())?;
            MODAL_GUARD.with(|slot| {
                slot.replace(Some(guard));
            });
        }
        Ok(())
    }
    fn attach_dialog(&self, window: &slint::Window) -> Result<()> {
        #[cfg(windows)]
        return MODAL_GUARD.with(|slot| {
            slot.borrow()
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("弹窗已关闭"))?
                .attach(window)
        });
        #[cfg(not(windows))]
        Ok(())
    }
    fn dialog_attached(&self, _window: &slint::Window) -> bool {
        #[cfg(windows)]
        return MODAL_GUARD.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|guard| guard.attached(_window))
        });
        #[cfg(not(windows))]
        true
    }
    fn watch_dialog<C: ComponentHandle + 'static>(&self, component: &C, settings: bool) {
        let weak = component.as_weak();
        Timer::single_shot(Duration::from_secs(3), move || {
            let Some(component) = weak.upgrade() else {
                return;
            };
            with_app(|app| {
                let current = if settings {
                    app.settings_view
                        .borrow()
                        .as_ref()
                        .is_some_and(|view| std::ptr::eq(view.window(), component.window()))
                } else {
                    app.search_view
                        .borrow()
                        .as_ref()
                        .is_some_and(|view| std::ptr::eq(view.window(), component.window()))
                };
                if current && !app.dialog_attached(component.window()) {
                    if settings {
                        app.close_settings();
                    } else {
                        app.close_search();
                    }
                    app.window.set_footer("弹窗初始化超时，请重试。".into());
                }
            });
        });
    }
    fn end_dialog(&self) {
        #[cfg(windows)]
        {
            MODAL_GUARD.with(|slot| {
                slot.take();
            });
            if let Some(platform) = self.platform.borrow().as_ref() {
                let settings = self.settings.borrow();
                if let Err(error) =
                    platform.apply_hotkeys(settings.toggle_shortcut, settings.refresh_shortcut)
                {
                    self.platform_message.replace(error.to_string());
                    self.window.set_footer(error.to_string().into());
                } else {
                    self.platform_message.borrow_mut().clear();
                }
            }
        }
    }
    pub(super) fn open_settings(self: &Rc<Self>) {
        if self.modal() {
            return;
        }
        self.show();
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
        w.set_global_options(font_options(s.global_font_percent, 120, false));
        w.set_quote_options(font_options(s.quote_font_percent, 150, true));
        w.set_depth_options(font_options(s.details_font_percent, 120, true));
        w.set_global_index(font_index(s.global_font_percent, 120, false));
        w.set_quote_index(font_index(s.quote_font_percent, 150, true));
        w.set_depth_index(font_index(s.details_font_percent, 120, true));
        self.recorded.set([s.toggle_shortcut, s.refresh_shortcut]);
        w.set_toggle_label(shortcut_label(s.toggle_shortcut).into());
        w.set_refresh_label(shortcut_label(s.refresh_shortcut).into());
        let global = ((s.global_font_percent * 110 + 50) / 100) as f32 / 100.;
        drop(s);
        w.on_key(|index, text, ctrl, alt, shift| {
            with_app(|a| {
                if !(0..2).contains(&index) {
                    return;
                }
                if let Some(w) = a.settings_view.borrow().as_ref() {
                    if let Some(value) = shortcut_value(&text, ctrl, alt, shift) {
                        let mut keys = a.recorded.get();
                        if value != 0 && keys[1 - index as usize] == value {
                            w.set_message("两个快捷键不能相同。".into());
                            return;
                        }
                        keys[index as usize] = value;
                        a.recorded.set(keys);
                        w.set_recording(-1);
                        w.set_toggle_label(shortcut_label(keys[0]).into());
                        w.set_refresh_label(shortcut_label(keys[1]).into());
                        w.set_message("".into());
                    } else {
                        w.set_message(
                            "快捷键无效。普通按键需 Ctrl 或 Alt；Tab / Esc 用于导航或取消。".into(),
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
        w.set_scale(global / self.window.window().scale_factor().max(1.));
        self.settings_view.replace(Some(w.clone_strong()));
        self.sync_update_view();
        if let Err(error) = self.begin_dialog() {
            self.close_settings();
            self.window.set_footer(error.to_string().into());
            return;
        }
        if let Err(error) = w.show() {
            self.close_settings();
            self.window.set_footer(error.to_string().into());
            return;
        }
        when_mapped(&w, move |mapped| {
            with_app(|app| {
                let current = app
                    .settings_view
                    .borrow()
                    .as_ref()
                    .is_some_and(|current| std::ptr::eq(current.window(), mapped.window()));
                if !current {
                    return;
                }
                if let Err(error) = app.attach_dialog(mapped.window()) {
                    app.close_settings();
                    app.window.set_footer(error.to_string().into());
                    return;
                }
                prepare_dialog(
                    mapped.window(),
                    app.window.window(),
                    (760, 832),
                    global,
                    |scale| mapped.set_scale(scale),
                );
            })
        });
        self.watch_dialog(&w, true);
    }
    fn close_settings(&self) {
        self.update_gen.set(self.update_gen.get().wrapping_add(1));
        if let Some(token) = self.update_token.take() {
            token.cancel();
        }
        if let Some(w) = self.settings_view.take() {
            self.end_dialog();
            let _ = w.hide();
        }
    }
    fn settings_action(self: &Rc<Self>, cmd: &str) {
        let current = self.settings_view.borrow();
        let Some(w) = current.as_ref() else { return };
        match cmd {
            "close" => {
                drop(current);
                self.close_settings();
            }
            "save" => {
                if w.get_update_busy() {
                    return;
                }
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
                for (value, index, maximum, local) in [
                    (&mut s.global_font_percent, w.get_global_index(), 120, false),
                    (&mut s.quote_font_percent, w.get_quote_index(), 150, true),
                    (&mut s.details_font_percent, w.get_depth_index(), 120, true),
                ] {
                    let Some(next) = font_choices(*value, maximum, local)
                        .get(index as usize)
                        .copied()
                    else {
                        w.set_message("请选择有效字号。".into());
                        return;
                    };
                    *value = next;
                }
                [s.toggle_shortcut, s.refresh_shortcut] = self.recorded.get();
                if !is_valid_shortcut(s.toggle_shortcut) || !is_valid_shortcut(s.refresh_shortcut) {
                    w.set_message("快捷键无效，请重新录入。".into());
                    return;
                }
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
                drop(current);
                self.check_update();
            }
            "download" => {
                drop(current);
                self.download_update();
            }
            _ => {}
        }
    }
    pub(super) fn open_search(self: &Rc<Self>) {
        if self.modal() {
            return;
        }
        self.show();
        self.close_depth();
        self.cancel_search();
        self.search_rows.borrow_mut().clear();
        let Ok(w) = SearchWindow::new() else { return };
        w.set_message("请输入编号或首字母，查询股票名称。".into());
        w.on_edited(|| {
            with_app(|a| {
                let view = a.search_view.borrow();
                let Some(w) = view.as_ref() else { return };
                if w.get_query().encode_utf16().count() > 256 {
                    w.set_query(limit_query(&w.get_query()).into());
                }
                let text = w.get_query().trim().to_string();
                if a.search_token.borrow().is_some() && *a.search_text.borrow() == text {
                    return;
                }
                a.cancel_search();
                a.search_rows.borrow_mut().clear();
                w.set_current(-1);
                w.set_anchor(-1);
                w.set_selected_rows(ModelRc::new(VecModel::<i32>::default()));
                w.set_busy(false);
                w.set_failed(false);
                w.set_query_ready(!text.is_empty());
                w.set_message(
                    if text.is_empty() {
                        "请输入编号或首字母，查询股票名称。"
                    } else {
                        "输入完成后自动搜索，也可按回车立即搜索。"
                    }
                    .into(),
                );
                drop(view);
                a.draw_matches();
                if !text.is_empty() {
                    a.debounce
                        .start(TimerMode::SingleShot, Duration::from_millis(300), || {
                            with_app(|a| a.start_search())
                        });
                }
            })
        });
        w.on_select(|index, ctrl, shift, keyboard| {
            with_app(|a| {
                let view = a.search_view.borrow();
                let Some(w) = view.as_ref() else { return };
                let previous = w.get_selected_rows().iter().collect::<Vec<_>>();
                let selected = selection_after_click(
                    &previous,
                    index,
                    w.get_anchor(),
                    a.search_rows.borrow().len(),
                    ctrl,
                    shift,
                    keyboard,
                );
                w.set_current(index);
                if !shift && !(keyboard && ctrl) {
                    w.set_anchor(index);
                }
                w.set_selected_rows(ModelRc::new(VecModel::from(selected)));
                drop(view);
                a.draw_matches();
            })
        });
        w.on_toggle(|index| {
            with_app(|a| {
                if let Some(row) = a.search_rows.borrow_mut().get_mut(index as usize) {
                    row.1 = !row.1;
                }
                a.draw_matches();
            })
        });
        w.on_action(|command| {
            with_app(|a| match command.as_str() {
                "close" => a.close_search(),
                "search" => {
                    a.debounce.stop();
                    a.start_search();
                }
                "toggle-selection" => {
                    if let Some(w) = a.search_view.borrow().as_ref() {
                        let index = w.get_current();
                        let mut rows = a.search_rows.borrow_mut();
                        if let Some((_, checked)) = rows.get(index as usize) {
                            let next = !*checked;
                            let selected = w.get_selected_rows().iter().collect::<Vec<_>>();
                            for i in selected.into_iter().chain(std::iter::once(index)) {
                                if let Some(row) = rows.get_mut(i as usize) {
                                    row.1 = next;
                                }
                            }
                        }
                    }
                    a.draw_matches();
                }
                "add" => {
                    let view = a.search_view.borrow();
                    let Some(w) = view.as_ref() else { return };
                    if w.get_busy() {
                        return;
                    }
                    let rows = a.search_rows.borrow();
                    let selected = selected_indices(
                        &rows.iter().map(|r| r.1).collect::<Vec<_>>(),
                        &w.get_selected_rows().iter().collect::<Vec<_>>(),
                    );
                    if selected.is_empty() {
                        w.set_failed(true);
                        w.set_message("请先在名称列表中选择股票。".into());
                        return;
                    }
                    let mut settings = a.settings.borrow().clone();
                    for i in selected {
                        if !settings.symbols.contains(&rows[i].0.symbol) {
                            settings.symbols.push(rows[i].0.symbol.clone());
                        }
                    }
                    drop(rows);
                    drop(view);
                    match a.save(settings) {
                        Ok(()) => a.close_search(),
                        Err(e) => {
                            if let Some(w) = a.search_view.borrow().as_ref() {
                                w.set_failed(true);
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
        let global = self.settings.borrow().global_font_percent as f32 / 100.;
        w.set_scale(global / self.window.window().scale_factor().max(1.));
        self.search_view.replace(Some(w.clone_strong()));
        if let Err(error) = self.begin_dialog() {
            self.close_search();
            self.window.set_footer(error.to_string().into());
            return;
        }
        if let Err(error) = w.show() {
            self.close_search();
            self.window.set_footer(error.to_string().into());
            return;
        }
        when_mapped(&w, move |mapped| {
            with_app(|app| {
                let current = app
                    .search_view
                    .borrow()
                    .as_ref()
                    .is_some_and(|current| std::ptr::eq(current.window(), mapped.window()));
                if !current {
                    return;
                }
                if let Err(error) = app.attach_dialog(mapped.window()) {
                    app.close_search();
                    app.window.set_footer(error.to_string().into());
                    return;
                }
                prepare_dialog(
                    mapped.window(),
                    app.window.window(),
                    (720, 660),
                    global,
                    |scale| mapped.set_scale(scale),
                );
            })
        });
        self.watch_dialog(&w, false);
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
            self.end_dialog();
            let _ = w.hide();
        }
    }
    fn start_search(self: &Rc<Self>) {
        let view = self.search_view.borrow();
        let Some(w) = view.as_ref() else { return };
        let query = w.get_query().trim().to_string();
        if query.is_empty() {
            w.set_query_ready(false);
            w.set_message("请输入编号或首字母，查询股票名称。".into());
            return;
        }
        if self.search_token.borrow().is_some() && *self.search_text.borrow() == query {
            return;
        }
        self.cancel_search();
        self.search_rows.borrow_mut().clear();
        w.set_current(-1);
        w.set_anchor(-1);
        w.set_selected_rows(ModelRc::new(VecModel::<i32>::default()));
        w.set_matches(ModelRc::new(VecModel::<SearchResult>::default()));
        w.set_count(0);
        w.set_busy(true);
        w.set_failed(false);
        w.set_query_ready(true);
        let generation = self.search_generation.get();
        let token = CancellationToken::new();
        self.search_token.replace(Some(token.clone()));
        self.search_text.replace(query.clone());
        w.set_message("正在查询股票名称…".into());
        if let Some(replay) = &self.replay {
            let rows = replay
                .quotes
                .iter()
                .filter(|q| q.symbol.starts_with(&query) || q.name.contains(&query))
                .map(|q| SymbolMatch {
                    symbol: q.symbol.clone(),
                    name: q.name.clone(),
                    initials: String::new(),
                })
                .collect();
            drop(view);
            self.finish_search(generation, Ok(rows));
        } else {
            let market = self.market.clone();
            self.runtime.spawn(async move {
                let rows = market
                    .search(&query, token)
                    .await
                    .map_err(|e| e.to_string());
                deliver(Completion::Search(generation, rows));
            });
        }
    }
    pub(super) fn finish_search(&self, generation: u64, result: Result<Vec<SymbolMatch>, String>) {
        if generation != self.search_generation.get() {
            return;
        }
        self.search_token.take();
        if let Some(w) = self.search_view.borrow().as_ref() {
            w.set_busy(false);
            match result {
                Ok(rows) => {
                    let mut seen = std::collections::HashSet::new();
                    let rows = rows
                        .into_iter()
                        .filter_map(|mut row| {
                            let id = normalize_symbol(&row.symbol).ok()?;
                            if row.name.trim().is_empty() || !seen.insert(id.clone()) {
                                return None;
                            }
                            row.symbol = id;
                            row.name = row.name.trim().to_string();
                            Some((row, false))
                        })
                        .collect::<Vec<_>>();
                    w.set_failed(false);
                    w.set_message(if rows.is_empty() {
                        "未找到匹配股票，请换编号或首字母重试。".into()
                    } else {
                        format!(
                            "找到 {} 个匹配，请核对名称、编号和市场后选择添加。",
                            rows.len()
                        )
                        .into()
                    });
                    self.search_rows.replace(rows);
                }
                Err(error) => {
                    self.search_rows.borrow_mut().clear();
                    w.set_failed(true);
                    w.set_message(search_failure_message(&error).into());
                }
            }
        }
        self.draw_matches();
    }
    fn draw_matches(&self) {
        if let Some(w) = self.search_view.borrow().as_ref() {
            let rows = self.search_rows.borrow();
            let selected = w.get_selected_rows().iter().collect::<Vec<_>>();
            let display = rows
                .iter()
                .enumerate()
                .map(|(index, (row, chosen))| {
                    let (code, market) = row.symbol.split_once('.').unwrap_or((&row.symbol, ""));
                    SearchResult {
                        name: row.name.as_str().into(),
                        code: code.into(),
                        market: match market {
                            "SH" => "沪市",
                            "SZ" => "深市",
                            "BJ" => "北交所",
                            _ => "—",
                        }
                        .into(),
                        chosen: *chosen,
                        selected: selected.contains(&(index as i32)),
                    }
                })
                .collect::<Vec<_>>();
            w.set_matches(ModelRc::new(VecModel::from(display)));
            w.set_count(
                selected_indices(&rows.iter().map(|r| r.1).collect::<Vec<_>>(), &selected).len()
                    as i32,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legal_existing_font_percentages_keep_their_actual_visible_choice() {
        for (value, maximum, local) in [
            (85, 120, false),
            (115, 120, true),
            (145, 150, true),
            (0, 150, true),
            (80, 120, false),
            (120, 120, false),
        ] {
            let values = font_choices(value, maximum, local);
            assert_eq!(values[font_index(value, maximum, local) as usize], value);
            assert_eq!(values.iter().filter(|n| **n == value).count(), 1);
        }
        assert_eq!(
            font_choices(85, 120, false),
            vec![80, 90, 100, 110, 120, 85]
        );
        assert_eq!(font_choices(145, 150, true).first(), Some(&0));
    }
    #[test]
    fn selection_is_explicit_and_checks_take_precedence_over_highlighted_rows() {
        assert!(selected_indices(&[false, false, false], &[]).is_empty());
        assert_eq!(selected_indices(&[false, false, false], &[1]), vec![1]);
        assert_eq!(
            selected_indices(&[false, false, false], &[0, 2]),
            vec![0, 2]
        );
        assert_eq!(selected_indices(&[false, true, false], &[0, 2]), vec![1]);
        assert_eq!(selected_indices(&[true, true, false], &[2]), vec![0, 1]);
        assert_eq!(selected_indices(&[false, false, false], &[2]), vec![2]);
    }
    #[test]
    fn row_click_multiselect_and_keyboard_navigation_do_not_toggle_checkboxes() {
        assert_eq!(
            selection_after_click(&[], 1, -1, 5, false, false, false),
            vec![1]
        );
        assert_eq!(
            selection_after_click(&[1], 3, 1, 5, true, false, false),
            vec![1, 3]
        );
        assert_eq!(
            selection_after_click(&[1, 3], 1, 3, 5, true, false, false),
            vec![3]
        );
        assert_eq!(
            selection_after_click(&[1], 3, 1, 5, false, true, true),
            vec![1, 2, 3]
        );
        assert_eq!(
            selection_after_click(&[1, 3], 4, 3, 5, true, false, true),
            vec![1, 3]
        );
        assert_eq!(
            selection_after_click(&[1], -1, 1, 5, false, false, false),
            vec![1]
        );
    }
    #[test]
    fn search_limits_and_errors_match_the_v105_input_boundary() {
        let value = "😀".repeat(129);
        assert_eq!(limit_query(&value).encode_utf16().count(), 256);
        assert_eq!(limit_query(&"6".repeat(257)).len(), 256);
        assert_eq!(
            search_failure_message("request timed out"),
            "查询超时，请重试。"
        );
        assert_eq!(
            search_failure_message("一次最多查询20个不同代码。"),
            "一次最多查询20个不同代码。"
        );
        assert_eq!(
            search_failure_message("error sending request for url https://example.test?q=private"),
            "查询失败，请检查网络后重试。"
        );
    }
    #[test]
    fn shortcut_capture_preserves_navigation_clearing_and_valid_winforms_bits() {
        assert_eq!(shortcut_value("q", true, true, false), Some(0x60051));
        assert_eq!(shortcut_value("r", false, true, false), Some(0x40052));
        assert_eq!(
            shortcut_value(&char::from(Key::F24).to_string(), false, false, false),
            Some(135)
        );
        assert_eq!(
            shortcut_value(&char::from(Key::UpArrow).to_string(), true, false, false),
            Some(0x20026)
        );
        assert_eq!(
            shortcut_value(&char::from(Key::Backspace).to_string(), false, false, false),
            Some(0)
        );
        assert_eq!(
            shortcut_value(&char::from(Key::Delete).to_string(), true, false, false),
            Some(0)
        );
        assert_eq!(
            shortcut_value(&char::from(Key::Escape).to_string(), true, false, false),
            None
        );
        assert_eq!(
            shortcut_value(&char::from(Key::Tab).to_string(), true, false, false),
            None
        );
        assert_eq!(shortcut_value("q", false, false, false), None);
        assert_eq!(shortcut_value("q", false, false, true), None);
        assert_eq!(shortcut_value("!", true, false, true), Some(0x30031));
    }
}
