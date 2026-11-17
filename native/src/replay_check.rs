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

pub fn start(app: Rc<App>, out: PathBuf) -> Result<Timer> {
    std::fs::create_dir(&out).context("验收输出必须是新目录")?;
    let mut phase = 0;
    let mut hidden_tick = 0;
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(1500), move || {
        if phase == 0 && app.tick.get() < 2 && app.start.elapsed() < Duration::from_secs(20) {
            return;
        }
        let test = (|| -> Result<()> {
            match phase {
                0 => {
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
                    if phase == 1 {
                        capture(d.window(), &out.join("minute.png"))?;
                        d.invoke_command("unit".into());
                        d.invoke_command("daily".into());
                    } else {
                        ensure!(d.get_amount() && d.get_daily(), "盘口未切换为金额与日 K");
                        capture(d.window(), &out.join("daily.png"))?;
                        d.invoke_command("close".into());
                        app.command("settings");
                    }
                }
                3 => {
                    let w = app.settings_view.borrow().as_ref().context("设置没有打开")?.clone_strong();
                    capture(w.window(), &out.join("settings.png"))?;
                    w.set_tab(1);
                    w.set_quote_index(4);
                    w.invoke_action("save".into());
                    ensure!(app.settings.borrow().quote_font_percent == 110, "字号未生效");
                    app.command("add");
                }
                4 => {
                    let w = app.search_view.borrow().as_ref().context("添加自选没有打开")?.clone_strong();
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
                    std::fs::write(out.join("result.txt"), "PASS quote replay, 550x564 geometry, depth and chart modes, settings save, explicit search, opacity and topmost, close-to-tray and restore")?;
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
