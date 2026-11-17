//! The acceptance driver uses only the requested fixture and captures its own windows.
use super::*;
use anyhow::{Context, ensure};
use std::path::Path;

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
    let with_labels = depth.window().take_snapshot()?;
    depth.set_axis(ModelRc::default());
    let without_labels = depth.window().take_snapshot()?;
    depth.set_axis(labels);
    let width = with_labels.width() as usize;
    let rightmost = with_labels
        .as_bytes()
        .chunks_exact(4)
        .zip(without_labels.as_bytes().chunks_exact(4))
        .enumerate()
        .filter_map(|(i, (a, b))| (a != b).then_some(i % width))
        .max()
        .context("分时图没有绘制价格刻度")?;
    let scale = depth.get_scale() * depth.window().scale_factor();
    let gutter_edge = (18. + 72. - 8.) * scale;
    ensure!(
        (gutter_edge - rightmost as f32).abs() <= 5. * scale,
        "价格刻度没有在绘图区左侧右对齐，刻度右边缘 {rightmost}"
    );
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

fn click_name(app: &App, row: usize) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let position = name_position(app, row);
    let button = PointerEventButton::Left;
    let window = app.window.window();
    window.dispatch_event(WindowEvent::PointerPressed { position, button });
    window.dispatch_event(WindowEvent::PointerReleased { position, button });
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

pub fn start(app: Rc<App>, out: PathBuf) -> Result<Timer> {
    std::fs::create_dir(&out).context("验收输出必须是新目录")?;
    let mut phase = -5_i32;
    let mut hidden_tick = 0;
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(1500), move || {
        if phase == -5 && app.tick.get() < 2 && app.start.elapsed() < Duration::from_secs(20) {
            return;
        }
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
                    if phase == -1 {
                        ensure!(app.selection.borrow().as_deref() == Some(replay.quotes[0].symbol.as_str()), "悬停改变了先前的单击选中行");
                        depth.invoke_command("pin".into());
                        point_at_name(&app, 0);
                    } else {
                        ensure!(app.selection.borrow().is_none(), "仅移动鼠标不应选中行");
                        if phase == -2 {
                            click_name(&app, 0);
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
                }
                1 | 2 => {
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
                        check_numeric_columns(&d)?;
                        check_chart_axis(&d)?;
                        ensure!(d.window().size() == slint::PhysicalSize::new(960, 840), "盘口初始尺寸错误 {:?}", d.window().size());
                        capture(d.window(), &out.join("minute.png"))?;
                        d.invoke_command("unit".into());
                        d.invoke_command("daily".into());
                    } else {
                        ensure!(d.get_amount() && d.get_daily(), "盘口未切换为金额与日 K");
                        capture(d.window(), &out.join("daily.png"))?;
                        d.invoke_command("close".into());
                        #[cfg(windows)]
                        {
                            let mut settings = app.settings.borrow().clone();
                            settings.always_on_top = true;
                            app.save(settings)?;
                        }
                        app.command("settings");
                    }
                }
                3 => {
                    let w = app.settings_view.borrow().as_ref().context("设置没有打开")?.clone_strong();
                    #[cfg(windows)]
                    check_dialog_order(&app, w.window(), true)?;
                    capture(w.window(), &out.join("settings.png"))?;
                    w.set_topmost(false);
                    w.set_tab(1);
                    w.set_quote_index(4);
                    w.invoke_action("save".into());
                    ensure!(app.settings.borrow().quote_font_percent == 110, "字号未生效");
                    app.command("add");
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
                    std::fs::write(out.join("result.txt"), "PASS quote replay, 550x564 geometry, hover before and after click, pinned hover, price columns stable across lot widths and quantities at both table edges, depth and chart modes, settings save, explicit search, opacity and topmost, close-to-tray and restore")?;
                    app.command("exit");
                }
            }
            Ok(())
        })();
        if let Err(error) = test {
            let _ = std::fs::write(out.join("result.txt"), format!("FAIL phase {phase}: {error:#}"));
            std::process::exit(1);
        }
        phase += 1;
    });
    Ok(timer)
}
