use super::policy::{DesktopCommand, Hotkeys, resize_hit_test};
use crate::model::AppSettings;
use anyhow::{Context, Result, bail};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    rc::{Rc, Weak},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::Threading::CreateMutexW,
    UI::{
        Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_DISABLED, ODS_SELECTED, ODT_MENU},
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{
            MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, ReleaseCapture,
            UnregisterHotKey,
        },
        Shell::*,
        WindowsAndMessaging::*,
    },
};

const PRODUCT: &str = "轻盘 · 桌面看盘助手";
const WAKE: u32 = 0x8001;
const TRAY: u32 = WM_APP + 2;
const OPEN_MENU: u32 = WM_APP + 3;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const SUBCLASS_ID: usize = 0x4c54;
const CANVAS: u32 = rgb(19, 22, 27);
const TEXT: u32 = rgb(239, 238, 236);
const MUTED: u32 = rgb(164, 168, 177);
const SELECTED: u32 = rgb(74, 34, 42);
const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn title(preview: bool) -> &'static str {
    if preview {
        "轻盘 · 桌面看盘助手 · Rust 预览"
    } else {
        PRODUCT
    }
}

pub struct InstanceGuard(HANDLE);
impl InstanceGuard {
    /// `None` means the existing instance was asked to show. Preview never targets v1.0.5.
    pub fn acquire(preview: bool) -> Result<Option<Self>> {
        let name = wide(if preview {
            "Local\\LiteTicker.Desktop.RustPreview"
        } else {
            "Local\\LiteTicker.Desktop"
        });
        unsafe {
            let handle = CreateMutexW(null(), 0, name.as_ptr());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error()).context("无法创建轻盘单实例标识");
            }
            let exists = GetLastError() == ERROR_ALREADY_EXISTS;
            if exists {
                CloseHandle(handle);
                let hwnd = FindWindowW(null(), wide(title(preview)).as_ptr());
                if !hwnd.is_null() {
                    PostMessageW(hwnd, WAKE, 0, 0);
                    SetForegroundWindow(hwnd);
                }
                Ok(None)
            } else {
                Ok(Some(Self(handle)))
            }
        }
    }
}
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct Dispatch {
    callback: Box<dyn Fn(DesktopCommand)>,
    alive: Rc<Cell<bool>>,
    hwnd: HWND,
}
#[derive(Clone, Copy)]
enum Action {
    Command(DesktopCommand),
    Hide,
    Activate,
    RefreshVisible,
}
thread_local! {
    static DISPATCH: RefCell<HashMap<usize, Weak<Dispatch>>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<usize> = const { Cell::new(1) };
}
fn defer(id: usize, action: Action) {
    // Only the integer ID crosses the Send boundary; UI callbacks remain thread-local.
    let _ = slint::invoke_from_event_loop(move || {
        let dispatch = DISPATCH.with(|items| items.borrow().get(&id).and_then(Weak::upgrade));
        if let Some(dispatch) = dispatch.filter(|item| item.alive.get()) {
            match action {
                Action::Command(command) => (dispatch.callback)(command),
                Action::Hide => {
                    if unsafe { IsWindowVisible(dispatch.hwnd) } != 0 {
                        (dispatch.callback)(DesktopCommand::ToggleVisibility);
                    }
                }
                Action::Activate | Action::RefreshVisible => {
                    unsafe {
                        if IsWindowVisible(dispatch.hwnd) == 0 {
                            (dispatch.callback)(DesktopCommand::ToggleVisibility);
                        }
                        if dispatch.alive.get() {
                            ShowWindow(dispatch.hwnd, SW_RESTORE);
                            SetForegroundWindow(dispatch.hwnd);
                        }
                    }
                    if dispatch.alive.get() && matches!(action, Action::RefreshVisible) {
                        (dispatch.callback)(DesktopCommand::Refresh);
                    }
                }
            }
        }
    });
}

struct NativeState {
    hwnd: HWND,
    id: usize,
    preview: bool,
    alive: Rc<Cell<bool>>,
    hotkeys: RefCell<Hotkeys>,
    enable_hotkeys: bool,
    font: Cell<HFONT>,
    font_pixels: Cell<i32>,
    brush: HBRUSH,
    icon: HICON,
    taskbar_created: u32,
    topmost: Cell<bool>,
}

pub struct DesktopPlatform {
    state: Box<NativeState>,
    _dispatch: Rc<Dispatch>,
    _window_handle: slint::WindowHandle,
}

