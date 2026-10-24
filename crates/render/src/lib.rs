//! Холст для рисования с масштабом (крупные экраны, HiDPI).
//!
//! Вся оболочка считает размеры в «точках»: полоса меню 28, значок дока 48…
//! На крупном экране точка — больше одного пикселя (масштаб 1,5 или 2).
//! [`Canvas`] принимает размеры и координаты в точках, а рисует сразу в
//! пикселях экрана — поэтому линии, буквы и значки остаются чёткими, а не
//! растянутой маленькой картинкой.
//!
//! Масштаб один на всю программу: задаётся при запуске ([`set_scale`]).

use std::sync::atomic::{AtomicU32, Ordering};

use tiny_skia::{
    Color, FillRule, Mask, Paint, Path, Pixmap, PixmapPaint, PixmapRef, Rect, Stroke, Transform,
};

/// Масштаб (биты `f32`); 1.0 — точка равна пикселю.
static SCALE: AtomicU32 = AtomicU32::new(0x3F80_0000);

/// Масштаб программы: сколько пикселей экрана в одной точке.
pub fn scale() -> f32 {
    f32::from_bits(SCALE.load(Ordering::Relaxed))
}

/// Задать масштаб (при запуске, до создания окон). Неверное значение — 1.
pub fn set_scale(s: f32) {
    let s = if s.is_finite() {
        s.clamp(1.0, 4.0)
    } else {
        1.0
    };
    SCALE.store(s.to_bits(), Ordering::Relaxed);
}

/// Точки → пиксели при масштабе `s` (не меньше 1 пикселя у ненулевого).
pub fn to_px(v: u32, s: f32) -> u32 {
    if v == 0 {
        0
    } else {
        ((v as f32 * s).round() as u32).max(1)
    }
}

/// Готовые пиксели для вывода в окно: картинка или холст.
pub trait Image {
    fn image(&self) -> &Pixmap;
}

impl Image for Pixmap {
    fn image(&self) -> &Pixmap {
        self
    }
}

impl Image for Canvas {
    fn image(&self) -> &Pixmap {
        &self.pixmap
    }
}

/// Картинка в точках: рисует в пикселях с масштабом.
pub struct Canvas {
    pixmap: Pixmap,
    scale: f32,
    width: u32,
    height: u32,
}

impl Canvas {
    /// Холст `w`×`h` точек при масштабе программы. `None` — нулевой размер.
    pub fn new(w: u32, h: u32) -> Option<Canvas> {
        Canvas::with_scale(w, h, scale())
    }

    /// Холст с заданным масштабом (для проверок и снимков).
    pub fn with_scale(w: u32, h: u32, scale: f32) -> Option<Canvas> {
        Some(Canvas {
            pixmap: Pixmap::new(to_px(w, scale), to_px(h, scale))?,
            scale,
            width: w,
            height: h,
        })
    }

    /// Холст поверх готовой картинки в пикселях (например, фона во весь
    /// экран); размер в точках — её размер, делённый на масштаб.
    pub fn from_pixmap(pixmap: Pixmap, scale: f32) -> Canvas {
        let points = |v: u32| ((v as f32 / scale).round() as u32).max(1);
        Canvas {
            width: points(pixmap.width()),
            height: points(pixmap.height()),
            pixmap,
            scale,
        }
    }

    /// Нарисовать значок `icon` (отрисованный в пикселях экрана, см.
    /// `shell_icons::Icons::get`) левым верхним углом в точке `x`, `y`.
    pub fn draw_icon(&mut self, icon: PixmapRef, x: f32, y: f32, paint: &PixmapPaint) {
        let k = 1.0 / self.scale;
        let t = Transform::from_scale(k, k).post_translate(x, y);
        self.draw_pixmap(0, 0, icon, paint, t, None);
    }

    /// Ширина в точках.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Высота в точках.
    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Преобразование из точек (с учётом `t`) в пиксели холста.
    pub fn transform(&self, t: Transform) -> Transform {
        t.post_scale(self.scale, self.scale)
    }

