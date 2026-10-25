//! Кадр терминала: сетка клеток → картинка. Фон клетки, знак из кэша нужным
//! цветом, подчёркивание и зачёркивание, выделение, курсор. Всё — прямо в
//! пикселях своего буфера, без общего рисования: так быстрее всего.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use tiny_skia::{IntSize, Pixmap};

use crate::glyphs::Glyphs;
use crate::palette::{Palette, Rgb};

/// Поля вокруг сетки, пиксели.
pub const PAD: u32 = 6;

/// Картинка в пикселях (непрозрачная).
struct Frame {
    w: u32,
    h: u32,
    data: Vec<u8>,
}

impl Frame {
    fn new(w: u32, h: u32, bg: Rgb) -> Frame {
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            data.extend_from_slice(&[bg[0], bg[1], bg[2], 0xff]);
        }
        Frame { w, h, data }
    }

    fn rect(&mut self, x: u32, y: u32, w: u32, h: u32, c: Rgb) {
        for row in y..(y + h).min(self.h) {
            for col in x..(x + w).min(self.w) {
                let i = ((row * self.w + col) * 4) as usize;
                self.data[i..i + 3].copy_from_slice(&c);
            }
        }
    }

    /// Stamp a glyph in colour `fg` over what is there (a colour glyph — in
    /// its own colours).
    fn blend(&mut self, x: u32, y: u32, mask: &crate::glyphs::Mask, fg: Rgb) {
        if let Some(color) = &mask.color {
            self.blend_color(x, y, mask, color);
            return;
        }
        for my in 0..mask.h {
            let row = y + my;
            if row >= self.h {
                break;
            }
            for mx in 0..mask.w {
                let col = x + mx;
                if col >= self.w {
                    break;
                }
                let a = u16::from(mask.alpha[(my * mask.w + mx) as usize]);
                if a == 0 {
                    continue;
                }
                let i = ((row * self.w + col) * 4) as usize;
                for (dst, &f) in self.data[i..i + 3].iter_mut().zip(&fg) {
                    *dst = ((u16::from(f) * a + u16::from(*dst) * (255 - a)) / 255) as u8;
                }
            }
        }
    }

    /// A colour glyph (premultiplied RGBA) over what is there.
    fn blend_color(&mut self, x: u32, y: u32, mask: &crate::glyphs::Mask, color: &[[u8; 4]]) {
        for my in 0..mask.h {
            let row = y + my;
            if row >= self.h {
                break;
            }
            for mx in 0..mask.w {
                let col = x + mx;
                if col >= self.w {
                    break;
                }
                let [r, g, b, a] = color[(my * mask.w + mx) as usize];
                if a == 0 {
                    continue;
                }
                let i = ((row * self.w + col) * 4) as usize;
                let keep = 255 - u16::from(a);
                for (dst, src) in self.data[i..i + 3].iter_mut().zip([r, g, b]) {
                    *dst = (u16::from(src) + u16::from(*dst) * keep / 255).min(255) as u8;
                }
            }
        }
    }

    fn into_pixmap(self) -> Option<Pixmap> {
        Pixmap::from_vec(self.data, IntSize::from_wh(self.w, self.h)?)
    }
}

/// Сколько клеток помещается в окно `w`×`h` пикселей.
pub fn grid_size(w: u32, h: u32, cell: (u32, u32)) -> (usize, usize) {
    let cols = (w.saturating_sub(2 * PAD) / cell.0).max(2);
    let rows = (h.saturating_sub(2 * PAD) / cell.1).max(1);
    (cols as usize, rows as usize)
}

/// Клетка под точкой окна (пиксели): столбец и строка экрана.
pub fn cell_at(x: f32, y: f32, cell: (u32, u32), (cols, rows): (usize, usize)) -> (usize, usize) {
    let col = ((x - PAD as f32) / cell.0 as f32).floor().max(0.0) as usize;
    let row = ((y - PAD as f32) / cell.1 as f32).floor().max(0.0) as usize;
    (
        col.min(cols.saturating_sub(1)),
        row.min(rows.saturating_sub(1)),
    )
}

