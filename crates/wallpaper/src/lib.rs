//! Свои обои, нарисованные программой под любой размер экрана: «Волны»,
//! «Космос», «Сияние». Ничего чужого — только градиенты, волны и точки.
//!
//! Мягкость даёт размытие: крупные формы рисуются в малом размере, размываются
//! и растягиваются на весь экран. Чтобы на плавных переходах не было полос
//! (в 8 битах на цвет их видно), в конце добавляется едва заметный шум.

use tiny_skia::{
    Color, FillRule, FilterQuality, GradientStop, IntSize, LinearGradient, Paint, PathBuilder,
    Pixmap, PixmapPaint, Point, RadialGradient, Shader, SpreadMode, Transform,
};

/// Какие обои.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Waves,
    Space,
    Aurora,
}

impl Style {
    pub const ALL: [Style; 3] = [Style::Waves, Style::Space, Style::Aurora];

    /// Имя в настройках.
    pub fn name(self) -> &'static str {
        match self {
            Style::Waves => "waves",
            Style::Space => "space",
            Style::Aurora => "aurora",
        }
    }

    pub fn parse(s: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|st| st.name() == s)
    }
}

/// Во сколько раз меньше рисуются мягкие формы перед размытием.
const SMALL: u32 = 4;

/// Обои размером `w`×`h`.
pub fn draw(style: Style, w: u32, h: u32) -> Option<Pixmap> {
    let (sw, sh) = ((w / SMALL).max(8), (h / SMALL).max(8));
    let mut soft = Pixmap::new(sw, sh)?;
    let mut rng = Rng(0x0005_eed5_4e11 ^ u64::from(w) << 20 ^ u64::from(h));
    match style {
        Style::Waves => waves_sky(&mut soft),
        Style::Space => nebula(&mut soft, &mut rng),
        Style::Aurora => aurora(&mut soft),
    }
    blur(&mut soft, (sw / 40).max(2) as usize);
    let mut pm = Pixmap::new(w, h)?;
    let paint = PixmapPaint {
        quality: FilterQuality::Bicubic,
        ..PixmapPaint::default()
    };
    let t = Transform::from_scale(w as f32 / sw as f32, h as f32 / sh as f32);
    pm.draw_pixmap(0, 0, soft.as_ref(), &paint, t, None);
    // Чёткое поверх размытого фона: волны и звёзды.
    match style {
        Style::Waves => waves(&mut pm),
        Style::Space => stars(&mut pm, &mut rng),
        Style::Aurora => {}
    }
    dither(&mut pm, &mut rng);
    Some(pm)
}

fn rgba(c: [u8; 4]) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], c[3])
}

fn fill_all(pm: &mut Pixmap, shader: Shader) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let rect = tiny_skia::Rect::from_xywh(0.0, 0.0, w, h).expect("не пустой");
    let paint = Paint {
        shader,
        anti_alias: true,
        ..Paint::default()
    };
    pm.fill_rect(rect, &paint, Transform::identity(), None);
}

/// Вертикальный переход сверху вниз через цвета `stops` (доля высоты, цвет).
fn vertical(pm: &mut Pixmap, stops: &[(f32, [u8; 4])]) {
    let h = pm.height() as f32;
    let stops = stops
        .iter()
        .map(|&(p, c)| GradientStop::new(p, rgba(c)))
        .collect();
    if let Some(s) = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(0.0, h),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        fill_all(pm, s);
    }
}

