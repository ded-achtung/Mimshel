//! Как выглядит Spotlight. Размеры и цвета — из rofi-темы macos-look
//! (`spotlight.rasi`), чтобы замена была незаметной.

use shell_icons::Icons;
use shell_text::{Fonts, Style};
use tiny_skia::{
    Color, FillRule, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
};

use crate::Spotlight;

pub const WIDTH: u32 = 640;
/// Высота строки поиска.
pub const BAR_H: u32 = 56;
/// Высота строки результата.
pub const ROW_H: u32 = 40;
/// Отступ списка от краёв окна.
pub const LIST_PAD: u32 = 6;
pub const MAX_ROWS: usize = 8;

const RADIUS: f32 = 14.0;
const ROW_RADIUS: f32 = 8.0;
const BAR_PAD_X: f32 = 18.0;
const SEARCH_ICON: u32 = 22;
const ROW_PAD_X: f32 = 12.0;
const ICON: u32 = 26;
const GAP: f32 = 12.0;

const BACKGROUND: [u8; 4] = [0x1e, 0x1e, 0x1e, 0xe8];
const BORDER: [u8; 4] = [0xff, 0xff, 0xff, 0x1f];
const SELECTED: [u8; 4] = [0x0a, 0x64, 0xd6, 0xff];
const MAGNIFIER: [u8; 4] = [0x9a, 0x9a, 0x9a, 0xff];
const CURSOR: [u8; 4] = [0xf2, 0xf2, 0xf2, 0xd0];

const ENTRY: Style = Style {
    size: 22.0,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 400,
};
const PLACEHOLDER: Style = Style {
    color: [0x9a, 0x9a, 0x9a, 0xff],
    ..ENTRY
};
const ROW: Style = Style {
    size: 17.0,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 400,
};
const ROW_SELECTED: Style = Style {
    color: [0xff, 0xff, 0xff, 0xff],
    ..ROW
};
const DIM: Style = Style {
    color: [0x9a, 0x9a, 0x9a, 0xff],
    ..ROW
};

impl Spotlight {
    /// Рисует окно целиком в картинку размера [`Spotlight::size`].
    pub fn render(&self, fonts: &mut Fonts, icons: &mut Icons) -> Pixmap {
        let (w, h) = self.size();
        let mut pm = Pixmap::new(w, h).expect("размер окна не нулевой");
        let (wf, hf) = (w as f32, h as f32);

        // Фон и тонкая светлая рамка.
        if let Some(bg) = rounded(0.0, 0.0, wf, hf, RADIUS) {
            pm.fill_path(
                &bg,
                &paint(BACKGROUND),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        if let Some(edge) = rounded(0.5, 0.5, wf - 1.0, hf - 1.0, RADIUS - 0.5) {
            let stroke = Stroke {
                width: 1.0,
                ..Stroke::default()
            };
            pm.stroke_path(&edge, &paint(BORDER), &stroke, Transform::identity(), None);
        }

        // Строка поиска: лупа, текст или подсказка, курсор.
        let bar = BAR_H as f32;
        magnifier(
            &mut pm,
            BAR_PAD_X,
            (bar - SEARCH_ICON as f32) / 2.0,
            SEARCH_ICON as f32,
            MAGNIFIER,
        );
        let text_x = BAR_PAD_X + SEARCH_ICON as f32 + GAP;
        let text_y = (bar - ENTRY.line_height()) / 2.0;
        let text_w = wf - text_x - BAR_PAD_X;
        if self.field.is_empty() {
            fonts.draw(
                &mut pm,
                "Поиск Spotlight",
                &PLACEHOLDER,
                text_x,
                text_y,
                Some(text_w),
            );
        } else {
            fonts.draw(
                &mut pm,
                self.field.text(),
                &ENTRY,
                text_x,
                text_y,
                Some(text_w),
            );
        }
        let cursor_x =
            (text_x + fonts.measure(self.field.before_cursor(), &ENTRY)).min(wf - BAR_PAD_X);
        let cursor_h = ENTRY.size * 1.1;
        fill_rect(
            &mut pm,
            cursor_x.round(),
            ((bar - cursor_h) / 2.0).round(),
            2.0,
            cursor_h.round(),
            CURSOR,
        );

        let rows = self.visible_rows();
        if rows == 0 {
            return pm;
        }

        // Черта между полем и списком.
        fill_rect(&mut pm, 0.0, bar, wf, 1.0, BORDER);

        let top = (BAR_H + 1 + LIST_PAD) as f32;
        let pad = LIST_PAD as f32;
        let row_h = ROW_H as f32;
        let icon_x = pad + ROW_PAD_X;
        let name_x = icon_x + ICON as f32 + GAP;
        let name_w = wf - pad - ROW_PAD_X - name_x;

        if self.result_count() == 0 {
            let y = top + (row_h - DIM.line_height()) / 2.0;
            fonts.draw(&mut pm, "Ничего не найдено", &DIM, name_x, y, Some(name_w));
            return pm;
        }

        for (row, (i, app)) in self
            .results()
            .enumerate()
            .skip(self.scroll)
            .take(rows)
            .enumerate()
        {
            let y = top + row as f32 * row_h;
            let selected = i == self.selected;
            if selected && let Some(r) = rounded(pad, y, wf - pad * 2.0, row_h, ROW_RADIUS) {
                pm.fill_path(
                    &r,
                    &paint(SELECTED),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            if let Some(icon) = app.icon.as_deref().and_then(|n| icons.get(n, ICON)) {
                let iy = y as i32 + (ROW_H - ICON) as i32 / 2;
                draw_icon(&mut pm, icon, icon_x as i32, iy);
            }
            let style = if selected { ROW_SELECTED } else { ROW };
            let ty = y + (row_h - style.line_height()) / 2.0;
            fonts.draw(&mut pm, &app.name, &style, name_x, ty, Some(name_w));
        }
        pm
    }
}

pub(crate) fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c[0], c[1], c[2], c[3]));
    p.anti_alias = true;
    p
}

/// Серая лупа, как в Spotlight на Mac, в квадрате `size` с углом в `x`, `y`.
pub(crate) fn magnifier(pm: &mut Pixmap, x: f32, y: f32, size: f32, color: [u8; 4]) {
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

pub(crate) fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &paint(c), Transform::identity(), None);
    }
}

pub(crate) fn draw_icon(pm: &mut Pixmap, icon: &Pixmap, x: i32, y: i32) {
    pm.draw_pixmap(
        x,
        y,
        icon.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spotlight::tests::spotlight;
    use shell_core::input::{Key, KeyPress};

    #[test]
    fn renders_at_its_size() {
        let mut fonts = Fonts::new("Inter");
        let mut icons = Icons::with_dirs("none", vec![], vec![]);
        let mut s = spotlight();

        let empty = s.render(&mut fonts, &mut icons);
        assert_eq!((empty.width(), empty.height()), s.size());
        // Углы прозрачные (скругление), середина — тёмный полупрозрачный фон.
        assert_eq!(empty.pixel(0, 0).unwrap().alpha(), 0);
        assert_eq!(empty.pixel(320, 4).unwrap().alpha(), BACKGROUND[3]);

        s.key(KeyPress::plain(Key::Char('т')));
        let list = s.render(&mut fonts, &mut icons);
        assert_eq!((list.width(), list.height()), s.size());
        // Первая строка выбрана — синяя подложка.
        let y = BAR_H + 1 + LIST_PAD + 2;
        let px = list.pixel(WIDTH - LIST_PAD - 20, y).unwrap();
        assert_eq!((px.red(), px.green(), px.blue()), (0x0a, 0x64, 0xd6));
    }
}