    pub fn fill(&mut self, color: Color) {
        self.pixmap.fill(color);
    }

    pub fn fill_path(
        &mut self,
        path: &Path,
        paint: &Paint,
        rule: FillRule,
        t: Transform,
        mask: Option<&Mask>,
    ) {
        let t = self.transform(t);
        self.pixmap.fill_path(path, paint, rule, t, mask);
    }

    pub fn stroke_path(
        &mut self,
        path: &Path,
        paint: &Paint,
        stroke: &Stroke,
        t: Transform,
        mask: Option<&Mask>,
    ) {
        let t = self.transform(t);
        self.pixmap.stroke_path(path, paint, stroke, t, mask);
    }

    pub fn fill_rect(&mut self, rect: Rect, paint: &Paint, t: Transform, mask: Option<&Mask>) {
        let t = self.transform(t);
        self.pixmap.fill_rect(rect, paint, t, mask);
    }

    /// Нарисовать картинку `src` в пикселях как есть (`x`, `y` и `t` — в
    /// точках). Значок, отрисованный крупнее (размер × масштаб), рисуйте с
    /// уменьшением `t` (размер / ширину картинки) — он ляжет пиксель в пиксель.
    pub fn draw_pixmap(
        &mut self,
        x: i32,
        y: i32,
        src: PixmapRef,
        paint: &PixmapPaint,
        t: Transform,
        mask: Option<&Mask>,
    ) {
        let t = self.transform(t);
        self.pixmap.draw_pixmap(x, y, src, paint, t, mask);
    }

    /// Нарисовать другой холст (того же масштаба) в точке `x`, `y` (в точках).
    pub fn draw_canvas(&mut self, x: f32, y: f32, src: &Canvas, paint: &PixmapPaint) {
        let k = 1.0 / src.scale;
        let t = Transform::from_scale(k, k).post_translate(x, y);
        self.draw_pixmap(0, 0, src.pixmap.as_ref(), paint, t, None);
    }

    /// Пиксели (для своих эффектов: размытие, тени, текст).
    pub fn pixmap(&self) -> &Pixmap {
        &self.pixmap
    }

    pub fn pixmap_mut(&mut self) -> &mut Pixmap {
        &mut self.pixmap
    }

    /// Пиксель по координатам **в пикселях** (для проверок).
    pub fn pixel(&self, x: u32, y: u32) -> Option<tiny_skia::PremultipliedColorU8> {
        self.pixmap.pixel(x, y)
    }

    /// Все пиксели (для проверок).
    pub fn pixels(&self) -> &[tiny_skia::PremultipliedColorU8] {
        self.pixmap.pixels()
    }

    /// Сохранить в PNG (в пикселях).
    pub fn save_png(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        self.pixmap.save_png(path).map_err(std::io::Error::other)
    }

    /// Готовая картинка в пикселях — вывести в окно или в файл.
    pub fn into_pixmap(self) -> Pixmap {
        self.pixmap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_in_points_pixels_scaled() {
        let mut c = Canvas::with_scale(10, 4, 2.0).unwrap();
        assert_eq!((c.width(), c.height()), (10, 4));
        assert_eq!((c.pixmap().width(), c.pixmap().height()), (20, 8));
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 0, 0, 255);
        // Квадрат 1×1 точку в (2,1) — 2×2 пикселя в (4,2).
        let r = Rect::from_xywh(2.0, 1.0, 1.0, 1.0).unwrap();
        c.fill_rect(r, &paint, Transform::identity(), None);
        let px = |x: u32, y: u32| c.pixmap().pixel(x, y).unwrap().alpha();
        assert_eq!((px(4, 2), px(5, 3), px(6, 2), px(3, 2)), (255, 255, 0, 0));
    }

    #[test]
    fn fractional_sizes_round() {
        assert_eq!(to_px(28, 1.5), 42);
        assert_eq!(to_px(1, 1.25), 1);
        assert_eq!(to_px(0, 2.0), 0);
        let c = Canvas::with_scale(15, 15, 1.25).unwrap();
        assert_eq!(c.pixmap().width(), 19);
    }
}
