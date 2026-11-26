//! The acceptance driver uses only the requested fixture and captures its own windows.
use super::*;
use anyhow::{Context, ensure};
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

fn capture(window: &slint::Window, file: &Path) -> Result<()> {
    let image = window.take_snapshot()?;
    let size =
        tiny_skia::IntSize::from_wh(image.width(), image.height()).context("窗口截图大小无效")?;
    tiny_skia::Pixmap::from_vec(image.as_bytes().to_vec(), size)
        .context("渲染缓冲区无效")?
        .save_png(file)?;
    Ok(())
}

// Only the lot counts change between captures. Both price columns must stay put
// and each final quantity must reach the right edge of its own table.
fn check_numeric_columns(depth: &DetailsWindow) -> Result<()> {
    let trades = depth.get_trades();
    let ladder = depth.get_ladder();
    let mut trade_rows: Vec<_> = trades.iter().collect();
    let mut levels: Vec<_> = ladder.iter().collect();
    let marker = Color::from_rgb_u8(250, 20, 192);
    trade_rows.first_mut().context("缺少成交回放")?.ink = marker;
    levels.first_mut().context("缺少五档回放")?.ink = marker;
    let mut first_ink = Vec::new();
    let mut previous_frame = Vec::new();
    for lots in ["7", "34", "1234567"] {
        trade_rows[0].size = lots.into();
        levels[0].size = lots.into();
        depth.set_trades(ModelRc::new(VecModel::from(trade_rows.clone())));
        depth.set_ladder(ModelRc::new(VecModel::from(levels.clone())));
        let image = depth.window().take_snapshot()?;
        let width = image.width() as usize;
        let pixels = image.as_bytes();
        let ink: Vec<_> = pixels
            .chunks_exact(4)
            .enumerate()
            .filter_map(|(i, p)| (p[0] > 190 && p[1] < 65 && p[2] > 145).then_some(i))
            .collect();
        ensure!(
            ink.iter().any(|i| i % width < width / 2) && ink.iter().any(|i| i % width >= width / 2),
            "五档或成交价格没有绘制"
        );
        if previous_frame.is_empty() {
            previous_frame = pixels.to_vec();
            first_ink = ink;
            continue;
        }
        ensure!(ink == first_ink, "手数 {lots} 改变了价格的绘制位置");
        let mut rightmost = [0; 2];
        for (i, (before, after)) in previous_frame
            .chunks_exact(4)
            .zip(pixels.chunks_exact(4))
            .enumerate()
        {
            if before != after {
                let x = i % width;
                let side = usize::from(x >= width / 2);
                rightmost[side] = rightmost[side].max(x);
            }
        }
        let scale = depth.get_scale() * depth.window().scale_factor();
        let edges = [
            (width as f32 - 20. * scale) / 2.,
            width as f32 - 18. * scale,
        ];
        for (ink_x, edge) in rightmost.into_iter().zip(edges) {
            ensure!(
                (edge - ink_x as f32).abs() <= 5. * scale,
                "末列手数未靠右，笔画像素 {ink_x}，表格右缘 {edge}"
            );
        }
        // Compare consecutive counts: 7 and 1234567 share the rightmost glyph.
        previous_frame.copy_from_slice(pixels);
    }
    depth.set_trades(trades);
    depth.set_ladder(ladder);
    Ok(())
}

fn check_chart_axis(depth: &DetailsWindow) -> Result<()> {
    let labels = depth.get_axis();
    ensure!(labels.row_count() == 5, "分时图缺少价格刻度");
    let chart = depth.get_chart();
    let scale = depth.get_scale() * depth.window().scale_factor();
    let mut short_plot = 0;
    for text in ["12.345", "-1234567890.123"] {
        depth.set_axis(ModelRc::new(VecModel::from(vec![text.into(); 5])));
        let frame = depth.window().take_snapshot()?;
        depth.set_chart(slint::Image::default());
        let blank_plot = depth.window().take_snapshot()?;
        depth.set_chart(chart.clone());
        let width = frame.width() as usize;
        // Clearing only the raster preserves text measurements and reveals its real left edge.
        let plot_left = frame
            .as_bytes()
            .chunks_exact(4)
            .zip(blank_plot.as_bytes().chunks_exact(4))
            .enumerate()
            .filter_map(|(i, (a, b))| (a != b).then_some(i % width))
            .min()
            .context("走势图没有绘制")?;
        let ink: Vec<_> = frame
            .as_bytes()
            .chunks_exact(4)
            .enumerate()
            .filter_map(|(i, p)| {
                (i % width < plot_left && p[..3] == [144, 142, 156]).then_some(i % width)
            })
            .collect();
        let left = *ink.iter().min().context("价格刻度没有绘制")? as f32;
        let right = *ink.iter().max().unwrap() as f32;
        ensure!(left >= 20. * scale - 1., "长价格刻度贴边或被裁切：{text}");
        ensure!(
            (plot_left as f32 - 8. * scale - right).abs() <= 5. * scale,
            "价格刻度没有随绘图区左边缘右对齐：{text}"
        );
        if short_plot == 0 {
            short_plot = plot_left;
        } else {
            ensure!(
                plot_left as f32 > short_plot as f32 + 8. * scale,
                "长刻度没有扩大纵轴宽度"
            );
        }
    }
    depth.set_axis(labels);
    Ok(())
}

