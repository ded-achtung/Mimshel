//! Значки на столе: где каждый стоит, что под указателем, выделение и как
//! значок рисуется. Как на Mac — колонками от правого верхнего угла: сверху
//! вниз, потом следующая колонка левее.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};
use mimshel_widgets::shape::{paint, rounded};
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
    /// Выделенные значки (по возрастанию).
    selected: Vec<usize>,
    /// Значок, который сейчас переименовывают: его подпись не рисуется
    /// (поверх — поле ввода).
    renaming: Option<PathBuf>,
    /// Папка, над которой несут значки, — подсвечена: туда они упадут.
    drop_target: Option<usize>,
}

/// Куда стрелкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl IconGrid {
    pub fn new(area: Rect) -> IconGrid {
        IconGrid {
            items: Vec::new(),
            places: Vec::new(),
            saved: HashMap::new(),
            area,
            selected: Vec::new(),
            renaming: None,
            drop_target: None,
        }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Выделенный значок, если он один.
    pub fn selected(&self) -> Option<&Item> {
        match self.selected[..] {
            [i] => self.items.get(i),
            _ => None,
        }
    }

    /// Все выделенные значки.
    pub fn selected_items(&self) -> Vec<&Item> {
        self.selected
            .iter()
            .filter_map(|&i| self.items.get(i))
            .collect()
    }

    pub fn selected_indices(&self) -> &[usize] {
        &self.selected
    }

    /// Переименовывают этот значок (`None` — никакой). Возвращает, какие
    /// значки перерисовать.
    pub fn set_renaming(&mut self, path: Option<PathBuf>) -> Vec<usize> {
        let changed: Vec<usize> = (0..self.items.len())
            .filter(|&i| {
                let p = Some(&self.items[i].path);
                p == self.renaming.as_ref() || p == path.as_ref()
            })
            .collect();
        self.renaming = path;
        changed
    }

    /// Подсветить папку под переносимыми значками (`None` — снять).
    /// Возвращает, какие значки перерисовать.
    pub fn set_drop_target(&mut self, i: Option<usize>) -> Vec<usize> {
        if i == self.drop_target {
            return Vec::new();
        }
        let changed = [self.drop_target, i].into_iter().flatten().collect();
        self.drop_target = i;
        changed
    }

    /// Выделить всё (Ctrl+A).
    pub fn select_all(&mut self) -> Vec<usize> {
        self.set_selection((0..self.items.len()).collect())
    }

    /// Соседний значок от `from` в эту сторону (ближайший по прямой, с
    /// перевесом в сторону стрелки). Нет `from` — первый по порядку.
    pub fn neighbor(&self, from: Option<usize>, dir: Direction) -> Option<usize> {
        let Some(from) = from else {
            return (!self.items.is_empty()).then_some(0);
        };
        let (fc, fr) = self.slot(from);
        let (fc, fr) = (i64::from(fc), i64::from(fr));
        (0..self.items.len())
            .filter(|&i| i != from)
            .filter_map(|i| {
                let (c, r) = self.slot(i);
                let (dc, dr) = (i64::from(c) - fc, i64::from(r) - fr);
                // Столбцы считаются от правого края: левее — больше.
                let (along, across) = match dir {
                    Direction::Up => (-dr, dc),
                    Direction::Down => (dr, dc),
                    Direction::Left => (dc, dr),
                    Direction::Right => (-dc, dr),
                };
                (along > 0).then_some((along * along + 4 * across * across, i))
            })
            .min()
            .map(|(_, i)| i)
    }

    /// Первый значок после `after` (по кругу), имя которого начинается с
    /// `prefix` — набор букв на столе прыгает к значку, как в Finder.
    pub fn find_prefix(&self, prefix: &str, after: Option<usize>) -> Option<usize> {
        let prefix = prefix.to_lowercase();
        let n = self.items.len();
        let start = after.map_or(0, |a| a + 1);
        (0..n)
            .map(|k| (start + k) % n)
            .find(|&i| self.items[i].name.to_lowercase().starts_with(&prefix))
    }

    pub fn is_selected(&self, i: usize) -> bool {
        self.selected.contains(&i)
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
        let keep: Vec<PathBuf> = self
            .selected_items()
            .iter()
            .map(|i| i.path.clone())
            .collect();
        self.items = items;
        let names: HashSet<String> = self.items.iter().map(file_name).collect();
        let keep: HashSet<PathBuf> = keep.into_iter().collect();
        self.saved.retain(|name, _| names.contains(name));
        self.selected = (0..self.items.len())
            .filter(|&i| keep.contains(&self.items[i].path))
            .collect();
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
        let mut taken: HashSet<Slot> = HashSet::new();
        let mut places: Vec<Option<Slot>> = self
            .items
            .iter()
            .map(|item| {
                let slot = *self.saved.get(&file_name(item))?;
                (slot.0 < cols && slot.1 < rows && taken.insert(slot)).then_some(slot)
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
        let occupied: HashSet<Slot> = self
            .places
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != except)
            .map(|(_, &p)| p)
            .collect();
        let (rows, cols) = (self.rows(), self.columns());
        let dist = |s: Slot| {
            let dc = i64::from(s.0) - i64::from(want.0);
            let dr = i64::from(s.1) - i64::from(want.1);
            dc * dc + dr * dr
        };
        (0..cols)
            .flat_map(|c| (0..rows).map(move |r| (c, r)))
            .filter(|s| !occupied.contains(s))
            .min_by_key(|&s| dist(s))
            .unwrap_or(want)
    }

    /// Запомнить нынешние клетки всех значков: после того как значок
    /// передвинули руками (или убрали со стола — в папку, в корзину),
    /// остальные не должны сами «затекать» в освободившиеся клетки (как на
    /// Mac).
    pub fn freeze(&mut self) {
        for (i, item) in self.items.iter().enumerate() {
            if let Some(&slot) = self.places.get(i) {
                self.saved.insert(file_name(item), slot);
            }
        }
    }

    /// Запомнить клетку для файла с этим именем (значок появится там).
    pub fn place_name(&mut self, name: &str, slot: Slot) {
        self.freeze();
        self.saved.insert(name.to_string(), slot);
        self.layout();
    }

    /// Перетащили значок `i` в клетку `slot` (занята — в ближайшую свободную).
    pub fn move_item(&mut self, i: usize, slot: Slot) {
        let Some(item) = self.items.get(i) else {
            return;
        };
        let name = file_name(item);
        let slot = self.free_slot_near(slot, Some(i));
        self.freeze();
        self.saved.insert(name, slot);
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

    /// Выделить один значок (`None` — снять всё). Возвращает значки, которые
    /// надо перерисовать.
    pub fn select(&mut self, i: Option<usize>) -> Vec<usize> {
        self.set_selection(i.into_iter().collect())
    }

    /// Выделить эти значки (рамкой). Возвращает, какие перерисовать.
    pub fn set_selection(&mut self, mut new: Vec<usize>) -> Vec<usize> {
        new.sort_unstable();
        new.dedup();
        // Оба списка отсортированы; симметрическая разность двух указателей
        // вместо O(N²) повторных Vec::contains.
        let mut changed = Vec::new();
        let (mut a, mut b) = (0, 0);
        while a < self.selected.len() || b < new.len() {
            match (self.selected.get(a), new.get(b)) {
                (Some(&old), Some(&next)) if old == next => {
                    a += 1;
                    b += 1;
                }
                (Some(&old), Some(&next)) if old < next => {
                    changed.push(old);
                    a += 1;
                }
                (Some(_), Some(&next)) => {
                    changed.push(next);
                    b += 1;
                }
                (Some(&old), None) => {
                    changed.push(old);
                    a += 1;
                }
                (None, Some(&next)) => {
                    changed.push(next);
                    b += 1;
                }
                (None, None) => break,
            }
        }
        self.selected = new;
        changed
    }

    /// Где значок виден: картинка и две строки подписи.
    fn item_rect(&self, i: usize) -> Rect {
        let (cx, cy, cw, _) = self.cell(i);
        (
            cx + 4.0,
            cy + ICON_TOP - 2.0,
            cw - 8.0,
            LABEL_TOP - ICON_TOP + 2.0 + 2.0 * LABEL.line_height(),
        )
    }

    /// Значки, которых касается рамка `rect`.
    pub fn items_in(&self, (x, y, w, h): Rect) -> Vec<usize> {
        (0..self.items.len())
            .filter(|&i| {
                let (ix, iy, iw, ih) = self.item_rect(i);
                ix < x + w && x < ix + iw && iy < y + h && y < iy + ih
            })
            .collect()
    }

    /// Клетка значка `i` (для перетаскивания группы).
    pub fn slot(&self, i: usize) -> Slot {
        self.places.get(i).copied().unwrap_or((0, 0))
    }

    /// Перетащили несколько значков: каждый сдвигается так же, как значок
    /// `lead` (его принесли в `to`).
    pub fn move_items(&mut self, lead: usize, to: Slot, items: &[usize]) {
        let from = self.slot(lead);
        let (dc, dr) = (
            i64::from(to.0) - i64::from(from.0),
            i64::from(to.1) - i64::from(from.1),
        );
        let (cols, rows) = (i64::from(self.columns()), i64::from(self.rows()));
        // Ведущий — первым: он встаёт ровно туда, куда принесли. Куда
        // каждому — считаем по клеткам до переноса.
        let mut order: Vec<usize> = vec![lead];
        order.extend(items.iter().copied().filter(|&i| i != lead));
        let targets: Vec<(usize, Slot)> = order
            .into_iter()
            .map(|i| {
                let (c, r) = self.slot(i);
                let t = (
                    (i64::from(c) + dc).clamp(0, cols - 1) as u32,
                    (i64::from(r) + dr).clamp(0, rows - 1) as u32,
                );
                (i, t)
            })
            .collect();
        // Запоминаем исходные места один раз. Раньше каждый move_item()
        // заново делал freeze + layout, то есть N полных пересчётов сетки.
        self.freeze();
        let selected: HashSet<usize> = items
            .iter()
            .copied()
            .filter(|&i| i < self.items.len())
            .chain(std::iter::once(lead))
            .collect();
        let mut occupied: HashSet<Slot> = self
            .places
            .iter()
            .enumerate()
            .filter(|(i, _)| !selected.contains(i))
            .map(|(_, &slot)| slot)
            .collect();
        for (i, target) in targets {
            if i >= self.items.len() {
                continue;
            }
            let distance = |slot: Slot| {
                let dx = i64::from(slot.0) - i64::from(target.0);
                let dy = i64::from(slot.1) - i64::from(target.1);
                dx * dx + dy * dy
            };
            let nearest = (0..cols as u32)
                .flat_map(|c| (0..rows as u32).map(move |r| (c, r)))
                .filter(|slot| !occupied.contains(slot))
                .min_by_key(|&slot| distance(slot))
                .unwrap_or_else(|| self.slot(i));
            occupied.insert(nearest);
            self.saved.insert(file_name(&self.items[i]), nearest);
        }
        self.layout();
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
        let selected = self.is_selected(i) || self.drop_target == Some(i);
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
        if self.renaming.as_ref() == Some(&item.path) {
            return;
        }
        let lines = label_lines(fonts, &item.name, &LABEL, LABEL_W);
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

/// Подпись в две строки шириной `width`: первая — сколько влезет (по
/// словам, а длинное слово — по буквам), вторая — остальное, с «…», если не
/// влезло. Общая для стола и окна папки.
pub fn label_lines(fonts: &mut Fonts, text: &str, style: &Style, width: f32) -> Vec<String> {
    if fonts.measure(text, style) <= width {
        return vec![text.to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    // Сколько букв влезает в первую строку.
    let mut fit = 0;
    for n in 1..=chars.len() {
        let part: String = chars[..n].iter().collect();
        if fonts.measure(&part, style) > width {
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
            size: 0,
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
        assert_eq!(
            g.cell(2),
            g.slot_rect((0, 2)),
            "«в» не затекает на место «б»"
        );
        // На занятое место — в ближайшую свободную клетку.
        g.move_item(2, (2, 0));
        assert_ne!(g.cell(2), g.slot_rect((2, 0)));
        assert_eq!(
            g.saved().len(),
            3,
            "передвинули руками — места всех запомнены"
        );
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
    fn rubber_band_and_group_move() {
        let mut g = IconGrid::new((0.0, 0.0, 4.0 * CELL_W, 3.0 * CELL_H));
        g.set_items(vec![item("а"), item("б"), item("в"), item("г")]);
        // Рамка по первым двум значкам колонки (справа сверху).
        let (x, y, _, _) = g.cell(0);
        let band = (x + 20.0, y + 10.0, 40.0, CELL_H + 20.0);
        let changed = g.set_selection(g.items_in(band));
        assert_eq!(changed, [0, 1]);
        assert_eq!(g.selected_items().len(), 2);
        assert!(g.selected().is_none(), "не один — переименовать нечего");
        // Сдвинули группу на две колонки левее (там свободно).
        g.move_items(0, (2, 0), &[0, 1]);
        assert_eq!((g.slot(0), g.slot(1)), ((2, 0), (2, 1)));
        assert_eq!(g.slot(3), (1, 0), "«г» на месте");
        // Новый список — выделение по путям сохраняется.
        g.set_items(vec![item("а"), item("б"), item("в"), item("г"), item("д")]);
        assert_eq!(g.selected_indices(), [0, 1]);
        assert_eq!(g.select(None), [0, 1]);
    }

    #[test]
    fn group_move_avoids_stationary_icons_and_keeps_unique_slots() {
        let mut g = IconGrid::new((0.0, 0.0, 6.0 * CELL_W, 5.0 * CELL_H));
        g.set_items((0..20).map(|i| item(&format!("file{i}"))).collect());
        let pinned = g.slot(5);
        // Перемещаем выбранные значки на уже занятую неподвижным значком
        // клетку. Даже при столкновениях места не должны дублироваться.
        g.move_items(0, pinned, &[0, 1, 2, 3]);
        assert_eq!(g.slot(5), pinned);
        let mut unique = HashSet::new();
        for i in 0..g.items().len() {
            assert!(unique.insert(g.slot(i)), "повторяющаяся клетка для {i}");
        }
    }

    #[test]
    fn removed_icon_leaves_hole() {
        let mut g = IconGrid::new((0.0, 0.0, 4.0 * CELL_W, 3.0 * CELL_H));
        g.set_items(vec![item("а"), item("б"), item("в"), item("г")]);
        let (b, v) = (g.slot(1), g.slot(2));
        // «а» унесли в папку: остальные стоят, где стояли.
        g.freeze();
        g.set_items(vec![item("б"), item("в"), item("г")]);
        assert_eq!((g.slot(0), g.slot(1)), (b, v));
    }

    #[test]
    fn arrows_select_all_and_prefix() {
        let mut g = IconGrid::new((0.0, 0.0, 4.0 * CELL_W, 3.0 * CELL_H));
        g.set_items(vec![
            item("а"),
            item("б"),
            item("в"),
            item("Гора"),
            item("Город"),
        ]);
        // Колонка справа: а, б, в; левее: Гора, Город.
        assert_eq!(g.neighbor(None, Direction::Down), Some(0));
        assert_eq!(g.neighbor(Some(0), Direction::Down), Some(1));
        assert_eq!(g.neighbor(Some(0), Direction::Up), None);
        assert_eq!(g.neighbor(Some(0), Direction::Left), Some(3));
        assert_eq!(g.neighbor(Some(4), Direction::Right), Some(1));
        assert_eq!(g.select_all().len(), 5);
        assert_eq!(g.find_prefix("го", None), Some(3));
        assert_eq!(g.find_prefix("го", Some(3)), Some(4), "дальше по кругу");
        assert_eq!(g.find_prefix("я", None), None);
        assert_eq!(g.set_drop_target(Some(2)), [2]);
        assert_eq!(g.set_drop_target(None), [2]);
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
        assert_eq!(
            label_lines(&mut fonts, "Проекты", &LABEL, LABEL_W),
            ["Проекты"]
        );
        let lines = label_lines(
            &mut fonts,
            "Очень длинное название документа.pdf",
            &LABEL,
            LABEL_W,
        );
        assert_eq!(lines.len(), 2);
        assert!(fonts.measure(&lines[0], &LABEL) <= LABEL_W);
        assert_eq!(lines[0], "Очень", "по пробелу, не посреди слова");
    }
}
