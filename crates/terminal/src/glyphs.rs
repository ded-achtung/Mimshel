//! Знаки терминала: каждая буква рисуется шрифтом один раз — в маску
//! прозрачности размером с клетку (у широких — две клетки), дальше её только
//! накладывают нужным цветом. Рисовать строку шрифтом на каждый кадр было бы
//! в десятки раз дольше.

use std::collections::HashMap;

use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};

/// Маска знака: прозрачность по пикселям, `w`×`h`.
pub struct Mask {
    pub w: u32,
    pub h: u32,
    pub alpha: Vec<u8>,
}

pub struct Glyphs {
    fonts: Fonts,
    /// Размер шрифта в пикселях экрана.
    size: f32,
    /// Клетка в пикселях: ширина и высота.
    pub cell: (u32, u32),
    cache: HashMap<(char, bool, bool), Option<Mask>>,
}

impl Glyphs {
    /// Моноширинный шрифт `family` размером `size` пикселей. Сначала только
    /// это семейство (быстро); нет его — все шрифты системы.
    pub fn new(family: &str, size: f32) -> Glyphs {
        let mut fonts = Fonts::quick(family).unwrap_or_else(|| Fonts::new(family));
        let style = Self::style(size, false);
        // Ширина клетки — шаг моноширинного шрифта (по 16 знакам — точнее).
        let w = (fonts.measure(&"M".repeat(16), &style) / 16.0).ceil() as u32;
        let h = style.line_height() as u32;
        Glyphs {
            fonts,
            size,
            cell: (w.max(1), h.max(1)),
            cache: HashMap::new(),
        }
    }

    fn style(size: f32, bold: bool) -> Style {
        Style {
            size,
            color: [0xff, 0xff, 0xff, 0xff],
            weight: if bold { 700 } else { 400 },
        }
    }

    /// Маска знака `c` (`wide` — знак на две клетки: иероглифы, эмодзи).
    /// `None` — пробел или знак, которого нет в шрифте.
    pub fn get(&mut self, c: char, bold: bool, wide: bool) -> Option<&Mask> {
        let key = (c, bold, wide);
        if !self.cache.contains_key(&key) {
            let mask = self.draw(c, bold, wide);
            self.cache.insert(key, mask);
        }
        self.cache.get(&key).and_then(Option::as_ref)
    }

    fn draw(&mut self, c: char, bold: bool, wide: bool) -> Option<Mask> {
        if c == ' ' || c == '\0' {
            return None;
        }
        let (cw, h) = self.cell;
        let w = if wide { cw * 2 } else { cw };
        // Холст в пикселях (масштаб 1): размер шрифта уже в пикселях экрана.
        let mut canvas = Canvas::with_scale(w, h, 1.0)?;
        let style = Self::style(self.size, bold);
        self.fonts
            .draw(&mut canvas, &c.to_string(), &style, 0.0, 0.0, None);
        let alpha: Vec<u8> = canvas.pixels().iter().map(|p| p.alpha()).collect();
        alpha.iter().any(|&a| a > 0).then_some(Mask { w, h, alpha })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_and_masks() {
        let mut g = Glyphs::new("DejaVu Sans Mono", 14.0);
        let (w, h) = g.cell;
        assert!(
            (5..=12).contains(&w) && (14..=22).contains(&h),
            "клетка {w}×{h}"
        );
        assert!(g.get(' ', false, false).is_none());
        let m = g.get('Ж', false, false).expect("кириллица есть");
        assert_eq!((m.w, m.h), (w, h));
        assert!(m.alpha.iter().any(|&a| a > 200));
        // Второй раз — из кэша (та же маска).
        let first = g.get('Ж', false, false).unwrap().alpha.clone();
        assert_eq!(g.get('Ж', false, false).unwrap().alpha, first);
    }
}