fn name_position(app: &App, row: usize) -> slint::LogicalPosition {
    let window = app.window.window();
    let physical_row_height = (window.size().height as f32 - 48.) / 12.;
    let dpi = window.scale_factor();
    slint::LogicalPosition::new(
        45. / dpi,
        (8. + (row as f32 + 0.5) * physical_row_height) / dpi,
    )
}

fn point_at_name(app: &App, row: usize) {
    app.window
        .window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved {
            position: name_position(app, row),
        });
}

fn click_visible_name(app: &App, row: usize) -> Result<()> {
    use slint::platform::{PointerEventButton, WindowEvent};
    let position = name_position(app, row);
    let button = PointerEventButton::Left;
    let window = app.window.window();
    window.dispatch_event(WindowEvent::PointerPressed { position, button });
    check_hover_depth(app, Some(row), false).context("单击按下时盘口闪退")?;
    window.dispatch_event(WindowEvent::PointerReleased { position, button });
    check_hover_depth(app, Some(row), false).context("单击松开时盘口闪退")
}

#[derive(Clone, Copy)]
enum NamePointer {
    Hover,
    Press,
    Drag,
    Release,
}

fn name_pointer(app: &App, row: usize, offset: f32, action: NamePointer) -> Result<()> {
    let window = app.window.window();
    let mut position = name_position(app, row);
    position.x += offset;
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::{HiDpi::SetThreadDpiAwarenessContext, WindowsAndMessaging::*};
        let h = hwnd(window).context("缺少列表指针目标")?;
        let physical = position.to_physical(window.scale_factor());
        let at = ((physical.x as u16 as u32) | ((physical.y as u16 as u32) << 16)) as isize;
        let old = SetThreadDpiAwarenessContext(-4_isize as _);
        let mut sent = PostMessageW(
            h,
            WM_MOUSEMOVE,
            usize::from(matches!(action, NamePointer::Drag | NamePointer::Release)),
            at,
        ) != 0;
        match action {
            NamePointer::Press => sent &= PostMessageW(h, WM_LBUTTONDOWN, 1, at) != 0,
            NamePointer::Release => sent &= PostMessageW(h, WM_LBUTTONUP, 0, at) != 0,
            _ => {}
        }
        SetThreadDpiAwarenessContext(old);
        ensure!(sent, "无法发送列表指针输入");
    }
    #[cfg(not(windows))]
    {
        use slint::platform::{PointerEventButton, WindowEvent};
        window.dispatch_event(WindowEvent::PointerMoved { position });
        match action {
            NamePointer::Press => window.dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            }),
            NamePointer::Release => window.dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            }),
            _ => {}
        }
    }
    Ok(())
}

fn check_hover_depth(app: &App, row: Option<usize>, pinned: bool) -> Result<()> {
    let expected = row.map(|i| app.replay.as_ref().unwrap().quotes[i].symbol.clone());
    ensure!(
        *app.detail_symbol.borrow() == expected && app.pinned.get() == pinned,
        "拖动/悬停盘口状态错误：预期{expected:?} 固定{pinned}，实际{:?} 固定{}",
        app.detail_symbol.borrow(),
        app.pinned.get()
    );
    let depth = app.depth.borrow();
    let visible = depth.as_ref().is_some_and(|d| d.window().is_visible());
    ensure!(visible == row.is_some(), "拖动/悬停盘口可见性错误");
    #[cfg(windows)]
    if let Some(d) = depth.as_ref() {
        use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
        let h = hwnd(d.window()).context("盘口原生句柄丢失")?;
        ensure!(
            unsafe { IsWindowVisible(h) != 0 } == row.is_some(),
            "盘口原生可见性错误"
        );
    }
    Ok(())
}

