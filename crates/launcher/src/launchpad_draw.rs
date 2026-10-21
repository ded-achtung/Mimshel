//! Как выглядит Launchpad: размытый затемнённый рабочий стол, поле поиска,
//! сетка иконок с подписями, точки страниц. Размеры сетки — из настроек
//! (как в rofi-теме macos-look: 7×4, иконки 72 px).

use shell_icons::Icons;
use shell_text::{Fonts, Style};
use tiny_skia::{FillRule, IntSize, Pixmap, Transform};

use crate::Launchpad;
use crate::draw::{draw_icon, fill_rect, magnifier, paint, rounded};

/// Во сколько раз уменьшать снимок экрана перед размытием: и быстрее,
/// и само уменьшение — часть размытия.
const SHRINK: u32 = 8;
/// Радиус и число проходов размытия (в уменьшенной картинке).
const BLUR_RADIUS: usize = 3;
const BLUR_PASSES: usize = 3;
/// Затемнение поверх размытого стола.
const DIM: [u8; 4] = [0x00, 0x00, 0x00, 0x55];
/// Фон, если снимка экрана нет (проверки без экрана).
const PLAIN: [u8; 4] = [0x1c, 0x1c, 0x22, 0xff];

const SEARCH_BG: [u8; 4] = [0xff, 0xff, 0xff, 0x26];
const SEARCH_BORDER: [u8; 4] = [0xff, 0xff, 0xff, 0x1f];
const SELECTED: [u8; 4] = [0xff, 0xff, 0xff, 0x26];
const DOT_ON: [u8; 4] = [0xff, 0xff, 0xff, 0xe6];
const DOT_OFF: [u8; 4] = [0xff, 0xff, 0xff, 0x55];
const CURSOR: [u8; 4] = [0xf2, 0xf2, 0xf2, 0xd0];
const GLASS: [u8; 4] = [0xd0, 0xd0, 0xd0, 0xff];

const SEARCH_TEXT: Style = Style {
    size: 15.0,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 400,
};
const PLACEHOLDER: Style = Style {
    color: [0xc8, 0xc8, 0xc8, 0xff],
    ..SEARCH_TEXT
};
const LABEL: Style = Style {
    size: 15.0,
    color: [0xff, 0xff, 0xff, 0xff],
    weight: 400,
};
/// Тень под подписью, чтобы её было видно и на светлых обоях.
const LABEL_SHADOW: Style = Style {
    color: [0x00, 0x00, 0x00, 0x90],
    ..LABEL
};
const NOTHING: Style = Style {
    size: 22.0,
    color: [0xe0, 0xe0, 0xe0, 0xff],
    weight: 400,
};

/// Фон Launchpad из снимка экрана: уменьшить, размыть, растянуть обратно,
/// затемнить. Без снимка — ровный тёмный фон размера `size`.
pub fn backdrop(screen: Option<&Pixmap>, size: (u32, u32)) -> Pixmap {
    let size = (size.0.max(1), size.1.max(1));
    let Some(screen) = screen else {
        let mut out = Pixmap::new(size.0, size.1).expect("размер не нулевой");
        fill_rect(&mut out, 0.0, 0.0, size.0 as f32, size.1 as f32, PLAIN);
        return out;
    };
    let mut small = blur(&shrink(screen, SHRINK));
    // Затемняем маленькую картинку — в 64 раза меньше работы, чем на всём экране.
    let (w, h) = (small.width() as f32, small.height() as f32);
    fill_rect(&mut small, 0.0, 0.0, w, h, DIM);
    enlarge(&small, size)
}