impl DesktopPlatform {
    pub fn new(
        window: &slint::Window,
        callback: impl Fn(DesktopCommand) + 'static,
        preview: bool,
        enable_hotkeys: bool,
    ) -> Result<Self> {
        let provider = window.window_handle();
        let hwnd = match provider
            .window_handle()
            .context("主窗原生句柄尚未创建，请在 show 后的事件循环中初始化平台")?
            .as_raw()
        {
            RawWindowHandle::Win32(value) => value.hwnd.get() as HWND,
            _ => bail!("轻盘桌面集成只支持 Win32 窗口"),
        };
        let mut previous = 0;
        if unsafe { GetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, &mut previous) } != 0 {
            bail!("主窗已经初始化桌面集成");
        }
        let id = NEXT_ID.with(|next| {
            let id = next.get();
            next.set(id.checked_add(1).expect("desktop dispatch IDs exhausted"));
            id
        });
        let alive = Rc::new(Cell::new(true));
        let dispatch = Rc::new(Dispatch {
            callback: Box::new(callback),
            alive: alive.clone(),
            hwnd,
        });
        let brush = unsafe { CreateSolidBrush(CANVAS) };
        if brush.is_null() {
            bail!("无法创建菜单背景画刷");
        }
        let icon = load_icon();
        if icon.is_null() {
            unsafe {
                DeleteObject(brush);
            }
            bail!("无法读取轻盘托盘图标");
        }
        let state = Box::new(NativeState {
            hwnd,
            id,
            preview,
            alive,
            hotkeys: RefCell::new(Hotkeys::default()),
            enable_hotkeys,
            font: Cell::new(null_mut()),
            font_pixels: Cell::new(22),
            brush,
            icon,
            taskbar_created: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
            topmost: Cell::new(
                unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } & WS_EX_TOPMOST as isize != 0,
            ),
        });
        let platform = Self {
            state,
            _dispatch: dispatch,
            _window_handle: provider,
        };
        DISPATCH.with(|items| {
            items
                .borrow_mut()
                .insert(id, Rc::downgrade(&platform._dispatch));
        });
        unsafe {
            if SetWindowSubclass(
                hwnd,
                Some(subclass),
                SUBCLASS_ID,
                platform.state.as_ref() as *const _ as usize,
            ) == 0
            {
                bail!("无法挂接轻盘桌面消息");
            }
            SetWindowTextW(hwnd, wide(title(preview)).as_ptr());
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                (style | WS_EX_TOOLWINDOW as isize | WS_EX_LAYERED as isize)
                    & !(WS_EX_APPWINDOW as isize),
            );
            let dark: i32 = 1;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                &dark as *const _ as *const _,
                size_of::<i32>() as u32,
            );
        }
        platform.state.replace_font(22)?;
        if !platform.state.add_tray() {
            bail!("无法创建轻盘系统托盘图标");
        }
        Ok(platform)
    }

    pub fn notify_update(&self, version: &str) {
        let mut data = self.state.tray_data();
        data.uFlags = NIF_INFO;
        let title = wide(&format!("轻盘有新版本 v{version}"));
        let text = wide("点击进入设置，下载并重启更新。");
        let n = title.len().min(data.szInfoTitle.len() - 1);
        data.szInfoTitle[..n].copy_from_slice(&title[..n]);
        data.szInfo[..text.len()].copy_from_slice(&text);
        data.dwInfoFlags = NIIF_INFO;
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }
    /// A hotkey conflict leaves both old registrations and appearance unchanged.
    pub fn apply_settings(&self, settings: &AppSettings) -> Result<()> {
        let state = &self.state;
        if !state.alive.get() {
            bail!("主窗已关闭，无法应用桌面设置");
        }
        let pixels =
            (22.0 * settings.global_font_percent.clamp(80, 120) as f64 / 100.0).round() as i32;
        let next_font = if pixels != state.font_pixels.get() {
            Some(create_font(pixels)?)
        } else {
            None
        };
        let key_result = if !state.enable_hotkeys {
            Ok(())
        } else {
            state.hotkeys.borrow_mut().apply(
                settings.toggle_shortcut,
                settings.refresh_shortcut,
                |id, key| unsafe {
                    let mut modifiers = MOD_NOREPEAT;
                    if key & 0x10000 != 0 {
                        modifiers |= MOD_SHIFT;
                    }
                    if key & 0x20000 != 0 {
                        modifiers |= MOD_CONTROL;
                    }
                    if key & 0x40000 != 0 {
                        modifiers |= MOD_ALT;
                    }
                    RegisterHotKey(state.hwnd, id, modifiers, key & 0xffff) != 0
                },
                |id| unsafe {
                    UnregisterHotKey(state.hwnd, id);
                },
            )
        };
        if let Err(error) = key_result {
            if let Some(font) = next_font {
                unsafe {
                    DeleteObject(font);
                }
            }
            bail!(error);
        }
        if let Some(font) = next_font {
            let previous = state.font.replace(font);
            state.font_pixels.set(pixels);
            unsafe {
                DeleteObject(previous);
            }
        }
        state.topmost.set(settings.always_on_top);
        unsafe {
            if SetLayeredWindowAttributes(
                state.hwnd,
                0,
                (settings.opacity_percent.clamp(60, 100) * 255 / 100) as u8,
                LWA_ALPHA,
            ) == 0
            {
                return Err(std::io::Error::last_os_error()).context("无法设置窗口透明度");
            }
            if SetWindowPos(
                state.hwnd,
                if settings.always_on_top {
                    HWND_TOPMOST
                } else {
                    HWND_NOTOPMOST
                },
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            ) == 0
            {
                return Err(std::io::Error::last_os_error()).context("无法设置窗口置顶");
            }
        }
        Ok(())
    }
    /// Post first: TrackPopupMenu must never nest inside a Slint input callback.
    pub fn show_quote_menu(&self, has_selection: bool) {
        unsafe {
            PostMessageW(self.state.hwnd, OPEN_MENU, usize::from(has_selection), 0);
        }
    }
    pub fn start_drag(&self) {
        unsafe {
            ReleaseCapture();
            PostMessageW(self.state.hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
        }
    }
    pub fn window_position(&self) -> (i32, i32) {
        let mut rect = RECT::default();
        unsafe {
            GetWindowRect(self.state.hwnd, &mut rect);
        }
        (rect.left, rect.top)
    }
    pub fn show_and_activate(&self) {
        unsafe {
            ShowWindow(self.state.hwnd, SW_RESTORE);
            SetForegroundWindow(self.state.hwnd);
        }
    }
}