// This coordinate fixture stays inside its own HWND; a different desktop's hardware
// cursor must not cancel its synthetic hover with an unrelated WM_MOUSELEAVE.
fn filter_pointer_leave(window: &slint::Window, active: bool) -> Result<()> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::Shell::{RemoveWindowSubclass, SetWindowSubclass};
        const ID: usize = 0x4c545250;
        unsafe extern "system" fn filter(
            h: *mut std::ffi::c_void,
            message: u32,
            wp: usize,
            lp: isize,
            _: usize,
            _: usize,
        ) -> isize {
            if message == windows_sys::Win32::UI::Controls::WM_MOUSELEAVE {
                return 0;
            }
            unsafe { windows_sys::Win32::UI::Shell::DefSubclassProc(h, message, wp, lp) }
        }
        let h = hwnd(window).context("缺少指针验收原生窗口")?;
        let changed = if active {
            SetWindowSubclass(h, Some(filter), ID, 0)
        } else {
            RemoveWindowSubclass(h, Some(filter), ID)
        };
        ensure!(changed != 0, "无法更新指针验收鼠标跟踪过滤");
    }
    #[cfg(not(windows))]
    let _ = (window, active);
    Ok(())
}

// Give the real event queue and the hover and leave timers time to run between inputs.
fn check_quote_drag(app: Rc<App>, out: PathBuf, complete: Rc<Cell<bool>>, step: u8) {
    let test = (|| -> Result<()> {
        match step {
            0 => {
                filter_pointer_leave(app.window.window(), true)?;
                app.close_depth();
                name_pointer(&app, 0, 0., NamePointer::Press)?;
            }
            1 => {
                check_hover_depth(&app, None, false)?;
                name_pointer(&app, 0, 8., NamePointer::Drag)?;
            }
            2 => {
                check_hover_depth(&app, None, false)?;
                name_pointer(&app, 1, 8., NamePointer::Drag)?;
            }
            3 => {
                check_hover_depth(&app, None, false)?;
                name_pointer(&app, 1, 8., NamePointer::Release)?;
                name_pointer(&app, 1, 0., NamePointer::Hover)?;
            }
            4 => {
                check_hover_depth(&app, Some(1), false)?;
                name_pointer(&app, 0, 0., NamePointer::Hover)?;
            }
            5 => {
                check_hover_depth(&app, Some(0), false)?;
                name_pointer(&app, 0, 0., NamePointer::Press)?;
                name_pointer(&app, 0, 8., NamePointer::Drag)?;
            }
            6 => {
                check_hover_depth(&app, None, false)?;
                name_pointer(&app, 0, 8., NamePointer::Release)?;
                name_pointer(&app, 0, 0., NamePointer::Hover)?;
            }
            7 => {
                check_hover_depth(&app, Some(0), false)?;
                name_pointer(&app, 0, 0., NamePointer::Press)?;
                name_pointer(&app, 0, 0., NamePointer::Release)?;
                let next_app = app.clone();
                let next_out = out.clone();
                Timer::single_shot(Duration::from_millis(30), move || {
                    let gesture = (|| -> Result<()> {
                        name_pointer(&next_app, 0, 0., NamePointer::Press)?;
                        name_pointer(&next_app, 0, 6., NamePointer::Drag)?;
                        name_pointer(&next_app, 0, 6., NamePointer::Release)
                    })();
                    if let Err(error) = gesture {
                        let _ = std::fs::write(
                            next_out.join("result.txt"),
                            format!("FAIL second-click drag input: {error:#}"),
                        );
                        std::process::exit(1);
                    }
                });
            }
            8 => {
                ensure!(!app.pinned.get(), "短间隔第二次按下后拖动误触发双击固定");
                name_pointer(&app, 0, 0., NamePointer::Hover)?;
            }
            9 => {
                check_hover_depth(&app, Some(0), false)?;
                app.depth
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .invoke_command("pin".into());
                name_pointer(&app, 0, 0., NamePointer::Press)?;
                name_pointer(&app, 1, 8., NamePointer::Drag)?;
            }
            10 => {
                check_hover_depth(&app, Some(0), true)?;
                name_pointer(&app, 1, 8., NamePointer::Release)?;
                name_pointer(&app, 1, 0., NamePointer::Hover)?;
            }
            11 => {
                check_hover_depth(&app, Some(0), true)?;
                app.close_depth();
                name_pointer(&app, 0, 0., NamePointer::Hover)?;
            }
            12 => {
                check_hover_depth(&app, Some(0), false)?;
                app.window
                    .window()
                    .dispatch_event(slint::platform::WindowEvent::PointerExited);
                let depth = app.depth.borrow().as_ref().unwrap().clone_strong();
                let window = depth.window();
                let scale = depth.get_scale() * window.scale_factor();
                filter_pointer_leave(window, true)?;
                window.dispatch_event(slint::platform::WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(20., 200.),
                });
                click_dialog(window, 173. * scale, 138. * scale)?;
            }
            13 => {
                check_hover_depth(&app, Some(0), false).context("点击日 K 后盘口应保持打开")?;
                let depth = app.depth.borrow().as_ref().unwrap().clone_strong();
                ensure!(depth.get_daily(), "点击日 K 未切换图表");
                let scale = depth.get_scale() * depth.window().scale_factor();
                click_dialog(depth.window(), 67. * scale, 138. * scale)?;
            }
            14 => {
                check_hover_depth(&app, Some(0), false).context("点击分时后盘口应保持打开")?;
                let depth = app.depth.borrow().as_ref().unwrap().clone_strong();
                ensure!(!depth.get_daily(), "点击分时未切换图表");
                filter_pointer_leave(depth.window(), false)?;
                depth
                    .window()
                    .dispatch_event(slint::platform::WindowEvent::PointerExited);
            }
            15 => {
                check_hover_depth(&app, None, false).context("移出未固定的盘口后应自动关闭")?;
                point_at_name(&app, 0);
            }
            16 => {
                use slint::platform::{PointerEventButton, WindowEvent};
                check_hover_depth(&app, Some(0), false)?;
                app.window
                    .window()
                    .dispatch_event(WindowEvent::PointerPressed {
                        position: name_position(&app, 0),
                        button: PointerEventButton::Left,
                    });
                app.window
                    .window()
                    .dispatch_event(WindowEvent::PointerExited);
            }
            17 => {
                check_hover_depth(&app, None, false).context("按住后移出列表，未固定盘口应关闭")?;
                app.window.invoke_open_row(0);
                filter_pointer_leave(app.window.window(), false)?;
                complete.set(true);
                return Ok(());
            }
            _ => unreachable!(),
        }
        Ok(())
    })();
    if let Err(error) = test {
        let _ = std::fs::write(
            out.join("result.txt"),
            format!("FAIL quote pointer step {step}: {error:#}"),
        );
        std::process::exit(1);
    }
    if !complete.get() {
        let mut delay = if step == 7 { 600 } else { 300 };
        if step == 6 {
            // Start a new pair independently of the preceding drag's press.
            #[cfg(windows)]
            {
                delay = unsafe {
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime()
                } as u64
                    + 100;
            }
            #[cfg(not(windows))]
            {
                delay = 1100;
            }
        }
        Timer::single_shot(Duration::from_millis(delay), move || {
            check_quote_drag(app, out, complete, step + 1);
        });
    }
}