/// Растягивание до `size` с плавными переходами (билинейно). Своё, а не общее
/// рисование tiny-skia: для всего экрана оно в несколько раз быстрее — доли
/// пикселя для каждого столбца считаются один раз, дальше только целые числа.
fn enlarge(src: &Pixmap, size: (u32, u32)) -> Pixmap {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (w, h) = (size.0 as usize, size.1 as usize);
    // Для точки результата: левый/верхний сосед в исходной картинке и вес
    // правого/нижнего соседа (0..=256).
    let map = |out: usize, len_out: usize, len_src: usize| -> Vec<(usize, u32)> {
        (0..out)
            .map(|i| {
                let f = ((i as f32 + 0.5) * len_src as f32 / len_out as f32 - 0.5).max(0.0);
                let i0 = (f as usize).min(len_src - 1);
                (i0, ((f - i0 as f32) * 256.0) as u32)
            })
            .collect()
    };
    let xs = map(w, w, sw);
    let ys = map(h, h, sh);
    let data = src.data();
    // Каждая исходная строка, растянутая по ширине (значения ×256), — считается
    // один раз: строк на экране 1413, а исходных всего ~177.
    let stretch = |r: usize| -> Vec<u32> {
        let base = r * sw * 4;
        let mut v = vec![0u32; w * 4];
        for (x, &(x0, fx)) in xs.iter().enumerate() {
            let x1 = (x0 + 1).min(sw - 1);
            for c in 0..4 {
                let a = u32::from(data[base + x0 * 4 + c]);
                let b = u32::from(data[base + x1 * 4 + c]);
                v[x * 4 + c] = a * (256 - fx) + b * fx;
            }
        }
        v
    };
    let mut rows: Vec<Option<Vec<u32>>> = vec![None; sh];
    let mut out = vec![0u8; w * h * 4];
    for (y, &(y0, fy)) in ys.iter().enumerate() {
        let y1 = (y0 + 1).min(sh - 1);
        for r in [y0, y1] {
            if rows[r].is_none() {
                rows[r] = Some(stretch(r));
            }
        }
        let (Some(top), Some(bottom)) = (rows[y0].as_deref(), rows[y1].as_deref()) else {
            continue;
        };
        // Строка экрана — смесь двух готовых строк; такой цикл процессор считает пачками.
        let row = &mut out[y * w * 4..(y + 1) * w * 4];
        for ((o, &t), &b) in row.iter_mut().zip(top).zip(bottom) {
            *o = ((t * (256 - fy) + b * fy) >> 16) as u8;
        }
        // Строки выше уже не понадобятся — освобождаем память.
        if y0 > 0 {
            rows[y0 - 1] = None;
        }
    }
    Pixmap::from_vec(
        out,
        IntSize::from_wh(size.0, size.1).expect("размер не нулевой"),
    )
    .expect("размер данных совпадает")
}

