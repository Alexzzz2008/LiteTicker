use liteticker_native::model::MarketDetails;
use rust_decimal::{Decimal, prelude::ToPrimitive};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, Transform};

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
pub fn render(data: &MarketDetails, daily: bool, reference: Decimal) -> Image {
    let mut pix = Pixmap::new(900, 220).unwrap();
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
    let lo = coords.iter().min().and_then(|v| v.to_f64()).unwrap_or(0.);
    let hi = coords.iter().max().and_then(|v| v.to_f64()).unwrap_or(1.);
    let y = |v: Decimal| {
        if hi > lo {
            (210.0 - 200.0 * (v.to_f64().unwrap_or(lo) - lo) / (hi - lo)) as f32
        } else {
            110.
        }
    };
    for i in 0..=4 {
        let x = 2. + i as f32 * 224.;
        stroke(&mut pix, [(x, 2.), (x, 218.)], [56, 54, 62], 1.);
        let y = 2. + i as f32 * 54.;
        stroke(&mut pix, [(2., y), (898., y)], [56, 54, 62], 1.);
    }
    if daily {
        let step = 896. / data.daily.len().max(1) as f32;
        for (i, b) in data.daily.iter().enumerate() {
            let x = 2. + step * (i as f32 + 0.5);
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
                Some((2. + 896. * f32::from(minute_index(&m.time)?) / 240., y(v)))
            });
            stroke(&mut pix, points, color, 1.8);
        }
    }
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(pix.width(), pix.height());
    buffer.make_mut_bytes().copy_from_slice(pix.data());
    Image::from_rgba8_premultiplied(buffer)
}
