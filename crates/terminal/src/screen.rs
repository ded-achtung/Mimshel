//! Terminal pixels with a persistent buffer and row damage.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use tiny_skia::Pixmap;

use crate::glyphs::Glyphs;
use crate::palette::{Palette, Rgb};

/// Padding around the grid, in pixels.
pub const PAD: u32 = 6;

/// An opaque pixel buffer.
struct Frame<'a> {
    w: u32,
    h: u32,
    data: &'a mut [u8],
}

impl Frame<'_> {
    fn rect(&mut self, x: u32, y: u32, w: u32, h: u32, c: Rgb) {
        for row in y..(y + h).min(self.h) {
            for col in x..(x + w).min(self.w) {
                let i = ((row * self.w + col) * 4) as usize;
                self.data[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
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
}

/// Number of cells fitting a pixel window.
pub fn grid_size(w: u32, h: u32, cell: (u32, u32)) -> (usize, usize) {
    let cols = (w.saturating_sub(2 * PAD) / cell.0).max(2);
    let rows = (h.saturating_sub(2 * PAD) / cell.1).max(1);
    (cols as usize, rows as usize)
}

/// Column and visible row at a pixel position.
pub fn cell_at(x: f32, y: f32, cell: (u32, u32), (cols, rows): (usize, usize)) -> (usize, usize) {
    let col = ((x - PAD as f32) / cell.0 as f32).floor().max(0.0) as usize;
    let row = ((y - PAD as f32) / cell.1 as f32).floor().max(0.0) as usize;
    (
        col.min(cols.saturating_sub(1)),
        row.min(rows.saturating_sub(1)),
    )
}

/// Draw a complete reference frame.
pub fn render<T: EventListener>(
    term: &Term<T>,
    glyphs: &mut Glyphs,
    pal: &Palette,
    size: (u32, u32),
    focused: bool,
    cursor_on: bool,
) -> Option<Pixmap> {
    let mut image = Pixmap::new(size.0.max(1), size.1.max(1))?;
    draw_rows(
        term,
        glyphs,
        pal,
        &mut image,
        focused,
        cursor_on,
        &vec![true; term.screen_lines()],
        true,
    );
    Some(image)
}

#[allow(clippy::too_many_arguments)]
fn draw_rows<T: EventListener>(
    term: &Term<T>,
    glyphs: &mut Glyphs,
    pal: &Palette,
    image: &mut Pixmap,
    focused: bool,
    cursor_on: bool,
    rows: &[bool],
    full: bool,
) {
    let mut frame = Frame {
        w: image.width(),
        h: image.height(),
        data: image.data_mut(),
    };
    if full {
        frame.rect(0, 0, frame.w, frame.h, pal.background);
    } else {
        for (row, &dirty) in rows.iter().enumerate() {
            if dirty {
                frame.rect(
                    0,
                    PAD + row as u32 * glyphs.cell.1,
                    frame.w,
                    glyphs.cell.1,
                    pal.background,
                );
            }
        }
    }
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
        let row = cell.point.line.0 + offset;
        if row < 0 || !rows.get(row as usize).copied().unwrap_or(false) {
            continue;
        }
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
    // Non-solid cursor: beam, underline, or an unfocused outline.
    if let Some((x, y)) = at(cursor.point).filter(|_| {
        cursor_on
            && rows
                .get((cursor.point.line.0 + offset) as usize)
                .copied()
                .unwrap_or(false)
    }) {
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
}

/// Pixel rectangle with exclusive right and bottom edges.
pub type DamageRect = (u32, u32, u32, u32);

#[derive(PartialEq, Eq)]
struct State {
    size: (u32, u32),
    cell: (u32, u32),
    palette: Palette,
    focused: bool,
    selection: Option<alacritty_terminal::selection::SelectionRange>,
    offset: usize,
    colors: Vec<Option<alacritty_terminal::vte::ansi::Rgb>>,
}

/// A retained terminal image. Invalidate it when external drawing state changes.
#[derive(Default)]
pub struct Screen {
    pub image: Option<Pixmap>,
    pub damage: Vec<DamageRect>,
    state: Option<State>,
    cursor_row: Option<usize>,
    cursor: Option<alacritty_terminal::term::RenderableCursor>,
    cursor_on: bool,
    cells: Vec<Vec<alacritty_terminal::term::cell::Cell>>,
}

impl Screen {
    pub fn invalidate(&mut self) {
        self.state = None;
    }

    /// Consume Alacritty damage while the terminal is locked by the caller.
    pub fn update<T: EventListener>(
        &mut self,
        term: &mut Term<T>,
        glyphs: &mut Glyphs,
        pal: &Palette,
        size: (u32, u32),
        focused: bool,
        cursor_on: bool,
    ) -> Option<()> {
        let content = term.renderable_content();
        // Colors has no equality implementation; compare its small public table.
        let colors: Vec<_> = (0..alacritty_terminal::term::color::COUNT)
            .map(|i| content.colors[i])
            .collect();
        let cursor_row =
            usize::try_from(content.cursor.point.line.0 + content.display_offset as i32)
                .ok()
                .filter(|&row| row < term.screen_lines());
        let cursor = content.cursor;
        let cursor_changed = self.cursor != Some(cursor) || self.cursor_on != cursor_on;
        let state = State {
            size,
            cell: glyphs.cell,
            palette: pal.clone(),
            focused,
            selection: content.selection,
            offset: content.display_offset,
            colors,
        };
        let mut full = self.state.as_ref() != Some(&state);
        if self
            .image
            .as_ref()
            .is_none_or(|p| (p.width(), p.height()) != size)
        {
            self.image = Some(Pixmap::new(size.0, size.1)?);
            full = true;
        }
        let mut rows = vec![false; term.screen_lines()];
        match term.damage() {
            alacritty_terminal::term::TermDamage::Full => full = true,
            alacritty_terminal::term::TermDamage::Partial(lines) => {
                for line in lines {
                    if let Some(row) = rows.get_mut(line.line) {
                        *row = true;
                    }
                }
            }
        }
        for row in self.cursor_row.into_iter().chain(cursor_row) {
            if let Some(dirty) = rows.get_mut(row) {
                *dirty = true;
            }
        }
        let old_cursor_row = self.cursor_row;
        self.cursor_row = cursor_row;
        self.cursor = Some(cursor);
        self.cursor_on = cursor_on;
        if full {
            rows.fill(true);
        }
        self.cells.resize_with(rows.len(), Vec::new);
        for (row, dirty) in rows.iter_mut().enumerate() {
            if !*dirty {
                continue;
            }
            let line = alacritty_terminal::index::Line(row as i32 - state.offset as i32);
            let points = || {
                (0..term.columns())
                    .map(|col| Point::new(line, alacritty_terminal::index::Column(col)))
            };
            let cells_changed = !points().map(|p| &term.grid()[p]).eq(self.cells[row].iter());
            // Alacritty always damages the cursor, even for title-only output.
            // Compare cells exactly so a notification with no visible change draws nothing.
            *dirty = full
                || cells_changed
                || (cursor_changed && (Some(row) == old_cursor_row || Some(row) == cursor_row));
            if cells_changed {
                self.cells[row].clear();
                self.cells[row].extend(points().map(|p| term.grid()[p].clone()));
            }
        }
        draw_rows(
            term,
            glyphs,
            pal,
            self.image.as_mut()?,
            focused,
            cursor_on,
            &rows,
            full,
        );
        self.damage.clear();
        if full {
            self.damage.push((0, 0, size.0, size.1));
        } else {
            for (row, dirty) in rows.into_iter().enumerate() {
                if dirty {
                    let y = PAD + row as u32 * glyphs.cell.1;
                    self.damage
                        .push((0, y.min(size.1), size.0, (y + glyphs.cell.1).min(size.1)));
                }
            }
        }
        term.reset_damage();
        self.state = Some(state);
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Term<impl EventListener>, Glyphs, Palette, (u32, u32)) {
        struct Listener;
        impl EventListener for Listener {
            fn send_event(&self, _: alacritty_terminal::event::Event) {}
        }
        let mut term = Term::new(
            Default::default(),
            &alacritty_terminal::term::test::TermSize::new(200, 60),
            Listener,
        );
        let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
        for row in 0..60 {
            let text = format!("\x1b[{};1H{}", row + 1, "abcdefghij".repeat(20));
            parser.advance(&mut term, text.as_bytes());
        }
        let glyphs = Glyphs::new("DejaVu Sans Mono", 14.0, || {});
        let size = (200 * glyphs.cell.0 + 2 * PAD, 60 * glyphs.cell.1 + 2 * PAD);
        (term, glyphs, Palette::default(), size)
    }

    #[test]
    fn measure_damage() {
        let (mut term, mut glyphs, pal, size) = fixture();
        let mut screen = Screen::default();
        screen
            .update(&mut term, &mut glyphs, &pal, size, true, true)
            .unwrap();
        screen.invalidate();
        let start = std::time::Instant::now();
        screen
            .update(&mut term, &mut glyphs, &pal, size, true, true)
            .unwrap();
        let full = start.elapsed();
        let full_area: u32 = screen
            .damage
            .iter()
            .map(|&(x0, y0, x1, y1)| (x1 - x0) * (y1 - y0))
            .sum();
        let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
        // Keep the cursor on the modified row in both frames.
        parser.advance(&mut term, b"\x1b[30;20H");
        screen
            .update(&mut term, &mut glyphs, &pal, size, true, true)
            .unwrap();
        parser.advance(&mut term, b"X");
        let start = std::time::Instant::now();
        screen
            .update(&mut term, &mut glyphs, &pal, size, true, true)
            .unwrap();
        let elapsed = start.elapsed();
        let area: u32 = screen
            .damage
            .iter()
            .map(|&(x0, y0, x1, y1)| (x1 - x0) * (y1 - y0))
            .sum();
        println!(
            "full={full:?}, area={full_area} pixels; one_cell={elapsed:?}, area={area} pixels"
        );
        assert_eq!(area, size.0 * glyphs.cell.1);
    }

    #[test]
    fn title_only_and_unchanged_frames_draw_nothing() {
        let (mut term, mut glyphs, pal, size) = fixture();
        let mut screen = Screen::default();
        screen
            .update(&mut term, &mut glyphs, &pal, size, true, true)
            .unwrap();
        let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
        parser.advance(&mut term, b"\x1b]2;new title\x07");
        screen
            .update(&mut term, &mut glyphs, &pal, size, true, true)
            .unwrap();
        assert!(screen.damage.is_empty());
        screen
            .update(&mut term, &mut glyphs, &pal, size, true, false)
            .unwrap();
        assert_eq!(screen.damage.len(), 1, "blink only changes the cursor row");
        screen
            .update(&mut term, &mut glyphs, &pal, size, true, false)
            .unwrap();
        assert!(screen.damage.is_empty());
    }

    #[test]
    fn damaged_rows_match_full_frames() {
        use alacritty_terminal::index::{Column, Line, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let (mut term, mut glyphs, mut pal, mut size) = fixture();
        let mut screen = Screen::default();
        let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
        screen
            .update(&mut term, &mut glyphs, &pal, size, true, true)
            .unwrap();
        for (name, bytes) in [
            ("one cell", b"\x1b[60;20HX".as_slice()),
            ("cursor moved", b"\x1b[4;4H"),
            ("line scrolled", b"\x1b[60;1H\n"),
            (
                "wide, inverse, underline",
                "\x1b[10;1H\x1b[7;4m界a\x1b[0m".as_bytes(),
            ),
            ("palette", b"\x1b]4;1;rgb:ff/00/00\x07"),
        ] {
            parser.advance(&mut term, bytes);
            screen
                .update(&mut term, &mut glyphs, &pal, size, true, true)
                .unwrap();
            assert_eq!(
                screen.image.as_ref().unwrap().data(),
                render(&term, &mut glyphs, &pal, size, true, true)
                    .unwrap()
                    .data(),
                "{name}"
            );
            if name == "cursor moved" {
                assert_eq!(screen.damage.len(), 2);
            }
            if name == "palette" {
                assert_eq!(screen.damage, [(0, 0, size.0, size.1)]);
            }
        }
        for case in 0..10 {
            let mut focused = true;
            let mut cursor_on = true;
            match case {
                0 => {
                    let mut selection = Selection::new(
                        SelectionType::Simple,
                        Point::new(Line(3), Column(2)),
                        Side::Left,
                    );
                    selection.update(Point::new(Line(5), Column(9)), Side::Right);
                    term.selection = Some(selection);
                }
                1 => term.scroll_display(alacritty_terminal::grid::Scroll::Top),
                2 => pal.background = [5, 10, 15],
                3 => focused = false,
                4 => {
                    glyphs.set_size(16.0);
                    screen.invalidate();
                }
                5 => {
                    size.0 += 10;
                    term.resize(alacritty_terminal::term::test::TermSize::new(190, 55));
                }
                6 => {
                    screen.invalidate();
                } // Tab/decorations changed.
                7 => {
                    cursor_on = false;
                }
                8 => {
                    term.selection = Some(Selection::new(
                        SelectionType::Lines,
                        Point::new(Line(0), Column(0)),
                        Side::Left,
                    ));
                }
                _ => term.selection = None,
            }
            screen
                .update(&mut term, &mut glyphs, &pal, size, focused, cursor_on)
                .unwrap();
            assert_eq!(
                screen.image.as_ref().unwrap().data(),
                render(&term, &mut glyphs, &pal, size, focused, cursor_on)
                    .unwrap()
                    .data(),
                "fallback {case}"
            );
            if case != 7 {
                assert_eq!(screen.damage, [(0, 0, size.0, size.1)], "fallback {case}");
            }
        }
    }

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