#[cfg(windows)]
fn check_dialog_order(app: &App, window: &slint::Window, topmost: bool) -> Result<()> {
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*,
    };
    let owner = hwnd(app.window.window()).context("缺少行情窗口")?;
    let dialog = hwnd(window).context("缺少弹窗")?;
    unsafe {
        ensure!(
            GetWindow(dialog, GW_OWNER) == owner
                && IsWindowEnabled(owner) == 0
                && IsWindowEnabled(dialog) != 0
                && IsWindowVisible(dialog) != 0,
            "弹窗模态状态错误"
        );
        for h in [owner, dialog] {
            ensure!(
                (GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_TOPMOST as isize != 0) == topmost,
                "弹窗与行情窗口没有处于预期的置顶层"
            );
        }
        let mut next = GetWindow(dialog, GW_HWNDNEXT);
        while !next.is_null() && next != owner {
            next = GetWindow(next, GW_HWNDNEXT);
        }
        ensure!(next == owner, "行情列表挡住了弹窗");
    }
    Ok(())
}

fn click_dialog(window: &slint::Window, x: f32, y: f32) -> Result<()> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::{HiDpi::SetThreadDpiAwarenessContext, WindowsAndMessaging::*};
        let h = hwnd(window).context("缺少输入目标窗口")?;
        let old = SetThreadDpiAwarenessContext(-4_isize as _);
        let at = ((x.round() as u16 as u32) | ((y.round() as u16 as u32) << 16)) as isize;
        // winit takes click positions from the preceding mouse move, not button lParam.
        let sent = PostMessageW(h, WM_MOUSEMOVE, 0, at) != 0
            && PostMessageW(h, WM_LBUTTONDOWN, 1, at) != 0
            && PostMessageW(h, WM_LBUTTONUP, 0, at) != 0;
        SetThreadDpiAwarenessContext(old);
        ensure!(sent, "无法发送弹窗鼠标输入");
    }
    #[cfg(not(windows))]
    {
        use slint::platform::{PointerEventButton, WindowEvent};
        let position =
            slint::LogicalPosition::new(x / window.scale_factor(), y / window.scale_factor());
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }
    Ok(())
}

