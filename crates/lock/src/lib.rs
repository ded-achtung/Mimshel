//! Экран блокировки: фон для i3lock-color в стиле Sonoma — обои, внизу
//! круглый аватар, имя и овальное поле ввода «матовое стекло». Часы, дату и
//! кольцо ввода i3lock рисует поверх сам. Пароль проверяет i3lock (PAM);
//! здесь только картинка, без окон и файлов — её можно проверить в PNG.

mod layout;

pub use layout::{Layout, round};

use shell_text::{Fonts, Style};
use shell_widgets::shape::{circle, paint, rounded};
use tiny_skia::{FillRule, Mask, Pixmap, PixmapPaint, Stroke, Transform};

/// Что нарисовать.
pub struct Scene<'a> {
    /// Обои, уже растянутые на весь экран (`None` — тёмный фон).
    pub wallpaper: Option<Pixmap>,
    /// Аватар размером `avatar`×`avatar` (`None` — кружок с первой буквой имени).
    pub face: Option<Pixmap>,
    pub user: &'a str,
    /// Подсказка в поле, если её не рисует сам i3lock (он без заплатки).
    pub placeholder: Option<&'a str>,
}

/// Фон экрана блокировки размером `l.w`×`l.h`.
pub fn draw(l: &Layout, scene: Scene, fonts: &mut Fonts) -> Option<Pixmap> {
    let (w, h) = (u32::try_from(l.w).ok()?, u32::try_from(l.h).ok()?);
    let mut pm = match scene.wallpaper {
        Some(p) if p.width() == w && p.height() == h => p,
        _ => {
            let mut p = Pixmap::new(w, h)?;
            p.fill(tiny_skia::Color::from_rgba8(28, 28, 32, 255));
            p
        }
    };
    darken(&mut pm, 0.88);

    let s = l.s as f32;
    let (px, py, pw, ph) = (
        l.pill_x as f32,
        l.pill_y as f32,
        l.pill_w as f32,
        l.pill_h as f32,
    );
    // Матовое стекло: под полем размыто.
    let radius = round(14.0 * l.s).max(4) as usize;
    blur_region(&mut pm, l.pill_x, l.pill_y, l.pill_w, l.pill_h, radius);
    if let Some(pill) = rounded(px, py, pw, ph, ph / 2.0) {
        let id = Transform::identity();
        pm.fill_path(
            &pill,
            &paint([255, 255, 255, 56]),
            FillRule::Winding,
            id,
            None,
        );
        let stroke = Stroke {
            width: s.round().max(1.0),
            ..Stroke::default()
        };
        pm.stroke_path(&pill, &paint([255, 255, 255, 71]), &stroke, id, None);
    }

    let a = l.avatar as f32;
    let (ax, ay) = (((l.w - l.avatar) / 2) as f32, l.avatar_y as f32);
    draw_avatar(&mut pm, scene.face, scene.user, ax, ay, a, fonts);

    let name = Style {
        size: (17.0 * s).round(),
        color: [255, 255, 255, 255],
        weight: 600,
    };
    let nw = fonts.measure(scene.user, &name);
    fonts.draw(
        &mut pm,
        scene.user,
        &name,
        ((l.w as f32 - nw) / 2.0).round(),
        l.name_y as f32 + text_shift(&name),
        None,
    );
    if let Some(text) = scene.placeholder {
        let hint = Style {
            size: (13.5 * s).round(),
            color: [255, 255, 255, 158],
            weight: 400,
        };
        let y = py + (ph - hint.line_height()) / 2.0;
        fonts.draw(&mut pm, text, &hint, px + (14.0 * s).round(), y, None);
    }
    Some(pm)
}

/// ImageMagick ставит текст верхом букв на заданную строку, наш текст —
/// верхом строки высотой [`Style::line_height`]: сдвиг вверх на запас над
/// буквами, чтобы имя стояло там же, где у фона macos-look.
fn text_shift(style: &Style) -> f32 {
    -((style.line_height() - style.size) / 2.0).round()
}