/// Мягкое цветное пятно: от `c` в центре до прозрачного на краю.
fn glow(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, c: [u8; 4]) {
    let clear = [c[0], c[1], c[2], 0];
    if let Some(s) = RadialGradient::new(
        Point::from_xy(cx, cy),
        0.0,
        Point::from_xy(cx, cy),
        r,
        vec![
            GradientStop::new(0.0, rgba(c)),
            GradientStop::new(1.0, rgba(clear)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        fill_all(pm, s);
    }
}

/// Слой волн: (высота, размах, частота, сдвиг, цвет сверху, цвет снизу).
type Layer = (f32, f32, f32, f32, [u8; 4], [u8; 4]);

/// Небо за волнами: переход и мягкое свечение (рисуется размытым).
fn waves_sky(pm: &mut Pixmap) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    vertical(
        pm,
        &[
            (0.0, [0x14, 0x1a, 0x3a, 255]),
            (0.55, [0x23, 0x2c, 0x6e, 255]),
            (1.0, [0x10, 0x14, 0x2c, 255]),
        ],
    );
    glow(pm, w * 0.78, h * 0.18, w * 0.45, [0x6a, 0x5c, 0xd8, 150]);
}

/// «Волны»: слои с волнистым верхним краем, каждый — своим цветом; рисуются
/// чётко, на полном размере.
fn waves(pm: &mut Pixmap) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let layers: [Layer; 5] = [
        (
            0.38,
            0.10,
            1.1,
            0.2,
            [0x3a, 0x4f, 0xc8, 200],
            [0x1c, 0x24, 0x6a, 230],
        ),
        (
            0.50,
            0.08,
            1.6,
            1.7,
            [0x6b, 0x4c, 0xd6, 190],
            [0x2a, 0x1f, 0x6e, 230],
        ),
        (
            0.60,
            0.07,
            1.3,
            3.1,
            [0x2f, 0x86, 0xd8, 200],
            [0x16, 0x34, 0x7a, 235],
        ),
        (
            0.72,
            0.06,
            2.0,
            4.4,
            [0x8a, 0x5c, 0xe0, 190],
            [0x30, 0x22, 0x70, 240],
        ),
        (
            0.84,
            0.05,
            1.5,
            5.9,
            [0x24, 0x5e, 0xd0, 210],
            [0x10, 0x1a, 0x48, 245],
        ),
    ];
    for (base, amp, freq, phase, top, bottom) in layers {
        let mut pb = PathBuilder::new();
        let steps = 256;
        let y_at = |i: usize| {
            let x = i as f32 / steps as f32;
            let t = x * std::f32::consts::TAU * freq + phase;
            h * (base + amp * (t.sin() * 0.7 + (t * 2.3 + phase).sin() * 0.3))
        };
        pb.move_to(0.0, y_at(0));
        for i in 1..=steps {
            pb.line_to(w * i as f32 / steps as f32, y_at(i));
        }
        pb.line_to(w, h);
        pb.line_to(0.0, h);
        pb.close();
        let Some(path) = pb.finish() else { continue };
        let top_y = h * (base - amp);
        let shader = LinearGradient::new(
            Point::from_xy(0.0, top_y),
            Point::from_xy(0.0, h),
            vec![
                GradientStop::new(0.0, rgba(top)),
                GradientStop::new(1.0, rgba(bottom)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        );
        if let Some(shader) = shader {
            let paint = Paint {
                shader,
                anti_alias: true,
                ..Paint::default()
            };
            pm.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// «Космос»: тёмно-синее небо с мягкими туманностями.
fn nebula(pm: &mut Pixmap, rng: &mut Rng) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    vertical(
        pm,
        &[
            (0.0, [0x05, 0x07, 0x16, 255]),
            (0.6, [0x0b, 0x10, 0x2e, 255]),
            (1.0, [0x12, 0x0b, 0x2a, 255]),
        ],
    );
    let clouds: [(f32, f32, f32, [u8; 4]); 5] = [
        (0.28, 0.35, 0.42, [0x3b, 0x2a, 0xa8, 170]),
        (0.70, 0.62, 0.50, [0x8a, 0x2f, 0x9e, 130]),
        (0.55, 0.28, 0.30, [0x1f, 0x6f, 0xc6, 140]),
        (0.18, 0.80, 0.35, [0x1b, 0x3f, 0x9a, 120]),
        (0.86, 0.20, 0.28, [0xc0, 0x4a, 0x8a, 90]),
    ];
    for (x, y, r, c) in clouds {
        // Небольшой разброс, чтобы туманность не была из правильных кругов.
        let jx = (rng.next_f32() - 0.5) * 0.05;
        let jy = (rng.next_f32() - 0.5) * 0.05;
        glow(pm, w * (x + jx), h * (y + jy), w * r, c);
    }
}

/// Звёзды: мелкие точки разной яркости, изредка — покрупнее с ореолом.
fn stars(pm: &mut Pixmap, rng: &mut Rng) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let count = (w * h / 2600.0) as usize;
    for _ in 0..count {
        let (x, y) = (rng.next_f32() * w, rng.next_f32() * h);
        let bright = rng.next_f32();
        let a = (40.0 + bright * bright * 215.0) as u8;
        let r = if bright > 0.97 {
            1.6
        } else {
            0.7 + bright * 0.5
        };
        if bright > 0.985 {
            glow(pm, x, y, 7.0, [0xb8, 0xcc, 0xff, 70]);
        }
        let mut pb = PathBuilder::new();
        pb.push_circle(x, y, r);
        if let Some(p) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(rgba([0xee, 0xf2, 0xff, a]));
            paint.anti_alias = true;
            pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }
}

/// «Сияние»: крупные мягкие пятна, перетекающие друг в друга.
fn aurora(pm: &mut Pixmap) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    vertical(
        pm,
        &[
            (0.0, [0x0e, 0x12, 0x2a, 255]),
            (1.0, [0x18, 0x10, 0x30, 255]),
        ],
    );
    let blobs: [(f32, f32, f32, [u8; 4]); 6] = [
        (0.15, 0.25, 0.55, [0x2c, 0x6c, 0xf0, 230]),
        (0.80, 0.15, 0.50, [0x7a, 0x4c, 0xe8, 220]),
        (0.55, 0.75, 0.60, [0x1a, 0xb0, 0xc8, 200]),
        (0.10, 0.95, 0.45, [0x4a, 0x2a, 0xb0, 210]),
        (0.95, 0.85, 0.45, [0xd0, 0x4c, 0x9c, 170]),
        (0.45, 0.35, 0.35, [0x5a, 0x8a, 0xff, 150]),
    ];
    for (x, y, r, c) in blobs {
        glow(pm, w * x, h * y, w * r, c);
    }
}

/// Размыть всю картинку: три прохода скользящего среднего по строкам и столбцам.
fn blur(pm: &mut Pixmap, r: usize) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let mut a = pm.data().to_vec();
    let mut b = vec![0u8; a.len()];
    for _ in 0..3 {
        box_pass(&a, &mut b, w, h, 4, w * 4, r);
        box_pass(&b, &mut a, h, w, w * 4, 4, r);
    }
    if let Some(out) =
        Pixmap::from_vec(a, IntSize::from_wh(w as u32, h as u32).expect("не нулевой"))
    {
        *pm = out;
    }
}

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

/// Едва заметный шум ±2 — против полос на плавных переходах.
fn dither(pm: &mut Pixmap, rng: &mut Rng) {
    for px in pm.data_mut().as_chunks_mut::<4>().0 {
        let d = (rng.next_u64() % 5) as i16 - 2;
        let alpha = i16::from(px[3]);
        for c in &mut px[..3] {
            *c = (i16::from(*c) + d).clamp(0, alpha) as u8;
        }
    }
}

/// Простой генератор случайных чисел (xorshift): одни и те же обои при
/// одном и том же размере экрана.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_style_fills_the_screen() {
        for style in Style::ALL {
            let pm = draw(style, 320, 200).unwrap();
            assert_eq!((pm.width(), pm.height()), (320, 200));
            // Непрозрачно везде и не одноцветно.
            assert!(pm.pixels().iter().all(|p| p.alpha() == 255), "{style:?}");
            let first = pm.pixel(0, 0).unwrap();
            assert!(pm.pixels().iter().any(|p| *p != first), "{style:?}");
            // Одинаковые при одинаковом размере.
            assert_eq!(draw(style, 320, 200).unwrap().data(), pm.data());
            assert_eq!(Style::parse(style.name()), Some(style));
        }
    }
}