// The OS move loop can consume client-up. A following tab click must still work.
fn nonclient_title_release(window: &slint::Window, scale: f32) -> Result<Arc<AtomicU8>> {
    let state = Arc::new(AtomicU8::new(0));
    #[cfg(windows)]
    {
        let handle = hwnd(window).context("缺少标题拖动目标")? as isize;
        let completed = state.clone();
        std::thread::spawn(move || unsafe {
            use windows_sys::Win32::{
                Foundation::POINT,
                Graphics::Gdi::ClientToScreen,
                UI::{HiDpi::SetThreadDpiAwarenessContext, WindowsAndMessaging::*},
            };
            let h = handle as _;
            let old = SetThreadDpiAwarenessContext(-4_isize as _);
            let sent = (|| {
                let mut owner = 0;
                GetWindowThreadProcessId(h, &mut owner);
                if owner != std::process::id() {
                    return false;
                }
                let pack =
                    |x: i32, y: i32| ((x as u16 as u32) | ((y as u16 as u32) << 16)) as isize;
                let first = pack((255. * scale).round() as i32, (64. * scale).round() as i32);
                let mut last = POINT {
                    x: (273. * scale).round() as i32,
                    y: (73. * scale).round() as i32,
                };
                if PostMessageW(h, WM_MOUSEMOVE, 0, first) == 0 {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(120));
                if PostMessageW(h, WM_LBUTTONDOWN, 1, first) == 0 {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(120));
                if PostMessageW(h, WM_MOUSEMOVE, 1, pack(last.x, last.y)) == 0 {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(80));
                if ClientToScreen(h, &mut last) == 0 {
                    return false;
                }
                // Intentionally no WM_LBUTTONUP: reproduce release in the native move loop.
                PostMessageW(h, WM_NCLBUTTONUP, HTCAPTION as usize, pack(last.x, last.y)) != 0
            })();
            SetThreadDpiAwarenessContext(old);
            completed.store(if sent { 1 } else { 2 }, Ordering::Release);
        });
    }
    #[cfg(not(windows))]
    {
        click_dialog(window, 255. * scale, 64. * scale)?;
        state.store(1, Ordering::Release);
    }
    Ok(state)
}

fn checked_toggle_position(w: &SettingsWindow) -> Result<(f32, f32)> {
    let image = w.window().take_snapshot()?;
    let scale = w.get_scale() * w.window().scale_factor();
    let width = image.width() as usize;
    let mut bounds = (usize::MAX, usize::MAX, 0, 0);
    for (index, pixel) in image.as_bytes().chunks_exact(4).enumerate() {
        let (x, y) = (index % width, index / width);
        if x > width / 2
            && (y as f32) > 500. * scale
            && (y as f32) < 565. * scale
            && pixel[..3] == [222, 69, 82]
        {
            bounds.0 = bounds.0.min(x);
            bounds.1 = bounds.1.min(y);
            bounds.2 = bounds.2.max(x);
            bounds.3 = bounds.3.max(y);
        }
    }
    ensure!(bounds.0 != usize::MAX, "未找到可见置顶开关");
    Ok((
        ((bounds.0 + bounds.2) as f32) / 2.,
        ((bounds.1 + bounds.3) as f32) / 2.,
    ))
}

#[derive(Default)]
struct SettingsInput {
    step: u8,
    quote_before: i32,
    toggle: (f32, f32),
    title_release: Option<Arc<AtomicU8>>,
}

