//! Общее для окон «Поиск» и «Программы»: кисть, лупа, прямоугольники, значки.

use mimshel_render::Canvas;
use tiny_skia::{Color, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform};

pub(crate) fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c[0], c[1], c[2], c[3]));
    p.anti_alias = true;
    p
}

/// Серая лупа в квадрате `size` с углом в `x`, `y`.
pub(crate) fn magnifier(pm: &mut Canvas, x: f32, y: f32, size: f32, color: [u8; 4]) {
    let r = size * 0.3;
    let (cx, cy) = (x + size * 0.42, y + size * 0.42);
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r);
    // Ручка — по диагонали вниз-вправо от края стекла.
    let d = r * std::f32::consts::FRAC_1_SQRT_2;
    pb.move_to(cx + d, cy + d);
    pb.line_to(x + size * 0.9, y + size * 0.9);
    if let Some(path) = pb.finish() {
        let stroke = Stroke {
            width: (size / 10.0).max(1.4),
            line_cap: tiny_skia::LineCap::Round,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint(color), &stroke, Transform::identity(), None);
    }
}

pub(crate) fn fill_rect(pm: &mut Canvas, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &paint(c), Transform::identity(), None);
    }
}

/// То же на картинке в пикселях (фон окна «Программы»).
pub(crate) fn fill_pixels(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &paint(c), Transform::identity(), None);
    }
}

pub(crate) fn draw_icon(pm: &mut Canvas, icon: &Pixmap, x: i32, y: i32) {
    pm.draw_icon(icon.as_ref(), x as f32, y as f32, &PixmapPaint::default());
}

/// Прямоугольник со скруглёнными углами.
pub(crate) fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    // Доля радиуса для кривой Безье, приближающей четверть окружности.
    let k = r * 0.552_284_8;
    let (x1, y1) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}