impl Drop for DesktopPlatform {
    fn drop(&mut self) {
        let state = &self.state;
        DISPATCH.with(|items| {
            items.borrow_mut().remove(&state.id);
        });
        unsafe {
            if state.alive.replace(false) {
                RemoveWindowSubclass(state.hwnd, Some(subclass), SUBCLASS_ID);
                let nid = state.tray_data();
                Shell_NotifyIconW(NIM_DELETE, &nid);
                state.hotkeys.borrow_mut().clear(|id| {
                    UnregisterHotKey(state.hwnd, id);
                });
            }
            if !state.font.get().is_null() {
                DeleteObject(state.font.get());
            }
            DeleteObject(state.brush);
            DestroyIcon(state.icon);
        }
    }
}

impl NativeState {
    fn replace_font(&self, pixels: i32) -> Result<()> {
        let font = create_font(pixels)?;
        let old = self.font.replace(font);
        self.font_pixels.set(pixels);
        if !old.is_null() {
            unsafe {
                DeleteObject(old);
            }
        }
        Ok(())
    }
    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.hwnd;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = TRAY;
        data.hIcon = self.icon;
        let tip = wide(title(self.preview));
        let count = (tip.len() - 1).min(data.szTip.len() - 1);
        data.szTip[..count].copy_from_slice(&tip[..count]);
        data
    }
    fn add_tray(&self) -> bool {
        let mut data = self.tray_data();
        unsafe {
            if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
                return false;
            }
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
        true
    }
    fn menu(&self, tray: bool, selection: bool) {
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return;
        }
        struct OwnedMenu(HMENU);
        impl Drop for OwnedMenu {
            fn drop(&mut self) {
                unsafe {
                    DestroyMenu(self.0);
                }
            }
        }
        let _menu = OwnedMenu(menu);
        let ids: &[u32] = if tray {
            &[10, 2, 13, 11, 0, 12]
        } else {
            &[1, 2, 3, 4, 5, 0, 6, 7, 8, 9]
        };
        unsafe {
            let info = MENUINFO {
                cbSize: size_of::<MENUINFO>() as u32,
                fMask: MIM_BACKGROUND,
                hbrBack: self.brush,
                ..zeroed()
            };
            SetMenuInfo(menu, &info);
            for id in ids {
                let mut label = wide(menu_label(*id));
                let disabled = matches!(id, 1 | 3 | 4 | 5) && !selection;
                let item = MENUITEMINFOW {
                    cbSize: size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_ID | MIIM_FTYPE | MIIM_STRING | MIIM_DATA | MIIM_STATE,
                    fType: if *id == 0 {
                        MFT_SEPARATOR
                    } else {
                        MFT_OWNERDRAW
                    },
                    fState: if disabled { MFS_DISABLED } else { MFS_ENABLED },
                    wID: *id,
                    dwItemData: *id as usize,
                    dwTypeData: label.as_mut_ptr(),
                    ..zeroed()
                };
                if InsertMenuItemW(menu, u32::MAX, 1, &item) == 0 {
                    return;
                }
            }
            let mut cursor = POINT::default();
            GetCursorPos(&mut cursor);
            SetForegroundWindow(self.hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                cursor.x,
                cursor.y,
                0,
                self.hwnd,
                null(),
            ) as u32;
            PostMessageW(self.hwnd, WM_NULL, 0, 0);
            if command == 8 {
                defer(self.id, Action::Hide);
            } else if command == 10 {
                defer(self.id, Action::Command(DesktopCommand::ToggleVisibility));
            } else if command == 11 {
                defer(self.id, Action::RefreshVisible);
            } else if let Some(command) = menu_command(command) {
                defer(self.id, Action::Command(command));
            }
        }
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    unsafe {
        let state = &*(data as *const NativeState);
        match message {
            WM_HOTKEY => {
                if let Ok(keys) = state.hotkeys.try_borrow() {
                    if let Some(command) = keys.command(wp as i32) {
                        defer(state.id, Action::Command(command));
                    }
                }
                return 0;
            }
            WAKE => {
                defer(state.id, Action::Activate);
                return 0;
            }
            OPEN_MENU => {
                state.menu(false, wp != 0);
                return 0;
            }
            TRAY => {
                match lp as u32 & 0xffff {
                    NIN_BALLOONUSERCLICK => {
                        defer(state.id, Action::Command(DesktopCommand::ShowUpdates))
                    }
                    WM_CONTEXTMENU | WM_RBUTTONUP => state.menu(true, false),
                    WM_LBUTTONDBLCLK | NIN_KEYSELECT => defer(state.id, Action::Activate),
                    _ => {}
                }
                return 0;
            }
            WM_MOVE | WM_SIZE => defer(state.id, Action::Command(DesktopCommand::PositionDetails)),
            WM_CLOSE => {
                defer(state.id, Action::Hide);
                return 0;
            }
            WM_STYLECHANGING if wp as i32 == GWL_EXSTYLE && lp != 0 => {
                let style = &mut *(lp as *mut STYLESTRUCT);
                // winit recomputes these bits when hiding/showing the window.
                style.styleNew =
                    (style.styleNew | WS_EX_LAYERED | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
                if state.topmost.get() {
                    style.styleNew |= WS_EX_TOPMOST;
                } else {
                    style.styleNew &= !WS_EX_TOPMOST;
                }
            }
            WM_MEASUREITEM if lp != 0 => {
                let item = &mut *(lp as *mut MEASUREITEMSTRUCT);
                if item.CtlType == ODT_MENU {
                    let dc = GetDC(hwnd);
                    let old = SelectObject(dc, state.font.get());
                    let text = wide(menu_label(item.itemData as u32));
                    let mut size = SIZE::default();
                    GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32 - 1, &mut size);
                    SelectObject(dc, old);
                    ReleaseDC(hwnd, dc);
                    item.itemWidth = (size.cx + 52) as u32;
                    item.itemHeight = (size.cy.max(state.font_pixels.get()) + 16) as u32;
                    return 1;
                }
            }
            WM_DRAWITEM if lp != 0 => {
                let item = &*(lp as *const DRAWITEMSTRUCT);
                if item.CtlType == ODT_MENU {
                    let brush = CreateSolidBrush(if item.itemState & ODS_SELECTED != 0 {
                        SELECTED
                    } else {
                        CANVAS
                    });
                    FillRect(item.hDC, &item.rcItem, brush);
                    DeleteObject(brush);
                    let old_font = SelectObject(item.hDC, state.font.get());
                    let old_mode = SetBkMode(item.hDC, TRANSPARENT as i32);
                    let old_color = SetTextColor(
                        item.hDC,
                        if item.itemState & ODS_DISABLED != 0 {
                            MUTED
                        } else {
                            TEXT
                        },
                    );
                    let mut rect = item.rcItem;
                    rect.left += 24;
                    rect.right -= 20;
                    let text = wide(menu_label(item.itemData as u32));
                    DrawTextW(
                        item.hDC,
                        text.as_ptr(),
                        text.len() as i32 - 1,
                        &mut rect,
                        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                    );
                    SetTextColor(item.hDC, old_color);
                    SetBkMode(item.hDC, old_mode);
                    SelectObject(item.hDC, old_font);
                    return 1;
                }
            }
            WM_NCHITTEST => {
                let hit = DefSubclassProc(hwnd, message, wp, lp);
                if hit == HTCLIENT as isize {
                    let mut point = POINT {
                        x: lp as i16 as i32,
                        y: (lp >> 16) as i16 as i32,
                    };
                    ScreenToClient(hwnd, &mut point);
                    let mut rect = RECT::default();
                    GetClientRect(hwnd, &mut rect);
                    let dpi = GetDpiForWindow(hwnd).max(96) as i32;
                    return resize_hit_test(
                        point.x,
                        point.y,
                        rect.right,
                        rect.bottom,
                        (6 * dpi / 96).max(5),
                    ) as isize;
                }
                return hit;
            }
            WM_NCDESTROY => {
                let nid = state.tray_data();
                Shell_NotifyIconW(NIM_DELETE, &nid);
                state.alive.set(false);
                RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID);
            }
            _ if message == state.taskbar_created && message != 0 => {
                state.add_tray();
                return 0;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wp, lp)
    }
}