fn check_settings_inputs(app: &Rc<App>, out: &Path, input: &mut SettingsInput) -> Result<bool> {
    if input.step == 7 {
        ensure!(app.settings_view.borrow().is_none(), "取消按钮没有关闭设置");
        ensure!(
            app.settings.borrow().quote_font_percent == input.quote_before
                && app.settings.borrow().always_on_top,
            "取消提交了未保存的设置"
        );
        app.command("settings");
    } else if input.step == 11 {
        ensure!(app.settings_view.borrow().is_none(), "保存按钮没有关闭设置");
        ensure!(
            app.settings.borrow().quote_font_percent == 110 && !app.settings.borrow().always_on_top,
            "鼠标保存没有应用字号/置顶设置"
        );
        return Ok(true);
    } else {
        let w = app
            .settings_view
            .borrow()
            .as_ref()
            .context("设置输入期间窗口消失")?
            .clone_strong();
        let scale = w.get_scale() * w.window().scale_factor();
        // Logical design anchors, scaled by the actual dialog DPI and fitted font size.
        let tab =
            |index: usize| click_dialog(w.window(), [82., 196., 320.][index] * scale, 149. * scale);
        let footer = |save: bool| {
            click_dialog(
                w.window(),
                w.window().size().width as f32 - if save { 107. } else { 254. } * scale,
                w.window().size().height as f32 - 52. * scale,
            )
        };
        match input.step {
            0 => {
                #[cfg(windows)]
                check_dialog_order(app, w.window(), true)?;
                ensure!(w.get_tab() == 0 && w.get_topmost(), "设置初始页/置顶不正确");
                input.quote_before = app.settings.borrow().quote_font_percent;
                input.toggle = checked_toggle_position(&w)?;
                capture(w.window(), &out.join("settings.png"))?;
                input.title_release = Some(nonclient_title_release(w.window(), scale)?);
            }
            1 => {
                ensure!(
                    input
                        .title_release
                        .as_ref()
                        .context("标题输入未开始")?
                        .load(Ordering::Acquire)
                        == 1,
                    "标题非客户区释放输入未完成"
                );
                tab(1)?;
            }
            2 => {
                ensure!(
                    w.get_tab() == 1,
                    "标题拖动释放后字体页不能点击，可能仍有鼠标grab"
                );
                capture(w.window(), &out.join("settings-font.png"))?;
                // Preserve the existing save/cancel font fixture; pages and buttons use pointer input.
                w.set_quote_index(4);
                tab(2)?;
            }
            3 => {
                ensure!(w.get_tab() == 2, "版本页不能点击");
                capture(w.window(), &out.join("settings-version.png"))?;
                tab(0)?;
            }
            4 => {
                ensure!(w.get_tab() == 0, "常规页不能点击");
                click_dialog(w.window(), input.toggle.0, input.toggle.1)?;
            }
            5 => {
                ensure!(!w.get_topmost(), "置顶开关第一次点击无效");
                click_dialog(w.window(), input.toggle.0, input.toggle.1)?;
            }
            6 => {
                ensure!(w.get_topmost(), "置顶开关第二次点击没有还原");
                footer(false)?;
            }
            8 => {
                input.toggle = checked_toggle_position(&w)?;
                click_dialog(w.window(), input.toggle.0, input.toggle.1)?;
            }
            9 => {
                ensure!(!w.get_topmost(), "重新打开后置顶开关不能点击");
                tab(1)?;
            }
            10 => {
                ensure!(w.get_tab() == 1, "重新打开后字体页不能点击");
                w.set_quote_index(4);
                footer(true)?;
            }
            _ => unreachable!(),
        }
    }
    input.step += 1;
    Ok(false)
}

