//! Текст: шрифты, раскладка строки и рисование её на картинке (tiny-skia).
//! Если в основном шрифте нет нужного знака, cosmic-text сам возьмёт его из
//! другого установленного шрифта.

use cosmic_text::{
    Attrs, Buffer, Color, Ellipsize, EllipsizeHeightLimit, Family, FontSystem, Metrics, Shaping,
    SwashCache, Weight, Wrap,
};
use tiny_skia::Pixmap;

/// Как выглядит строка.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    /// Размер в пикселях.
    pub size: f32,
    /// Цвет RGBA (без предварительного умножения на прозрачность).
    pub color: [u8; 4],
    /// Насыщенность: 400 — обычный, 600 — полужирный.
    pub weight: u16,
}

impl Style {
    /// Высота строки: столько места занимает текст по вертикали.
    pub fn line_height(&self) -> f32 {
        (self.size * 1.25).ceil()
    }
}

pub struct Fonts {
    system: FontSystem,
    cache: SwashCache,
    family: String,
}

impl Fonts {
    /// Загружает шрифты системы; `family` — основной (например, «Inter»).
    pub fn new(family: &str) -> Fonts {
        Fonts {
            system: FontSystem::new(),
            cache: SwashCache::new(),
            family: family.to_string(),
        }
    }

    /// Ширина строки в пикселях.
    pub fn measure(&mut self, text: &str, style: &Style) -> f32 {
        let buffer = self.layout(text, style, None);
        buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    }

    /// Рисует одну строку. `x`, `y` — левый верхний угол строки высотой
    /// [`Style::line_height`]. Если задана `max_width`, лишнее заменяется на «…».
    pub fn draw(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        style: &Style,
        x: f32,
        y: f32,
        max_width: Option<f32>,
    ) {
        let mut buffer = self.layout(text, style, max_width);
        let [r, g, b, a] = style.color;
        let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
        let data = pixmap.data_mut();
        let (ox, oy) = (x.round() as i32, y.round() as i32);
        buffer.draw(
            &mut self.system,
            &mut self.cache,
            Color::rgba(r, g, b, a),
            |gx, gy, gw, gh, color| {
                // cosmic-text отдаёт в прозрачности только покрытие края буквы,
                // прозрачность цвета теряется — домножаем на неё сами (иначе
                // «приглушённый» текст выходит ярко-белым).
                let alpha = (u32::from(color.a()) * u32::from(a) / 255) as u8;
                let color = Color::rgba(color.r(), color.g(), color.b(), alpha);
                for py in oy + gy..oy + gy + gh as i32 {
                    for px in ox + gx..ox + gx + gw as i32 {
                        if px >= 0 && py >= 0 && px < w && py < h {
                            let i = ((py * w + px) * 4) as usize;
                            blend(&mut data[i..i + 4], color);
                        }
                    }
                }
            },
        );
    }

    fn layout(&mut self, text: &str, style: &Style, max_width: Option<f32>) -> Buffer {
        let metrics = Metrics::new(style.size, style.line_height());
        let mut buffer = Buffer::new(&mut self.system, metrics);
        buffer.set_wrap(Wrap::None);
        if max_width.is_some() {
            buffer.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));
        }
        buffer.set_size(max_width, Some(style.line_height()));
        let attrs = Attrs::new()
            .family(Family::Name(&self.family))
            .weight(Weight(style.weight));
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.system, false);
        buffer
    }
}

/// Накладывает цвет с прозрачностью на пиксель картинки (там цвета уже
/// умножены на прозрачность — так хранит tiny-skia).
fn blend(dst: &mut [u8], c: Color) {
    let a = c.a() as u32;
    if a == 0 {
        return;
    }
    let src = [c.r(), c.g(), c.b()].map(|v| v as u32 * a / 255);
    for i in 0..3 {
        dst[i] = (src[i] + dst[i] as u32 * (255 - a) / 255) as u8;
    }
    dst[3] = (a + dst[3] as u32 * (255 - a) / 255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLE: Style = Style {
        size: 17.0,
        color: [255, 255, 255, 255],
        weight: 400,
    };

    #[test]
    fn measure_and_draw() {
        let mut fonts = Fonts::new("Inter");
        let short = fonts.measure("Терминал", &STYLE);
        let long = fonts.measure("Терминал Терминал", &STYLE);
        assert!(short > 20.0 && long > short * 1.8, "{short} {long}");

        let mut pixmap = Pixmap::new(200, 30).unwrap();
        fonts.draw(&mut pixmap, "Терминал", &STYLE, 4.0, 4.0, None);
        let painted = pixmap.data().chunks(4).filter(|p| p[3] > 0).count();
        assert!(painted > 50, "нарисовано точек: {painted}");
    }

    #[test]
    fn ellipsis_keeps_within_width() {
        let mut fonts = Fonts::new("Inter");
        let mut pixmap = Pixmap::new(300, 30).unwrap();
        let text = "Очень длинное название программы, которое не влезает";
        fonts.draw(&mut pixmap, text, &STYLE, 0.0, 0.0, Some(100.0));
        let w = pixmap.width() as usize;
        let beyond = pixmap
            .data()
            .chunks(4)
            .enumerate()
            .filter(|(i, p)| i % w > 102 && p[3] > 0)
            .count();
        assert_eq!(beyond, 0);
    }

    #[test]
    fn color_alpha_dims_text() {
        let mut fonts = Fonts::new("Inter");
        let brightest = |fonts: &mut Fonts, alpha: u8| {
            let mut pm = Pixmap::new(80, 30).unwrap();
            pm.fill(tiny_skia::Color::BLACK);
            let style = Style {
                color: [255, 255, 255, alpha],
                ..STYLE
            };
            fonts.draw(&mut pm, "Пн 28", &style, 2.0, 2.0, None);
            pm.pixels().iter().map(|p| p.red()).max().unwrap()
        };
        assert!(brightest(&mut fonts, 255) > 240);
        let dim = brightest(&mut fonts, 70);
        assert!((50..=80).contains(&dim), "приглушённый текст: {dim}");
    }
}