fn menu_label(id: u32) -> &'static str {
    match id {
        1 => "查看盘口",
        2 => "添加自选",
        3 => "删除选中",
        4 => "上移",
        5 => "下移",
        6 => "设置",
        7 => "更新行情 (F5)",
        8 => "隐藏到托盘",
        9 => "退出",
        10 => "显示 / 隐藏",
        11 => "更新行情",
        12 => "退出轻盘",
        13 => "设置 / 快捷键",
        _ => "",
    }
}
fn menu_command(id: u32) -> Option<DesktopCommand> {
    Some(match id {
        1 => DesktopCommand::ShowDetails,
        2 => DesktopCommand::AddSymbols,
        3 => DesktopCommand::RemoveSelected,
        4 => DesktopCommand::MoveUp,
        5 => DesktopCommand::MoveDown,
        6 | 13 => DesktopCommand::ShowSettings,
        7 => DesktopCommand::Refresh,
        8 => DesktopCommand::ToggleVisibility,
        9 | 12 => DesktopCommand::Exit,
        _ => return None,
    })
}
fn create_font(pixels: i32) -> Result<HFONT> {
    let font = unsafe {
        CreateFontW(
            -pixels,
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32,
            wide("Microsoft YaHei UI").as_ptr(),
        )
    };
    if font.is_null() {
        bail!("无法创建轻盘菜单字体");
    }
    Ok(font)
}
fn load_icon() -> HICON {
    let bytes = include_bytes!("../../../assets/liteticker.ico");
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let mut best = None;
    for index in 0..count {
        let offset = 6 + index * 16;
        if offset + 16 > bytes.len() {
            return null_mut();
        }
        let width = if bytes[offset] == 0 {
            256
        } else {
            bytes[offset] as i32
        };
        let length =
            u32::from_le_bytes(bytes[offset + 8..offset + 12].try_into().unwrap()) as usize;
        let start =
            u32::from_le_bytes(bytes[offset + 12..offset + 16].try_into().unwrap()) as usize;
        if start
            .checked_add(length)
            .is_none_or(|end| end > bytes.len())
        {
            return null_mut();
        }
        let distance = (width - 32).abs();
        if best.is_none_or(|(score, _, _)| distance < score) {
            best = Some((distance, start, length));
        }
    }
    best.map_or(null_mut(), |(_, start, length)| unsafe {
        CreateIconFromResourceEx(
            bytes[start..].as_ptr(),
            length as u32,
            1,
            0x00030000,
            32,
            32,
            0,
        )
    })
}
