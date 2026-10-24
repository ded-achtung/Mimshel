//! Значки на столе: где каждый стоит, что под указателем, выделение и как
//! значок рисуется. Как на Mac — колонками от правого верхнего угла: сверху
//! вниз, потом следующая колонка левее.

use std::path::PathBuf;

use shell_render::Canvas;
use shell_text::{Fonts, Style};
use shell_widgets::shape::{paint, rounded};
use tiny_skia::{FillRule, Pixmap, PixmapPaint, Transform};

use crate::files::Item;

/// Клетка одного значка (в точках).
pub const CELL_W: f32 = 96.0;
pub const CELL_H: f32 = 106.0;
/// Размер картинки значка.
pub const ICON: u32 = 56;
/// Сверху картинки — отступ, под ней — подпись.
const ICON_TOP: f32 = 6.0;
const LABEL_TOP: f32 = ICON_TOP + ICON as f32 + 6.0;
const LABEL_W: f32 = CELL_W - 8.0;

const LABEL: Style = Style {
    size: 12.5,
    color: [0xff, 0xff, 0xff, 0xff],
    weight: 500,
};
/// Тень под подписью — чтобы читалась на любых обоях.
const LABEL_SHADOW: Style = Style {
    color: [0x00, 0x00, 0x00, 0xb0],
    ..LABEL
};
/// Выделенный значок: подложка под картинкой и синяя под подписью.
const ICON_SELECTED: [u8; 4] = [0x00, 0x00, 0x00, 0x50];
const LABEL_SELECTED: [u8; 4] = [0x0a, 0x64, 0xd6, 0xff];

/// Прямоугольник в точках: (x, y, ширина, высота).
pub type Rect = (f32, f32, f32, f32);

pub struct IconGrid {
    items: Vec<Item>,
    /// Где можно ставить значки (без строки меню и дока).
    area: Rect,
    selected: Option<usize>,
}

impl IconGrid {
    pub fn new(area: Rect) -> IconGrid {
        IconGrid {
            items: Vec::new(),
            area,
            selected: None,
        }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn selected(&self) -> Option<&Item> {
        self.selected.and_then(|i| self.items.get(i))
    }

    /// Новый список (папка изменилась). Выделение остаётся на том же файле.
    pub fn set_items(&mut self, items: Vec<Item>) {
        let keep: Option<PathBuf> = self.selected().map(|i| i.path.clone());
        self.items = items;
        self.selected = keep.and_then(|p| self.items.iter().position(|i| i.path == p));
    }

    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    /// Выделить значок с этим путём (например, только что созданную папку).
    pub fn select_path(&mut self, path: &std::path::Path) -> Vec<usize> {
        let i = self.items.iter().position(|i| i.path == path);
        self.select(i)
    }

    /// Строк в колонке.
    fn rows(&self) -> usize {
        ((self.area.3 / CELL_H) as usize).max(1)
    }

    /// Клетка значка `i`.
    pub fn cell(&self, i: usize) -> Rect {
        let rows = self.rows();
        let (col, row) = ((i / rows) as f32, (i % rows) as f32);
        let (ax, ay, aw, _) = self.area;
        (
            ax + aw - (col + 1.0) * CELL_W,
            ay + row * CELL_H,
            CELL_W,
            CELL_H,
        )
    }

    /// Значок под точкой: картинка или подпись (не вся клетка — между
    /// значками щелчок попадает в стол).
    pub fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        (0..self.items.len()).find(|&i| {
            let (cx, cy, cw, _) = self.cell(i);
            let icon_x = cx + (cw - ICON as f32) / 2.0 - 4.0;
            let on_icon = x >= icon_x
                && x < icon_x + ICON as f32 + 8.0
                && y >= cy + ICON_TOP - 2.0
                && y < cy + LABEL_TOP;
            let on_label = x >= cx + 4.0
                && x < cx + cw - 4.0
                && y >= cy + LABEL_TOP
                && y < cy + LABEL_TOP + 2.0 * LABEL.line_height();
            on_icon || on_label
        })
    }

    /// Выделить значок (`None` — снять). Возвращает значки, которые надо
    /// перерисовать.
    pub fn select(&mut self, i: Option<usize>) -> Vec<usize> {
        if i == self.selected {
            return Vec::new();
        }
        let changed: Vec<usize> = [self.selected, i].into_iter().flatten().collect();
        self.selected = i;
        changed
    }