/// Нарисовать терминал в картинку `w`×`h` пикселей. `focused` — окно
/// активно (курсор сплошной; иначе — рамкой).
pub fn render<T: EventListener>(
    term: &Term<T>,
    glyphs: &mut Glyphs,
    pal: &Palette,
    (w, h): (u32, u32),
    focused: bool,
    cursor_on: bool,
) -> Option<Pixmap> {
    let mut frame = Frame::new(w.max(1), h.max(1), pal.background);
    let content = term.renderable_content();
    let (cw, ch) = glyphs.cell;
    let offset = content.display_offset as i32;
    let colors = content.colors;
    let cursor = content.cursor;
    let lines = term.screen_lines() as i32;
    let at = |p: Point| -> Option<(u32, u32)> {
        let row = p.line.0 + offset;
        (0..lines)
            .contains(&row)
            .then(|| (PAD + p.column.0 as u32 * cw, PAD + row as u32 * ch))
    };
    for cell in content.display_iter {
        let flags = cell.flags;
        if flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let Some((x, y)) = at(cell.point) else {
            continue;
        };
        let mut fg = pal.resolve(cell.fg, colors);
        let mut bg = pal.resolve(cell.bg, colors);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.contains(Flags::DIM) {
            fg = fg.map(|v| (u16::from(v) * 2 / 3) as u8);
        }
        if flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        if content.selection.is_some_and(|s| s.contains(cell.point)) {
            bg = pal.selection;
        }
        let is_cursor = focused
            && cursor_on
            && cursor.point == cell.point
            && cursor.shape == CursorShape::Block;
        if is_cursor {
            fg = bg;
            bg = pal.cursor;
        }
        let wide = flags.contains(Flags::WIDE_CHAR);
        let width = if wide { cw * 2 } else { cw };
        if bg != pal.background {
            frame.rect(x, y, width, ch, bg);
        }
        if let Some(mask) = glyphs.get(cell.c, flags.contains(Flags::BOLD), wide) {
            frame.blend(x, y, mask, fg);
        }
        if flags.intersects(Flags::ALL_UNDERLINES) {
            frame.rect(x, y + ch - 2, width, 1, fg);
        }
        if flags.contains(Flags::STRIKEOUT) {
            frame.rect(x, y + ch / 2, width, 1, fg);
        }
    }
    // Курсор не сплошной: линия, черта снизу или рамка (окно не активно).
    if let Some((x, y)) = at(cursor.point).filter(|_| cursor_on) {
        match cursor.shape {
            CursorShape::Beam => frame.rect(x, y, 2, ch, pal.cursor),
            CursorShape::Underline => frame.rect(x, y + ch - 2, cw, 2, pal.cursor),
            CursorShape::HollowBlock => {
                frame.rect(x, y, cw, 1, pal.cursor);
                frame.rect(x, y + ch - 1, cw, 1, pal.cursor);
                frame.rect(x, y, 1, ch, pal.cursor);
                frame.rect(x + cw - 1, y, 1, ch, pal.cursor);
            }
            CursorShape::Block if !focused => {
                frame.rect(x, y, cw, 1, pal.cursor);
                frame.rect(x, y + ch - 1, cw, 1, pal.cursor);
                frame.rect(x, y, 1, ch, pal.cursor);
                frame.rect(x + cw - 1, y, 1, ch, pal.cursor);
            }
            _ => {}
        }
    }
    frame.into_pixmap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_fits_window() {
        let cell = (8, 18);
        assert_eq!(
            grid_size(80 * 8 + 2 * PAD, 24 * 18 + 2 * PAD, cell),
            (80, 24)
        );
        assert_eq!(grid_size(10, 10, cell), (2, 1), "крошечное окно — не ноль");
        assert_eq!(
            cell_at(PAD as f32 + 17.0, PAD as f32 + 40.0, cell, (80, 24)),
            (2, 2)
        );
        assert_eq!(cell_at(-5.0, 9999.0, cell, (80, 24)), (0, 23));
    }
}