pub fn start(app: Rc<App>, out: PathBuf) -> Result<Timer> {
    std::fs::create_dir(&out).context("验收输出必须是新目录")?;
    let mut phase = -5_i32;
    let mut hidden_tick = 0;
    let mut settings_input = SettingsInput::default();
    let drag_checked = Rc::new(Cell::new(false));
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(1500), move || {
        if phase == -5 && app.tick.get() < 2 && app.start.elapsed() < Duration::from_secs(20) {
            return;
        }
        let mut advance = true;
        let test = (|| -> Result<()> {
            match phase {
                -5 => point_at_name(&app, 0),
                -4..=-1 => {
                    let row = if phase == -3 || phase == -1 { 1 } else { 0 };
                    let replay = app.replay.as_ref().context("缺少行情回放")?;
                    let quote = &replay.quotes[row];
                    ensure!(app.detail_symbol.borrow().as_deref() == Some(quote.symbol.as_str()), "悬停第{}行未切到{}，当前{:?}", row + 1, quote.symbol, app.detail_symbol.borrow());
                    ensure!(!app.pinned.get(), "悬停或单击不应固定盘口");
                    let depth = app.depth.borrow().as_ref().context("悬停未打开盘口")?.clone_strong();
                    ensure!(depth.get_heading().contains(quote.name.as_str()), "盘口名称未跟随鼠标");
                    if quote.symbol != replay.details.symbol {
                        ensure!(depth.get_overlay_value().is_empty() && !depth.get_has_leading(), "切换证券后仍残留上一标的的分时数据");
                    }
                    if phase == -1 {
                        ensure!(app.selection.borrow().as_deref() == Some(replay.quotes[0].symbol.as_str()), "悬停改变了先前的单击选中行");
                        depth.invoke_command("pin".into());
                        point_at_name(&app, 0);
                    } else {
                        ensure!(app.selection.borrow().is_none(), "仅移动鼠标不应选中行");
                        if phase == -2 {
                            click_visible_name(&app, 0)?;
                            if depth.get_has_chart() && !app.details.borrow().snapshot(app.start.elapsed(), None).stale {
                                let image = depth.get_chart();
                                app.close_depth();
                                check_hover_depth(&app, None, false)?;
                                app.show_depth(&quote.symbol, false);
                                check_hover_depth(&app, Some(0), false)?;
                                ensure!(image == depth.get_chart(), "同一证券的有效缓存重新绘制了分时图");
                            }
                        }
                        point_at_name(&app, 1 - row);
                    }
                }
                0 => {
                    ensure!(app.pinned.get() && app.detail_symbol.borrow().as_deref() == Some(app.replay.as_ref().unwrap().quotes[1].symbol.as_str()), "固定后悬停不应更换标的");
                    let replay = app.replay.as_ref().context("需要回放数据")?;
                    ensure!(app.rows.row_count() == replay.quotes.len() && app.tick.get() >= 2, "回放未就绪：{} 行，{} 次刷新，{}", app.rows.row_count(), app.tick.get(), app.window.get_footer());
                    for (i, q) in replay.quotes.iter().enumerate() {
                        let row = app.rows.row_data(i).context("报价缺行")?;
                        let offset = if i == 0 { Decimal::new(((app.tick.get() - 1) % 2) as i64, 2) } else { Decimal::ZERO };
                        ensure!(row.name == q.name && row.price == format_price(q.price + offset)
                            && row.change == signed(q.change_percent) && row.volume == format_volume_lots(q.volume_lots), "回放第 {} 行不匹配", i+1);
                    }
                    #[cfg(windows)]
                    ensure!(app.platform.borrow().is_some() && app.platform_message.borrow().is_empty(), "Win32 桌面功能未完成初始化");
                    let size = app.window.window().size();
                    ensure!(size.width == 550 && size.height == 564, "20只回放小窗尺寸错误 {:?}", size);
                    capture(app.window.window(), &out.join("quotes.png"))?;
                    app.window.invoke_select_row(0);
                    app.window.invoke_open_row(0);
                    check_quote_drag(app.clone(), out.clone(), drag_checked.clone(), 0);
                }
                1 | 2 => {
                    if !drag_checked.get() {
                        advance = false;
                        return Ok(());
                    }
                    let d = app.depth.borrow().as_ref().context("盘口没有打开")?.clone_strong();
                    ensure!(d.get_pinned() && d.get_has_chart() && d.get_ladder().row_count() == 10, "盘口、图表或五档未就绪");
                    ensure!(d.get_trades().row_count() == 10, "成交表必须显示10行数据或占位");
                    let replay = app.replay.as_ref().context("缺少成交回放")?;
                    if replay.details.trades.len() >= 10 {
                        let mut expected = replay.details.trades.iter().collect::<Vec<_>>();
                        expected.sort_by(|a, b| b.time.cmp(&a.time));
                        for index in [8usize, 9] {
                            let row = d.get_trades().row_data(index).context("第9或第10笔成交缺行")?;
                            let trade = expected[index];
                            ensure!(row.time.as_str() == trade.time.as_str()
                                && row.price == format_price(trade.price)
                                && row.size == format_volume_lots(trade.volume_lots),
                                "第{}笔成交的时间、价格或手数与回放不符", index + 1);
                        }
                    }
                    if phase == 1 {
                        let index = replay.details.symbol == SHANGHAI_COMPOSITE;
                        let has_leading = index && replay.details.minutes.iter().any(|m| m.leading > Decimal::ZERO);
                        let expected = replay.details.minutes.last().map(|m| if index { m.leading } else { m.average })
                            .filter(|v| *v > Decimal::ZERO).map(format_price).unwrap_or_default();
                        ensure!(d.get_overlay_value().as_str() == expected, "盘口图例数值与最新分时数据不符");
                        let legend = d.get_chart_legend();
                        ensure!(legend.contains("上证领先") == has_leading && legend.contains("均价") != index, "分时图例与标的或数据不符：{legend}");
                        check_numeric_columns(&d)?;
                        check_chart_axis(&d)?;
                        ensure!(d.window().size() == slint::PhysicalSize::new(960, 840), "盘口初始尺寸错误 {:?}", d.window().size());
                        capture(d.window(), &out.join("minute.png"))?;
                        d.invoke_command("unit".into());
                        d.invoke_command("daily".into());
                    } else {
                        ensure!(d.get_amount() && d.get_daily(), "盘口未切换为金额与日 K");
                        ensure!(d.get_overlay_value().is_empty() && d.get_chart_legend() == "日 K", "日 K 仍显示分时图例或数值");
                        capture(d.window(), &out.join("daily.png"))?;
                        d.invoke_command("close".into());
                        #[cfg(windows)]
                        {
                            let mut settings = app.settings.borrow().clone();
                            settings.always_on_top = true;
                            app.save(settings)?;
                        }
                        let ticks_before_settings = app.tick.get();
                        app.command("settings");
                        ensure!(app.tick.get() == ticks_before_settings, "主窗可见时打开设置触发了行情刷新");
                    }
                }
                3 => {
                    if check_settings_inputs(&app, &out, &mut settings_input)
                        .with_context(|| format!("设置真实输入步骤{}", settings_input.step))?
                    {
                        app.command("add");
                    } else {
                        advance = false;
                    }
                }
                4 => {
                    let w = app.search_view.borrow().as_ref().context("添加自选没有打开")?.clone_strong();
                    #[cfg(windows)]
                    check_dialog_order(&app, w.window(), false)?;
                    w.set_query(app.replay.as_ref().unwrap().quotes[0].symbol[..6].into());
                    w.invoke_edited();
                    w.invoke_action("search".into());
                }
                5 => {
                    let w = app.search_view.borrow().as_ref().context("搜索窗口消失")?.clone_strong();
                    ensure!(w.get_matches().row_count() > 0, "无匹配结果");
                    w.invoke_toggle(0);
                    ensure!(w.get_count() == 1, "未显式勾选结果");
                    w.invoke_select(0, false, false, false);
                    capture(w.window(), &out.join("search.png"))?;
                    w.invoke_action("close".into());
                    #[cfg(windows)]
                    {
                        use windows_sys::Win32::UI::WindowsAndMessaging::*;
                        let h = hwnd(app.window.window()).context("缺少主窗 HWND")?;
                        for opacity in [60, 80, 100] {
                            let mut settings = app.settings.borrow().clone();
                            settings.opacity_percent = opacity;
                            settings.always_on_top = opacity == 80;
                            app.save(settings)?;
                            let mut alpha = 0;
                            let mut flags = 0;
                            unsafe {
                                ensure!(GetLayeredWindowAttributes(h, std::ptr::null_mut(), &mut alpha, &mut flags) != 0);
                                ensure!(flags & LWA_ALPHA != 0 && alpha as i32 == opacity * 255 / 100, "整窗透明度未生效");
                                ensure!((GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0) == (opacity == 80), "置顶设置未生效");
                            }
                        }
                        unsafe { SendMessageW(h, WM_CLOSE, 0, 0); }
                    }
                    #[cfg(not(windows))]
                    app.hide();
                    hidden_tick = app.tick.get();
                }
                6 => {
                    ensure!(!app.visible.get() && app.tick.get() == hidden_tick, "隐藏期间仍刷新");
                    #[cfg(windows)]
                    unsafe {
                        use windows_sys::Win32::UI::WindowsAndMessaging::*;
                        let h = hwnd(app.window.window()).context("隐藏期间主窗句柄丢失")?;
                        ensure!(IsWindowVisible(h) == 0, "隐藏后窗口仍可见");
                        // The shell delivers this notification to the tray owner's window.
                        SendMessageW(h, WM_APP + 2, 0, WM_LBUTTONDBLCLK as isize);
                    }
                    #[cfg(not(windows))]
                    app.show();
                }
                _ => {
                    ensure!(app.visible.get() && app.tick.get() > hidden_tick, "托盘恢复后未刷新");
                    std::fs::write(out.join("result.txt"), "PASS quote replay, 550x564 geometry, hover before and after click, held pointer and cross-row drag suppress unpinned depth, release resumes hover, pinned depth survives drag, unpinned depth stays open when switching charts and closes on pointer exit, price columns stable across lot widths and quantities at both table edges, depth and chart modes, settings tabs after title drag, toggle, cancel and save, explicit search, opacity and topmost, close-to-tray and restore")?;
                    app.command("exit");
                }
            }
            Ok(())
        })();
        if let Err(error) = test {
            let _ = std::fs::write(out.join("result.txt"), format!("FAIL phase {phase}: {error:#}"));
            std::process::exit(1);
        }
        if advance { phase += 1; }
    });
    Ok(timer)
}
