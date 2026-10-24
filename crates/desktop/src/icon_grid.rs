//! Значки на столе: где каждый стоит, что под указателем, выделение и как
//! значок рисуется. Как на Mac — колонками от правого верхнего угла: сверху
//! вниз, потом следующая колонка левее.

use std::collections::HashMap;
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

/// Клетка сетки: (столбец, считая от правого края, строка сверху).
pub type Slot = (u32, u32);

pub struct IconGrid {
    items: Vec<Item>,
    /// Клетка каждого значка (по порядку `items`).
    places: Vec<Slot>,
    /// Запомненные клетки по имени файла: куда значок перетащили или где
    /// создали папку. У остальных — первая свободная клетка.
    saved: HashMap<String, Slot>,
    /// Где можно ставить значки (без строки меню и дока).
    area: Rect,
    selected: Option<usize>,
}

impl IconGrid {
    pub fn new(area: Rect) -> IconGrid {
        IconGrid {
            items: Vec::new(),
            places: Vec::new(),
            saved: HashMap::new(),
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

    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    /// Запомненные клетки (сохранить в файл).
    pub fn saved(&self) -> &HashMap<String, Slot> {
        &self.saved
    }

    /// Клетки, прочитанные из файла.
    pub fn set_saved(&mut self, saved: HashMap<String, Slot>) {
        self.saved = saved;
        self.layout();
    }

    /// Новый список (папка изменилась). Выделение остаётся на том же файле;
    /// клетки пропавших файлов забываются.
    pub fn set_items(&mut self, items: Vec<Item>) {
        let keep: Option<PathBuf> = self.selected().map(|i| i.path.clone());
        self.items = items;
        let names: Vec<String> = self.items.iter().map(file_name).collect();
        self.saved.retain(|name, _| names.contains(name));
        self.selected = keep.and_then(|p| self.items.iter().position(|i| i.path == p));
        self.layout();
    }

    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
        self.layout();
    }

    /// Выделить значок с этим путём (например, только что созданную папку).
    pub fn select_path(&mut self, path: &std::path::Path) -> Vec<usize> {
        let i = self.items.iter().position(|i| i.path == path);
        self.select(i)
    }

    /// Строк в колонке и колонок на экране.
    fn rows(&self) -> u32 {
        ((self.area.3 / CELL_H) as u32).max(1)
    }

    fn columns(&self) -> u32 {
        ((self.area.2 / CELL_W) as u32).max(1)
    }

    /// Расставить: сначала значки с запомненной клеткой (если она есть на
    /// экране и не занята), остальные — в свободные клетки колонками справа.
    fn layout(&mut self) {
        let (rows, cols) = (self.rows(), self.columns());
        let mut taken: Vec<Slot> = Vec::new();
        let mut places: Vec<Option<Slot>> = self
            .items
            .iter()
            .map(|item| {
                let slot = *self.saved.get(&file_name(item))?;
                (slot.0 < cols && slot.1 < rows && !taken.contains(&slot)).then(|| {
                    taken.push(slot);
                    slot
                })
            })
            .collect();
        let mut free = (0..u32::MAX)
            .map(|n| (n / rows, n % rows))
            .filter(|s| !taken.contains(s));
        for place in places.iter_mut().filter(|p| p.is_none()) {
            *place = free.next();
        }
        self.places = places.into_iter().map(|p| p.unwrap_or((0, 0))).collect();
    }

    /// Прямоугольник клетки.
    pub fn slot_rect(&self, (col, row): Slot) -> Rect {
        let (ax, ay, aw, _) = self.area;
        (
            ax + aw - (col as f32 + 1.0) * CELL_W,
            ay + row as f32 * CELL_H,
            CELL_W,
            CELL_H,
        )
    }

    /// Клетка под точкой (ближайшая на экране).
    pub fn slot_at(&self, x: f32, y: f32) -> Slot {
        let (ax, ay, aw, _) = self.area;
        let col = ((ax + aw - x) / CELL_W).floor().max(0.0) as u32;
        let row = ((y - ay) / CELL_H).floor().max(0.0) as u32;
        (col.min(self.columns() - 1), row.min(self.rows() - 1))
    }

    /// Ближайшая к `want` клетка, где нет других значков (`except` — этот
    /// значок не считается: его и двигаем).
    pub fn free_slot_near(&self, want: Slot, except: Option<usize>) -> Slot {
        let busy = |s: Slot| {
            self.places
                .iter()
                .enumerate()
                .any(|(i, p)| *p == s && Some(i) != except)
        };
        let (rows, cols) = (self.rows(), self.columns());
        let dist = |s: Slot| {
            let dc = i64::from(s.0) - i64::from(want.0);
            let dr = i64::from(s.1) - i64::from(want.1);
            dc * dc + dr * dr
        };
        (0..cols)
            .flat_map(|c| (0..rows).map(move |r| (c, r)))
            .filter(|&s| !busy(s))
            .min_by_key(|&s| dist(s))
            .unwrap_or(want)
    }

    /// Запомнить клетку для файла с этим именем (значок появится там).
    pub fn place_name(&mut self, name: &str, slot: Slot) {
        self.saved.insert(name.to_string(), slot);
        self.layout();
    }

    /// Перетащили значок `i` в клетку `slot` (занята — в ближайшую свободную).
    pub fn move_item(&mut self, i: usize, slot: Slot) {
        let Some(item) = self.items.get(i) else {
            return;
        };
        let slot = self.free_slot_near(slot, Some(i));
        self.saved.insert(file_name(item), slot);
        self.layout();
    }

    /// Файл переименовали — клетка остаётся за ним.
    pub fn rename_saved(&mut self, old: &str, new: &str) {
        if let Some(slot) = self.saved.remove(old) {
            self.saved.insert(new.to_string(), slot);
        } else if let Some(i) = self.items.iter().position(|it| file_name(it) == old) {
            // Значок стоял в клетке по порядку — запомнить её, чтобы новое
            // имя (другое место по алфавиту) не сдвинуло значок.
            self.saved.insert(new.to_string(), self.places[i]);
        }
    }

    /// «Упорядочить»: забыть все клетки — значки встают по порядку справа.
    pub fn arrange(&mut self) {
        self.saved.clear();
        self.layout();
    }

    /// Клетка значка `i`.
    pub fn cell(&self, i: usize) -> Rect {
        self.slot_rect(self.places.get(i).copied().unwrap_or((0, 0)))
    }

    /// Где подпись значка `i` (для поля переименования).
    pub fn label_rect(&self, i: usize) -> Rect {
        let (cx, cy, cw, _) = self.cell(i);
        (cx, cy + LABEL_TOP, cw, LABEL.line_height())
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

/// Имя файла значка — по нему запоминается клетка.
fn file_name(item: &Item) -> String {
    item.path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
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
    fn saved_places_moves_and_arrange() {
        let mut g = IconGrid::new((0.0, 0.0, 4.0 * CELL_W, 3.0 * CELL_H));
        g.set_items(vec![item("а"), item("б"), item("в")]);
        assert_eq!(g.cell(1), g.slot_rect((0, 1)), "по порядку справа");
        // Перетащили «б» в третью колонку от края.
        g.move_item(1, (2, 0));
        assert_eq!(g.cell(1), g.slot_rect((2, 0)));
        assert_eq!(g.cell(2), g.slot_rect((0, 1)), "«в» заняла место «б»");
        // На занятое место — в ближайшую свободную клетку.
        g.move_item(2, (2, 0));
        assert_ne!(g.cell(2), g.slot_rect((2, 0)));
        assert_eq!(g.saved().len(), 2);
        // Новую папку создают в клетке под щелчком — значок появится там.
        let (x, y, _, _) = g.slot_rect((3, 2));
        let slot = g.slot_at(x + 10.0, y + 10.0);
        assert_eq!(slot, (3, 2));
        g.place_name("Новая папка", slot);
        g.set_items(vec![item("а"), item("б"), item("в"), item("Новая папка")]);
        assert_eq!(g.cell(3), g.slot_rect((3, 2)));
        // Переименовали — клетка осталась за ним.
        g.rename_saved("Новая папка", "Фото");
        g.set_items(vec![item("а"), item("б"), item("в"), item("Фото")]);
        assert_eq!(g.cell(3), g.slot_rect((3, 2)));
        g.arrange();
        assert_eq!(g.cell(3), g.slot_rect((1, 0)), "по порядку");
        assert!(g.saved().is_empty());
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
