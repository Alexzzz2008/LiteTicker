use liteticker_native::model::MarketDetails;
use rust_decimal::{Decimal, prelude::ToPrimitive};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, Transform};

const WIDTH: u32 = 900;
const HEIGHT: u32 = 220;

fn tick_fraction(index: usize) -> f32 {
    index as f32 / 4.
}

fn stroke(
    pix: &mut Pixmap,
    points: impl IntoIterator<Item = (f32, f32)>,
    rgb: [u8; 3],
    width: f32,
) {
    let mut points = points.into_iter();
    let Some((x, y)) = points.next() else { return };
    let mut path = PathBuilder::new();
    path.move_to(x, y);
    for (x, y) in points {
        path.line_to(x, y);
    }
    if let Some(path) = path.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(rgb[0], rgb[1], rgb[2], 255);
        pix.stroke_path(
            &path,
            &paint,
            &Stroke {
                width,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }
}
fn minute_index(time: &str) -> Option<u16> {
    let (h, m) = time.split_once(':')?;
    let minute = h
        .parse::<u16>()
        .ok()?
        .checked_mul(60)?
        .checked_add(m.parse::<u16>().ok()?)?;
    match minute {
        570..=690 => Some(minute - 570),
        780..=900 => Some(minute - 660),
        _ => None,
    }
}
pub fn render(
    data: &MarketDetails,
    daily: bool,
    reference: Decimal,
    price: Decimal,
) -> (Image, Vec<slint::SharedString>) {
    let mut pix = Pixmap::new(WIDTH, HEIGHT).unwrap();
    pix.fill(Color::from_rgba8(25, 24, 29, 255));
    let coords: Vec<_> = if daily {
        data.daily
            .iter()
            .flat_map(|b| [b.high, b.low])
            .filter(|v| *v > Decimal::ZERO)
            .collect()
    } else {
        data.minutes
            .iter()
            .flat_map(|m| [m.price, m.average])
            .chain([reference])
            .filter(|v| *v > Decimal::ZERO)
            .collect()
    };
    let mut lo = coords.iter().min().and_then(|v| v.to_f64()).unwrap_or(0.);
    let mut hi = coords.iter().max().and_then(|v| v.to_f64()).unwrap_or(1.);
    if !daily && reference > Decimal::ZERO {
        let mid = reference.to_f64().unwrap_or_default();
        let span = (hi - mid).abs().max((lo - mid).abs());
        hi = mid + span;
        lo = mid - span;
    }
    let padding = ((hi - lo) * 0.08).max(0.005).max(hi.abs() * 0.001);
    hi += padding;
    lo -= padding;
    let precision = if [reference, price]
        .into_iter()
        .chain(coords.iter().copied())
        .any(|n| n.round_dp(2) != n)
    {
        3
    } else {
        2
    };
    let labels = (0..5)
        .map(|i| format!("{:.*}", precision, hi - (hi - lo) * f64::from(i) / 4.).into())
        .collect();
    let y = |v: Decimal| {
        if hi > lo {
            (f64::from(HEIGHT) * (hi - v.to_f64().unwrap_or(lo)) / (hi - lo)) as f32
        } else {
            HEIGHT as f32 / 2.
        }
    };
    for i in 0..=4 {
        let x = WIDTH as f32 * tick_fraction(i);
        if !daily {
            stroke(&mut pix, [(x, 0.), (x, HEIGHT as f32)], [56, 54, 62], 1.);
        }
        let y = HEIGHT as f32 * tick_fraction(i);
        stroke(&mut pix, [(0., y), (WIDTH as f32, y)], [56, 54, 62], 1.);
    }
    if !daily && reference > Decimal::ZERO {
        let line = y(reference);
        stroke(
            &mut pix,
            [(0., line), (WIDTH as f32, line)],
            [113, 103, 88],
            1.,
        );
    }
    if daily {
        let step = WIDTH as f32 / data.daily.len().max(1) as f32;
        for (i, b) in data.daily.iter().enumerate() {
            let x = step * (i as f32 + 0.5);
            let rgb = if b.close >= b.open {
                [231, 54, 62]
            } else {
                [42, 167, 98]
            };
            stroke(&mut pix, [(x, y(b.low)), (x, y(b.high))], rgb, 1.);
            let top = y(b.open);
            let bottom = y(b.close);
            stroke(
                &mut pix,
                [
                    (x, top),
                    (
                        x,
                        if (top - bottom).abs() < 1. {
                            top + 1.
                        } else {
                            bottom
                        },
                    ),
                ],
                rgb,
                (step * 0.65).max(1.),
            );
        }
    } else {
        for (average, color) in [(false, [220, 220, 222]), (true, [220, 178, 78])] {
            let points = data.minutes.iter().filter_map(|m| {
                let v = if average { m.average } else { m.price };
                if v <= Decimal::ZERO {
                    return None;
                }
                Some((
                    WIDTH as f32 * f32::from(minute_index(&m.time)?) / 240.,
                    y(v),
                ))
            });
            stroke(&mut pix, points, color, 1.8);
        }
    }
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(pix.width(), pix.height());
    buffer.make_mut_bytes().copy_from_slice(pix.data());
    (Image::from_rgba8_premultiplied(buffer), labels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_grid_and_price_labels_share_all_five_plot_positions() {
        let (image, labels) = render(
            &MarketDetails::default(),
            true,
            Decimal::ZERO,
            Decimal::ZERO,
        );
        assert_eq!(labels.len(), 5);
        let rgba = image.to_rgba8().unwrap();
        for i in 0..5 {
            let source_y = HEIGHT as f32 * tick_fraction(i);
            let row = (source_y as usize).min(HEIGHT as usize - 1);
            let offset = (row * WIDTH as usize + 90) * 4;
            assert_ne!(
                &rgba.as_bytes()[offset..offset + 3],
                &[25, 24, 29],
                "missing grid tick {i}"
            );
            for scale in [0.8, 1., 1.2] {
                for dpi in [1., 1.5, 2.] {
                    for height in [60., 220., 317.] {
                        let label_height = 26. * scale;
                        let plot_top = label_height / 2.;
                        let plot_height = height * scale - label_height;
                        let line = plot_top + source_y / HEIGHT as f32 * plot_height;
                        let label_top = plot_top + plot_height * i as f32 / 4. - label_height / 2.;
                        assert!(((label_top + label_height / 2. - line) * dpi).abs() < 0.001);
                    }
                }
            }
        }
    }
}