fn draw_avatar(
    pm: &mut Pixmap,
    face: Option<Pixmap>,
    user: &str,
    x: f32,
    y: f32,
    a: f32,
    fonts: &mut Fonts,
) {
    let Some(round_shape) = circle(x + a / 2.0, y + a / 2.0, a / 2.0) else {
        return;
    };
    match face {
        Some(face) => {
            let Some(mut mask) = Mask::new(pm.width(), pm.height()) else {
                return;
            };
            mask.fill_path(&round_shape, FillRule::Winding, true, Transform::identity());
            pm.draw_pixmap(
                x as i32,
                y as i32,
                face.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                Some(&mask),
            );
        }
        None => {
            // Кружок с первой буквой имени, как у macOS без фотографии.
            pm.fill_path(
                &round_shape,
                &paint([0x8e, 0x8e, 0x93, 255]),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            let letter: String = user.chars().take(1).flat_map(char::to_uppercase).collect();
            let style = Style {
                size: (a * 0.45).round(),
                color: [255, 255, 255, 255],
                weight: 600,
            };
            let lw = fonts.measure(&letter, &style);
            fonts.draw(
                pm,
                &letter,
                &style,
                x + (a - lw) / 2.0,
                y + (a - style.line_height()) / 2.0,
                None,
            );
        }
    }
}

/// Затемнить: цвет умножается на `k` (как `-fill black -colorize 12%`).
fn darken(pm: &mut Pixmap, k: f32) {
    for px in pm.data_mut().as_chunks_mut::<4>().0 {
        for c in &mut px[..3] {
            *c = (f32::from(*c) * k).round() as u8;
        }
    }
}

/// Размыть прямоугольник: три прохода скользящего среднего по строкам и
/// столбцам (вместе почти как размытие по Гауссу). Берутся только точки
/// внутри прямоугольника, за краем повторяется крайняя — как `-region` у
/// ImageMagick.
fn blur_region(pm: &mut Pixmap, x: i64, y: i64, w: i64, h: i64, r: usize) {
    let (pw, ph) = (i64::from(pm.width()), i64::from(pm.height()));
    let (x0, y0) = (x.clamp(0, pw), y.clamp(0, ph));
    let (x1, y1) = ((x + w).clamp(0, pw), (y + h).clamp(0, ph));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let stride = pm.width() as usize * 4;
    let data = pm.data_mut();
    let mut buf: Vec<u8> = Vec::with_capacity(rw * rh * 4);
    for row in 0..rh {
        let start = (y0 as usize + row) * stride + x0 as usize * 4;
        buf.extend_from_slice(&data[start..start + rw * 4]);
    }
    let mut tmp = vec![0u8; buf.len()];
    for _ in 0..3 {
        box_pass(&buf, &mut tmp, rw, rh, 4, rw * 4, r);
        box_pass(&tmp, &mut buf, rh, rw, rw * 4, 4, r);
    }
    for row in 0..rh {
        let start = (y0 as usize + row) * stride + x0 as usize * 4;
        data[start..start + rw * 4].copy_from_slice(&buf[row * rw * 4..(row + 1) * rw * 4]);
    }
}

/// Один проход среднего по линиям: `len` точек в линии с шагом `step`,
/// `lines` линий с шагом `line_step`.
fn box_pass(
    src: &[u8],
    dst: &mut [u8],
    len: usize,
    lines: usize,
    step: usize,
    line_step: usize,
    r: usize,
) {
    let r = r as isize;
    let n = (2 * r + 1) as u32;
    for line in 0..lines {
        let base = line * line_step;
        for c in 0..4 {
            let at = |i: isize| -> u32 {
                let i = i.clamp(0, len as isize - 1) as usize;
                u32::from(src[base + i * step + c])
            };
            let mut sum: u32 = (-r..=r).map(at).sum();
            for i in 0..len as isize {
                dst[base + i as usize * step + c] = (sum / n) as u8;
                sum += at(i + r + 1);
                sum -= at(i - r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_has_pill_avatar_and_name() {
        let l = Layout::new(640, 400);
        let mut wall = Pixmap::new(640, 400).unwrap();
        wall.fill(tiny_skia::Color::from_rgba8(0, 100, 200, 255));
        let mut fonts = Fonts::new("Inter");
        let pm = draw(
            &l,
            Scene {
                wallpaper: Some(wall),
                face: None,
                user: "ded",
                placeholder: Some("Введите пароль"),
            },
            &mut fonts,
        )
        .unwrap();
        assert_eq!((pm.width(), pm.height()), (640, 400));
        // Обои затемнены на 12 %.
        let corner = pm.pixel(0, 0).unwrap();
        assert_eq!((corner.green(), corner.blue()), (88, 176));
        // Поле светлее обоев.
        let pill = pm
            .pixel((l.pill_x + 4) as u32, (l.pill_y + l.pill_h / 2) as u32)
            .unwrap();
        assert!(pill.red() > 40, "поле: {pill:?}");
        // Аватар — серый кружок, по краям квадрата — обои.
        let c = (l.avatar_y + 2) as u32;
        let mid = pm.pixel(320, (l.avatar_y + l.avatar / 2) as u32).unwrap();
        assert!(mid.red() > 120, "середина аватара: {mid:?}");
        let edge = pm.pixel(((640 - l.avatar) / 2) as u32, c).unwrap();
        assert_eq!(edge.red(), 0, "угол квадрата аватара — фон");
        // Имя — белые буквы под аватаром.
        let name_row = (l.name_y as u32)..(l.name_y as u32 + 14);
        let white = name_row
            .flat_map(|y| (300..340).map(move |x| (x, y)))
            .any(|(x, y)| pm.pixel(x, y).unwrap().red() > 200);
        assert!(white, "имя видно");
    }

    #[test]
    fn region_blur_stays_inside() {
        let mut pm = Pixmap::new(20, 10).unwrap();
        for (i, px) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let v = if i % 2 == 0 { 255 } else { 0 };
            px.copy_from_slice(&[v, v, v, 255]);
        }
        blur_region(&mut pm, 5, 2, 10, 6, 2);
        // Внутри полосы сгладились, снаружи — как были.
        let inside = pm.pixel(10, 5).unwrap().red();
        assert!((100..160).contains(&inside), "{inside}");
        assert!(matches!(pm.pixel(1, 1).unwrap().red(), 0 | 255));
    }
}