/// Уменьшение в `k` раз усреднением квадратов k×k.
fn shrink(src: &Pixmap, k: u32) -> Pixmap {
    let (w, h) = (src.width(), src.height());
    let (sw, sh) = (w.div_ceil(k), h.div_ceil(k));
    let data = src.data();
    let mut out = vec![0u8; (sw * sh * 4) as usize];
    for y in 0..sh {
        for x in 0..sw {
            let mut sum = [0u32; 4];
            let mut n = 0;
            for yy in y * k..((y + 1) * k).min(h) {
                let row = (yy * w) as usize * 4;
                for xx in x * k..((x + 1) * k).min(w) {
                    let i = row + xx as usize * 4;
                    for c in 0..4 {
                        sum[c] += u32::from(data[i + c]);
                    }
                    n += 1;
                }
            }
            let o = ((y * sw + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (sum[c] / n.max(1)) as u8;
            }
        }
    }
    Pixmap::from_vec(out, IntSize::from_wh(sw, sh).expect("размер не нулевой"))
        .expect("размер данных совпадает")
}

/// Размытие: несколько проходов скользящего среднего по строкам и столбцам
/// (вместе это почти как размытие по Гауссу).
fn blur(src: &Pixmap) -> Pixmap {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let mut a = src.data().to_vec();
    let mut b = vec![0u8; a.len()];
    for _ in 0..BLUR_PASSES {
        box_pass(&a, &mut b, w, h, 4, w * 4);
        box_pass(&b, &mut a, h, w, w * 4, 4);
    }
    Pixmap::from_vec(a, IntSize::from_wh(w as u32, h as u32).expect("не нулевой"))
        .expect("размер данных совпадает")
}

/// Один проход среднего по линиям: `len` точек в линии с шагом `step`,
/// `lines` линий с шагом `line_step` (так один код идёт и по строкам, и по столбцам).
fn box_pass(src: &[u8], dst: &mut [u8], len: usize, lines: usize, step: usize, line_step: usize) {
    let r = BLUR_RADIUS as isize;
    for line in 0..lines {
        let base = line * line_step;
        for c in 0..4 {
            let at = |i: isize| -> u32 {
                let i = i.clamp(0, len as isize - 1) as usize;
                u32::from(src[base + i * step + c])
            };
            let mut sum: u32 = (-r..=r).map(at).sum();
            let n = (2 * r + 1) as u32;
            for i in 0..len as isize {
                dst[base + i as usize * step + c] = (sum / n) as u8;
                sum += at(i + r + 1);
                sum -= at(i - r);
            }
        }
    }
}

impl Launchpad {
    /// Рисует экран Launchpad поверх готового фона (см. [`backdrop`]).
    pub fn render(&self, background: &Pixmap, fonts: &mut Fonts, icons: &mut Icons) -> Pixmap {
        let mut pm = background.clone();
        self.draw_search(&mut pm, fonts);

        let mut any = false;
        for (slot, (i, app)) in self.page_apps().enumerate() {
            any = true;
            let (x, y, w, _) = self.cell_box(slot);
            if self.selected == Some(i)
                && let Some(r) = rounded(x, y, w, self.grid.icon as f32 + 52.0, 14.0)
            {
                pm.fill_path(
                    &r,
                    &paint(SELECTED),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            let icon = self.grid.icon;
            let ix = x + (w - icon as f32) / 2.0;
            let iy = y + 12.0;
            if let Some(p) = app.icon.as_deref().and_then(|n| icons.get(n, icon)) {
                draw_icon(&mut pm, p, ix.round() as i32, iy.round() as i32);
            }
            // Подпись по центру под иконкой; длинная — с многоточием.
            let max = w - 12.0;
            let tw = fonts.measure(&app.name, &LABEL).min(max);
            let tx = x + (w - tw) / 2.0;
            let ty = iy + icon as f32 + 8.0;
            fonts.draw(&mut pm, &app.name, &LABEL_SHADOW, tx, ty + 1.0, Some(max));
            fonts.draw(&mut pm, &app.name, &LABEL, tx, ty, Some(max));
        }
        if !any && !self.field.is_empty() {
            let text = "Ничего не найдено";
            let tw = fonts.measure(text, &NOTHING);
            let y = self.layout.h * 0.4;
            fonts.draw(&mut pm, text, &NOTHING, (self.layout.w - tw) / 2.0, y, None);
        }

        if self.pages() > 1 {
            for p in 0..self.pages() {
                let (cx, cy) = self.dot_center(p);
                let color = if p == self.page { DOT_ON } else { DOT_OFF };
                if let Some(dot) = rounded(cx - 4.0, cy - 4.0, 8.0, 8.0, 4.0) {
                    pm.fill_path(
                        &dot,
                        &paint(color),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
        pm
    }

    /// Поле поиска: пока пусто — лупа и «Поиск» по центру, как на Mac;
    /// при наборе — текст от левого края и курсор.
    fn draw_search(&self, pm: &mut Pixmap, fonts: &mut Fonts) {
        let (x, y, w, h) = self.layout.search;
        if let Some(r) = rounded(x, y, w, h, 8.0) {
            pm.fill_path(
                &r,
                &paint(SEARCH_BG),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        if let Some(r) = rounded(x + 0.5, y + 0.5, w - 1.0, h - 1.0, 7.5) {
            let stroke = tiny_skia::Stroke {
                width: 1.0,
                ..tiny_skia::Stroke::default()
            };
            pm.stroke_path(
                &r,
                &paint(SEARCH_BORDER),
                &stroke,
                Transform::identity(),
                None,
            );
        }
        let glass = 14.0;
        let ty = y + (h - SEARCH_TEXT.line_height()) / 2.0;
        let gy = y + (h - glass) / 2.0;
        if self.field.is_empty() {
            let label = "Поиск";
            let tw = fonts.measure(label, &PLACEHOLDER);
            let gx = x + (w - (glass + 6.0 + tw)) / 2.0;
            magnifier(pm, gx, gy, glass, GLASS);
            fonts.draw(pm, label, &PLACEHOLDER, gx + glass + 6.0, ty, None);
        } else {
            let gx = x + 10.0;
            magnifier(pm, gx, gy, glass, GLASS);
            let tx = gx + glass + 6.0;
            let max = x + w - 10.0 - tx;
            fonts.draw(pm, self.field.text(), &SEARCH_TEXT, tx, ty, Some(max));
            let cx =
                (tx + fonts.measure(self.field.before_cursor(), &SEARCH_TEXT)).min(x + w - 10.0);
            fill_rect(pm, cx.round(), (y + 7.0).round(), 1.5, h - 14.0, CURSOR);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launchpad::Grid;
    use crate::spotlight::tests::app;
    use shell_core::input::{Key, KeyPress};

    #[test]
    fn blur_keeps_size_and_smooths() {
        // Чёрно-белые полосы по 1 точке после размытия становятся серыми.
        let mut src = Pixmap::new(64, 32).unwrap();
        for x in (0..64).step_by(2) {
            fill_rect(&mut src, x as f32, 0.0, 1.0, 32.0, [255, 255, 255, 255]);
        }
        let bg = backdrop(Some(&src), (64, 32));
        assert_eq!((bg.width(), bg.height()), (64, 32));
        let p = bg.pixel(30, 16).unwrap();
        // Серый, без полос, и чуть затемнён.
        let p2 = bg.pixel(31, 16).unwrap();
        assert!(p.red().abs_diff(p2.red()) < 8);
        assert!(p.red() > 40 && p.red() < 128, "{}", p.red());
    }

    #[test]
    fn enlarge_is_smooth() {
        // Две точки: чёрная и белая → плавный переход между ними.
        let mut src = Pixmap::new(2, 1).unwrap();
        fill_rect(&mut src, 1.0, 0.0, 1.0, 1.0, [255, 255, 255, 255]);
        let big = enlarge(&src, (8, 1));
        let reds: Vec<u8> = (0..8).map(|x| big.pixel(x, 0).unwrap().red()).collect();
        assert_eq!((reds[0], reds[7]), (0, 255));
        assert!(reds.windows(2).all(|w| w[0] <= w[1]), "{reds:?}");
        assert!(reds[3] > 60 && reds[4] < 200, "{reds:?}");
    }

    #[test]
    fn renders_page() {
        let mut fonts = Fonts::new("Inter");
        let mut icons = Icons::with_dirs("none", vec![], vec![]);
        let grid = Grid {
            columns: 3,
            rows: 2,
            icon: 72,
        };
        let apps = ["А", "Б", "В", "Г", "Д", "Е", "Ж"].map(app).to_vec();
        let mut l = Launchpad::new(apps, (900, 600), grid);
        let bg = backdrop(None, l.size());
        let pm = l.render(&bg, &mut fonts, &mut icons);
        assert_eq!((pm.width(), pm.height()), (900, 600));
        // Две страницы — есть точки: текущая яркая.
        let (cx, cy) = l.dot_center(0);
        assert!(pm.pixel(cx as u32, cy as u32).unwrap().red() > 200);

        l.key(KeyPress::plain(Key::Right));
        let pm = l.render(&bg, &mut fonts, &mut icons);
        let (x, y, _, _) = l.cell_box(0);
        // Подсветка выбранной иконки светлее фона.
        let lit = pm.pixel(x as u32 + 3, y as u32 + 20).unwrap().red();
        assert!(lit > PLAIN[0] + 20, "{lit}");
    }
}

#[cfg(test)]
mod timing {
    use super::*;
    use std::time::Instant;

    /// Замер частей размытия на экране 2560×1413:
    /// `cargo test --release -p shell-launcher timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn backdrop_parts() {
        let screen = Pixmap::new(2560, 1413).unwrap();
        let t = Instant::now();
        let small = shrink(&screen, SHRINK);
        let t1 = t.elapsed();
        let blurred = blur(&small);
        let t2 = t.elapsed();
        let _big = enlarge(&blurred, (2560, 1413));
        let t3 = t.elapsed();
        eprintln!(
            "уменьшение {t1:?}, размытие {:?}, растягивание {:?}",
            t2 - t1,
            t3 - t2
        );
    }
}
