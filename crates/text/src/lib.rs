//! Текст: шрифты, раскладка строки и рисование её на картинке (tiny-skia).
//! Если в основном шрифте нет нужного знака, cosmic-text сам возьмёт его из
//! другого установленного шрифта.

use cosmic_text::{
    Attrs, Buffer, Color, Ellipsize, EllipsizeHeightLimit, Family, FontSystem, Metrics, Shaping,
    SwashCache, Weight, Wrap,
};
use mimshel_render::Canvas;
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

    /// Только шрифты семейства `family` — быстро: полный набор (`new`) читает
    /// каждый шрифт системы (их бывают тысячи), после загрузки системы с
    /// холодного диска это секунды. Пути к файлам семейства знает fontconfig
    /// (`fc-list`). Знаков, которых в семействе нет, такой набор не нарисует —
    /// служба потом подменяет его полным. `None` — семейство не нашлось.
    pub fn quick(family: &str) -> Option<Fonts> {
        let out = std::process::Command::new("fc-list")
            .args([family, "-f", "%{file}\n"])
            .output()
            .ok()?;
        let mut db = cosmic_text::fontdb::Database::new();
        for path in String::from_utf8_lossy(&out.stdout).lines() {
            let _ = db.load_font_file(path);
        }
        if db.is_empty() {
            return None;
        }
        Some(Fonts {
            system: FontSystem::new_with_locale_and_db(locale(), db),
            cache: SwashCache::new(),
            family: family.to_string(),
        })
    }

    /// Ширина строки в пикселях.
    pub fn measure(&mut self, text: &str, style: &Style) -> f32 {
        let buffer = self.layout(text, style, None);
        buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    }

    /// Рисует одну строку. `x`, `y` — левый верхний угол строки высотой
    /// [`Style::line_height`]. Если задана `max_width`, лишнее заменяется на «…».
    /// Всё в точках холста; буквы рисуются сразу в пикселях (чёткие при
    /// любом масштабе).
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        style: &Style,
        x: f32,
        y: f32,
        max_width: Option<f32>,
    ) {
        let k = canvas.scale();
        self.draw_scaled(canvas.pixmap_mut(), k, text, style, x, y, max_width);
    }

    /// То же прямо на картинке в пикселях, без масштаба (экран блокировки
    /// рисуется в пикселях экрана).
    pub fn draw_pixels(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        style: &Style,
        x: f32,
        y: f32,
        max_width: Option<f32>,
    ) {
        self.draw_scaled(pixmap, 1.0, text, style, x, y, max_width);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_scaled(
        &mut self,
        pixmap: &mut Pixmap,
        k: f32,
        text: &str,
        style: &Style,
        x: f32,
        y: f32,
        max_width: Option<f32>,
    ) {
        let scaled = Style {
            size: style.size * k,
            ..*style
        };
        let mut buffer = self.layout(text, &scaled, max_width.map(|m| m * k));
        let [r, g, b, a] = style.color;
        let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
        let data = pixmap.data_mut();
        let (ox, oy) = ((x * k).round() as i32, (y * k).round() as i32);
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

/// Язык системы для правил текста (`ru_RU.UTF-8` → `ru-RU`), как берёт его
/// cosmic-text сам.
fn locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .map(|v| v.split('.').next().unwrap_or(&v).replace('_', "-"))
        .unwrap_or_else(|| "en-US".into())
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

        let mut pixmap = Canvas::with_scale(200, 30, 1.0).unwrap();
        fonts.draw(&mut pixmap, "Терминал", &STYLE, 4.0, 4.0, None);
        let painted = pixmap
            .pixmap()
            .data()
            .chunks(4)
            .filter(|p| p[3] > 0)
            .count();
        assert!(painted > 50, "нарисовано точек: {painted}");
    }

    #[test]
    fn ellipsis_keeps_within_width() {
        let mut fonts = Fonts::new("Inter");
        let mut pixmap = Canvas::with_scale(300, 30, 1.0).unwrap();
        let text = "Очень длинное название программы, которое не влезает";
        fonts.draw(&mut pixmap, text, &STYLE, 0.0, 0.0, Some(100.0));
        let w = pixmap.width() as usize;
        let beyond = pixmap
            .pixmap()
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
            let mut pm = Canvas::with_scale(80, 30, 1.0).unwrap();
            pm.fill(tiny_skia::Color::BLACK);
            let style = Style {
                color: [255, 255, 255, alpha],
                ..STYLE
            };
            fonts.draw(&mut pm, "Пн 28", &style, 2.0, 2.0, None);
            pm.pixmap().pixels().iter().map(|p| p.red()).max().unwrap()
        };
        assert!(brightest(&mut fonts, 255) > 240);
        let dim = brightest(&mut fonts, 70);
        assert!((50..=80).contains(&dim), "приглушённый текст: {dim}");
    }

    #[test]
    fn scaled_canvas_draws_bigger_text() {
        let mut fonts = Fonts::new("Inter");
        let width = |fonts: &mut Fonts, k: f32| {
            let mut c = Canvas::with_scale(200, 30, k).unwrap();
            fonts.draw(&mut c, "Терминал", &STYLE, 4.0, 4.0, None);
            let pm = c.pixmap();
            let w = pm.width() as usize;
            let cols: Vec<usize> = pm
                .data()
                .chunks(4)
                .enumerate()
                .filter(|(_, p)| p[3] > 0)
                .map(|(i, _)| i % w)
                .collect();
            cols.iter().max().unwrap() - cols.iter().min().unwrap()
        };
        let (one, two) = (width(&mut fonts, 1.0) as f32, width(&mut fonts, 2.0) as f32);
        assert!((two / one - 2.0).abs() < 0.1, "{one} → {two}");
    }

    #[test]
    fn quick_set_draws_like_full() {
        let Some(mut quick) = Fonts::quick("Inter") else {
            return; // Inter не установлен — проверять нечего
        };
        let mut full = Fonts::new("Inter");
        let a = quick.measure("Терминал 24 окт", &STYLE);
        let b = full.measure("Терминал 24 окт", &STYLE);
        assert!((a - b).abs() < 0.5, "{a} {b}");
        assert!(Fonts::quick("Нет-такого-шрифта-123").is_none());
    }
}
