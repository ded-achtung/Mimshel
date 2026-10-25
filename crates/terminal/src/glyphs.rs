//! Terminal glyphs: each character is drawn by the font once — into a mask of
//! the cell size (two cells for wide ones) — and afterwards only stamped in
//! the needed colour. Shaping text on every frame would be tens of times slower.
//!
//! Fonts: the window opens at once with just the monospace family; every
//! other installed font (for emoji, CJK, rare symbols) loads in a background
//! thread and replaces it when ready — then the characters that were missing
//! are drawn again. Colour emoji keep their colours.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};

use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};

/// A glyph: transparency per pixel, or ready colour pixels (emoji).
pub struct Mask {
    pub w: u32,
    pub h: u32,
    pub alpha: Vec<u8>,
    /// Colour glyph: premultiplied RGBA per pixel, drawn as is (not tinted).
    pub color: Option<Vec<[u8; 4]>>,
}

pub struct Glyphs {
    fonts: Fonts,
    family: String,
    /// Font size in screen pixels.
    size: f32,
    /// Cell in pixels: width and height.
    pub cell: (u32, u32),
    cache: HashMap<(char, bool, bool), Option<Mask>>,
    /// All system fonts, while they are loading.
    full: Option<Receiver<Fonts>>,
}

impl Glyphs {
    /// Monospace family `family`, `size` pixels. Only that family at first
    /// (fast); `on_full` is called (from a background thread) when all fonts
    /// are loaded — call [`Glyphs::refresh`] then.
    pub fn new(family: &str, size: f32, on_full: impl FnOnce() + Send + 'static) -> Glyphs {
        let (tx, rx) = mpsc::channel();
        let name = family.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(Fonts::new(&name));
            on_full();
        });
        let fonts = Fonts::quick(family).unwrap_or_else(|| Fonts::new(family));
        let mut g = Glyphs {
            fonts,
            family: family.to_string(),
            size,
            cell: (1, 1),
            cache: HashMap::new(),
            full: Some(rx),
        };
        g.measure();
        g
    }

    fn style(size: f32, bold: bool) -> Style {
        Style {
            size,
            color: [0xff, 0xff, 0xff, 0xff],
            weight: if bold { 700 } else { 400 },
        }
    }

    /// Cell size: the monospace advance (over 16 characters — more precise).
    fn measure(&mut self) {
        let style = Self::style(self.size, false);
        let w = (self.fonts.measure(&"M".repeat(16), &style) / 16.0).ceil() as u32;
        let h = style.line_height() as u32;
        self.cell = (w.max(1), h.max(1));
    }

    /// The monospace family in use.
    pub fn family(&self) -> &str {
        &self.family
    }

    /// New font size (pixels): the cell changes, glyphs are drawn anew; the
    /// fonts themselves are not reloaded.
    pub fn set_size(&mut self, size: f32) {
        self.size = size;
        self.cache.clear();
        self.measure();
    }

    /// All fonts have loaded — switch to them and redraw the characters that
    /// were missing. `true` — the screen should be redrawn.
    pub fn refresh(&mut self) -> bool {
        let Some(rx) = &self.full else {
            return false;
        };
        let Ok(fonts) = rx.try_recv() else {
            return false;
        };
        self.full = None;
        self.fonts = fonts;
        self.cache.retain(|_, mask| mask.is_some());
        true
    }

    /// Glyph of `c` (`wide` — a two-cell character: CJK, emoji).
    /// `None` — a space or a character missing from the fonts.
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
        // Canvas in pixels (scale 1): the font size is already in screen pixels.
        let mut canvas = Canvas::with_scale(w, h, 1.0)?;
        let style = Self::style(self.size, bold);
        self.fonts
            .draw(&mut canvas, &c.to_string(), &style, 0.0, 0.0, None);
        let pixels = canvas.pixels();
        let alpha: Vec<u8> = pixels.iter().map(|p| p.alpha()).collect();
        if !alpha.iter().any(|&a| a > 0) {
            return None;
        }
        // Drawn in white: anything clearly not grey is a colour glyph (emoji).
        let colored = pixels.iter().any(|p| {
            let (r, g, b) = (p.red() as i16, p.green() as i16, p.blue() as i16);
            (r - g).abs() > 24 || (g - b).abs() > 24 || (r - b).abs() > 24
        });
        let color = colored.then(|| {
            pixels
                .iter()
                .map(|p| [p.red(), p.green(), p.blue(), p.alpha()])
                .collect()
        });
        Some(Mask { w, h, alpha, color })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_and_masks() {
        let mut g = Glyphs::new("DejaVu Sans Mono", 14.0, || {});
        let (w, h) = g.cell;
        assert!(
            (5..=12).contains(&w) && (14..=22).contains(&h),
            "cell {w}×{h}"
        );
        assert!(g.get(' ', false, false).is_none());
        let m = g.get('Ж', false, false).expect("Cyrillic is there");
        assert_eq!((m.w, m.h), (w, h));
        assert!(m.alpha.iter().any(|&a| a > 200));
        assert!(m.color.is_none(), "letters are tinted, not colour");
        // Second time — from the cache (the same mask).
        let first = g.get('Ж', false, false).unwrap().alpha.clone();
        assert_eq!(g.get('Ж', false, false).unwrap().alpha, first);
        // A bigger font — a bigger cell, fonts kept.
        g.set_size(20.0);
        assert!(g.cell.1 > h);
    }
}