    /// Нарисовать значок `i` на холсте, левый верхний угол которого — в точке
    /// `origin` стола. `icon` — картинка (в пикселях экрана), `None` — без неё.
    pub fn draw(
        &self,
        pm: &mut Canvas,
        origin: (f32, f32),
        i: usize,
        icon: Option<&Pixmap>,
        fonts: &mut Fonts,
    ) {
        let Some(item) = self.items.get(i) else {
            return;
        };
        let (cx, cy, cw, _) = self.cell(i);
        let (x, y) = (cx - origin.0, cy - origin.1);
        let selected = self.selected == Some(i);
        let id = Transform::identity();
        let ix = x + (cw - ICON as f32) / 2.0;
        if selected
            && let Some(r) = rounded(
                ix - 4.0,
                y + ICON_TOP - 4.0,
                ICON as f32 + 8.0,
                ICON as f32 + 8.0,
                6.0,
            )
        {
            pm.fill_path(&r, &paint(ICON_SELECTED), FillRule::Winding, id, None);
        }
        if let Some(img) = icon {
            let k = ICON as f32 / img.width().max(1) as f32;
            let t = Transform::from_scale(k, k).post_translate(ix, y + ICON_TOP);
            let quality = PixmapPaint {
                quality: tiny_skia::FilterQuality::Bicubic,
                ..PixmapPaint::default()
            };
            pm.draw_pixmap(0, 0, img.as_ref(), &quality, t, None);
        }
        let lines = label_lines(fonts, &item.name);
        let lh = LABEL.line_height();
        for (n, line) in lines.iter().enumerate() {
            let tw = fonts.measure(line, &LABEL).min(LABEL_W);
            let tx = (x + (cw - tw) / 2.0).round();
            let ty = y + LABEL_TOP + n as f32 * lh;
            if selected {
                if let Some(r) = rounded(tx - 4.0, ty, tw + 8.0, lh, 4.0) {
                    pm.fill_path(&r, &paint(LABEL_SELECTED), FillRule::Winding, id, None);
                }
            } else {
                fonts.draw(pm, line, &LABEL_SHADOW, tx, ty + 1.0, Some(LABEL_W));
            }
            fonts.draw(pm, line, &LABEL, tx, ty, Some(LABEL_W));
        }
    }
}

/// Подпись в две строки: первая — сколько влезет (по словам, а длинное
/// слово — по буквам), вторая — остальное, с «…», если не влезло.
fn label_lines(fonts: &mut Fonts, text: &str) -> Vec<String> {
    if fonts.measure(text, &LABEL) <= LABEL_W {
        return vec![text.to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    // Сколько букв влезает в первую строку.
    let mut fit = 0;
    for n in 1..=chars.len() {
        let part: String = chars[..n].iter().collect();
        if fonts.measure(&part, &LABEL) > LABEL_W {
            break;
        }
        fit = n;
    }
    // Перенос — по последнему пробелу или знаку (слово целиком на вторую
    // строку); нет их — по буквам.
    let cut = chars[..fit]
        .iter()
        .rposition(|c| matches!(c, ' ' | '-' | '_' | '.'))
        .map(|p| p + 1)
        .unwrap_or(fit.max(1));
    let first: String = chars[..cut].iter().collect();
    let rest: String = chars[cut..].iter().collect();
    vec![first.trim_end().to_string(), rest.trim_start().to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::Kind;

    fn item(name: &str) -> Item {
        Item {
            path: PathBuf::from(format!("/стол/{name}")),
            name: name.into(),
            kind: Kind::File,
            icons: vec![],
        }
    }

    #[test]
    fn columns_from_top_right() {
        let mut g = IconGrid::new((0.0, 40.0, 1000.0, 3.0 * CELL_H + 10.0));
        g.set_items((0..5).map(|n| item(&n.to_string())).collect());
        assert_eq!(g.cell(0), (1000.0 - CELL_W, 40.0, CELL_W, CELL_H));
        assert_eq!(g.cell(2).1, 40.0 + 2.0 * CELL_H, "вниз по колонке");
        assert_eq!(
            g.cell(3),
            (1000.0 - 2.0 * CELL_W, 40.0, CELL_W, CELL_H),
            "следующая колонка левее"
        );
        let (x, y, w, _) = g.cell(3);
        assert_eq!(g.item_at(x + w / 2.0, y + 30.0), Some(3));
        assert_eq!(g.item_at(x + 2.0, y + 2.0), None, "угол клетки — это стол");
    }

    #[test]
    fn selection_survives_rescan() {
        let mut g = IconGrid::new((0.0, 0.0, 500.0, 500.0));
        g.set_items(vec![item("б"), item("в")]);
        assert_eq!(g.select(Some(1)), [1]);
        assert_eq!(g.select(Some(1)), Vec::<usize>::new());
        g.set_items(vec![item("а"), item("б"), item("в")]);
        assert_eq!(g.selected().unwrap().name, "в");
        assert_eq!(g.select(None), [2]);
    }

    #[test]
    fn long_names_wrap_in_two_lines() {
        let mut fonts = Fonts::new("Inter");
        assert_eq!(label_lines(&mut fonts, "Проекты"), ["Проекты"]);
        let lines = label_lines(&mut fonts, "Очень длинное название документа.pdf");
        assert_eq!(lines.len(), 2);
        assert!(fonts.measure(&lines[0], &LABEL) <= LABEL_W);
        assert_eq!(lines[0], "Очень", "по пробелу, не посреди слова");
    }
}
