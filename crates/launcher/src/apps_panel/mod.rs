//! Окно «Программы»: все программы сеткой по страницам и поле поиска сверху.
//! Состояние и реакция на клавиши и мышь; как оно выглядит — в `draw`.
//!
//! Как на Mac: все программы сеткой по страницам (по алфавиту), сверху поле
//! поиска — набор текста сразу отбирает программы. Стрелки выбирают иконку,
//! Enter запускает; Esc сначала стирает поиск, затем закрывает; клик по пустому
//! месту закрывает; страницы листаются колёсиком, PageUp/PageDown и точками внизу.
//! Подсветки при наведении мыши нет — как на Mac (и не нужно перерисовывать
//! весь экран на каждое движение).

use mimshel_apps::App;
use mimshel_core::input::{Key, KeyPress};

mod draw;

pub use draw::backdrop;

use crate::Action;
use mimshel_widgets::field::{Edit, TextField};

/// Размер сетки (из настроек).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub columns: usize,
    pub rows: usize,
    /// Размер иконки в пикселях.
    pub icon: u32,
}

/// Где что лежит на экране.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Layout {
    pub w: f32,
    pub h: f32,
    /// Поле поиска.
    pub search: (f32, f32, f32, f32),
    /// Левый верхний угол сетки и размер клетки.
    pub grid_x: f32,
    pub grid_y: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    /// Высота строки точек страниц (центр).
    pub dots_y: f32,
}

pub(crate) const SEARCH_W: f32 = 260.0;
pub(crate) const SEARCH_H: f32 = 32.0;
/// Расстояние между точками страниц.
pub(crate) const DOT_STEP: f32 = 18.0;

impl Layout {
    fn new(w: u32, h: u32, grid: Grid) -> Layout {
        let (w, h) = (w as f32, h as f32);
        let grid_x = (w * 0.10).round();
        let grid_y = (h * 0.13).round();
        Layout {
            w,
            h,
            search: (
                ((w - SEARCH_W) / 2.0).round(),
                (h * 0.045).round(),
                SEARCH_W,
                SEARCH_H,
            ),
            grid_x,
            grid_y,
            cell_w: (w - grid_x * 2.0) / grid.columns as f32,
            cell_h: (h * 0.87 - grid_y) / grid.rows as f32,
            dots_y: (h * 0.93).round(),
        }
    }
}

pub struct AppsPanel {
    apps: Vec<App>,
    /// Поля `apps`, подготовленные для поиска один раз (см. [`prepare_all`]).
    prepared: Vec<mimshel_core::search::Prepared>,
    pub(crate) field: TextField,
    /// Что показано: номера программ в `apps` — все по алфавиту или найденные.
    shown: Vec<usize>,
    pub(crate) page: usize,
    /// Выбранная иконка (номер в `shown`). Пока не трогали стрелки — нет.
    pub(crate) selected: Option<usize>,
    pub(crate) grid: Grid,
    pub(crate) layout: Layout,
}

impl AppsPanel {
    pub fn new(apps: Vec<App>, screen: (u32, u32), grid: Grid) -> AppsPanel {
        let prepared = crate::prepare_all(&apps);
        let mut l = AppsPanel {
            apps,
            prepared,
            field: TextField::default(),
            shown: Vec::new(),
            page: 0,
            selected: None,
            grid,
            layout: Layout::new(screen.0, screen.1, grid),
        };
        l.reset();
        l
    }

    /// Новый список программ (после установки или удаления).
    pub fn set_apps(&mut self, apps: Vec<App>) {
        self.prepared = crate::prepare_all(&apps);
        self.apps = apps;
        self.search();
    }

    /// Размер экрана поменялся.
    pub fn set_screen(&mut self, w: u32, h: u32) {
        self.layout = Layout::new(w, h, self.grid);
    }

    pub fn size(&self) -> (u32, u32) {
        (self.layout.w as u32, self.layout.h as u32)
    }

    /// Пустой поиск, первая страница — так окно открывается каждый раз.
    pub fn reset(&mut self) {
        self.field.clear();
        self.search();
    }

    pub fn query(&self) -> &str {
        self.field.text()
    }

    pub(crate) fn per_page(&self) -> usize {
        self.grid.columns * self.grid.rows
    }

    pub fn pages(&self) -> usize {
        self.shown.len().div_ceil(self.per_page()).max(1)
    }

    /// Программы текущей страницы вместе с их номерами в `shown`.
    pub(crate) fn page_apps(&self) -> impl Iterator<Item = (usize, &App)> {
        let start = self.page * self.per_page();
        self.shown
            .iter()
            .enumerate()
            .skip(start)
            .take(self.per_page())
            .map(|(i, &a)| (i, &self.apps[a]))
    }

    pub fn shown(&self) -> impl Iterator<Item = &App> {
        self.shown.iter().map(|&i| &self.apps[i])
    }

    pub fn key(&mut self, k: KeyPress) -> Action {
        let cols = self.grid.columns as isize;
        match k.key {
            Key::Escape if !self.field.is_empty() => {
                self.reset();
                Action::Redraw
            }
            Key::Escape => Action::Close,
            // Повторное Super+A закрывает (клавиатура у окна, до Xfce не дойдёт).
            Key::Char('a') if k.logo => Action::Close,
            Key::Return => self.launch_selected(),
            Key::Right => self.move_selection(1),
            Key::Left => self.move_selection(-1),
            Key::Down => self.move_selection(cols),
            Key::Up => self.move_selection(-cols),
            Key::Tab if k.shift => self.move_selection(-1),
            Key::Tab => self.move_selection(1),
            Key::PageDown => self.turn(1),
            Key::PageUp => self.turn(-1),
            _ => match self.field.key(k) {
                Edit::Changed => {
                    self.search();
                    Action::Redraw
                }
                Edit::Moved => Action::Redraw,
                Edit::Ignored => Action::Nothing,
            },
        }
    }

    /// Нажатие кнопки мыши (координаты экрана).
    pub fn click(&mut self, x: i32, y: i32, button: u8) -> Action {
        let (x, y) = (x as f32, y as f32);
        match button {
            1 => {
                if let Some(i) = self.icon_at(x, y) {
                    self.selected = Some(i);
                    return self.launch_selected();
                }
                if let Some(p) = self.dot_at(x, y) {
                    return self.go_to(p);
                }
                let (sx, sy, sw, sh) = self.layout.search;
                if x >= sx && x < sx + sw && y >= sy && y < sy + sh {
                    return Action::Nothing;
                }
                // Клик по пустому месту закрывает, как на Mac.
                Action::Close
            }
            // Колёсико и наклон колёсика — листать страницы.
            4 | 6 => self.turn(-1),
            5 | 7 => self.turn(1),
            _ => Action::Nothing,
        }
    }

    /// Номер (в `shown`) иконки под точкой экрана.
    fn icon_at(&self, x: f32, y: f32) -> Option<usize> {
        let l = &self.layout;
        let col = ((x - l.grid_x) / l.cell_w).floor();
        let row = ((y - l.grid_y) / l.cell_h).floor();
        if col < 0.0 || row < 0.0 {
            return None;
        }
        let (col, row) = (col as usize, row as usize);
        if col >= self.grid.columns || row >= self.grid.rows {
            return None;
        }
        // Попадать надо в саму иконку с подписью, а не в любое место клетки.
        let (bx, by, bw, bh) = self.cell_box(row * self.grid.columns + col);
        if x < bx || x >= bx + bw || y < by || y >= by + bh {
            return None;
        }
        let i = self.page * self.per_page() + row * self.grid.columns + col;
        (i < self.shown.len()).then_some(i)
    }

    /// Подложка иконки с подписью в клетке `slot` текущей страницы.
    pub(crate) fn cell_box(&self, slot: usize) -> (f32, f32, f32, f32) {
        let l = &self.layout;
        let (col, row) = (slot % self.grid.columns, slot / self.grid.columns);
        let icon = self.grid.icon as f32;
        // Шире иконки: подписи «Расширенная конфигурация…» нужно место.
        let w = (icon * 2.6).min(l.cell_w - 16.0);
        let h = icon + 52.0;
        let x = l.grid_x + col as f32 * l.cell_w + (l.cell_w - w) / 2.0;
        let y = l.grid_y + row as f32 * l.cell_h + (l.cell_h - h) / 2.0;
        (x.round(), y.round(), w.round(), h.round())
    }

    /// Центр точки страницы `p`.
    pub(crate) fn dot_center(&self, p: usize) -> (f32, f32) {
        let n = self.pages() as f32;
        let x = self.layout.w / 2.0 - (n - 1.0) * DOT_STEP / 2.0 + p as f32 * DOT_STEP;
        (x, self.layout.dots_y)
    }

    fn dot_at(&self, x: f32, y: f32) -> Option<usize> {
        if self.pages() < 2 {
            return None;
        }
        (0..self.pages()).find(|&p| {
            let (cx, cy) = self.dot_center(p);
            (x - cx).abs() <= DOT_STEP / 2.0 && (y - cy).abs() <= DOT_STEP / 2.0
        })
    }

    fn launch_selected(&self) -> Action {
        // В поиске Enter без выбора запускает первую найденную программу.
        let i = match self.selected {
            Some(i) => i,
            None if !self.field.is_empty() => 0,
            None => return Action::Nothing,
        };
        match self.shown.get(i) {
            Some(&a) => Action::Launch(Box::new(self.apps[a].clone())),
            None => Action::Nothing,
        }
    }

    fn move_selection(&mut self, by: isize) -> Action {
        if self.shown.is_empty() {
            return Action::Nothing;
        }
        let last = self.shown.len() as isize - 1;
        let i = match self.selected {
            // Первое нажатие стрелки выбирает первую иконку страницы.
            None => (self.page * self.per_page()) as isize,
            Some(i) => (i as isize + by).clamp(0, last),
        } as usize;
        if self.selected == Some(i) {
            return Action::Nothing;
        }
        self.selected = Some(i);
        self.page = i / self.per_page();
        Action::Redraw
    }

    fn turn(&mut self, by: isize) -> Action {
        let p = (self.page as isize + by).clamp(0, self.pages() as isize - 1) as usize;
        self.go_to(p)
    }

    fn go_to(&mut self, page: usize) -> Action {
        if page == self.page {
            return Action::Nothing;
        }
        self.page = page;
        self.selected = None;
        Action::Redraw
    }

    fn search(&mut self) {
        self.shown = if self.field.is_empty() {
            (0..self.apps.len()).collect()
        } else {
            crate::rank(&self.prepared, self.field.text())
        };
        self.page = 0;
        // Во время поиска первая найденная программа выбрана сразу.
        self.selected = (!self.field.is_empty() && !self.shown.is_empty()).then_some(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search_panel::tests::app;

    const GRID: Grid = Grid {
        columns: 3,
        rows: 2,
        icon: 72,
    };

    /// 8 программ на сетке 3×2 — две страницы.
    fn panel() -> AppsPanel {
        let names = [
            "Блокнот",
            "Калькулятор",
            "Терминал",
            "Firefox",
            "Files",
            "Vim",
            "Meld",
            "Тест",
        ];
        AppsPanel::new(names.iter().map(|n| app(n)).collect(), (1200, 800), GRID)
    }

    fn press(l: &mut AppsPanel, key: Key) -> Action {
        l.key(KeyPress::plain(key))
    }

    fn names(l: &AppsPanel) -> Vec<&str> {
        l.shown().map(|a| a.name.as_str()).collect()
    }

    fn center(l: &AppsPanel, slot: usize) -> (i32, i32) {
        let (x, y, w, h) = l.cell_box(slot);
        ((x + w / 2.0) as i32, (y + h / 2.0) as i32)
    }

    #[test]
    fn pages_and_arrows() {
        let mut l = panel();
        assert_eq!(l.pages(), 2);
        assert_eq!(l.selected, None);
        assert_eq!(press(&mut l, Key::Return), Action::Nothing);
        press(&mut l, Key::Right);
        assert_eq!(l.selected, Some(0));
        press(&mut l, Key::Down);
        assert_eq!(l.selected, Some(3));
        press(&mut l, Key::Down);
        // Ушли на вторую страницу вслед за выбором.
        assert_eq!((l.selected, l.page), (Some(6), 1));
        press(&mut l, Key::Down);
        assert_eq!(l.selected, Some(7)); // дальше последней не уходит
        assert_eq!(press(&mut l, Key::PageUp), Action::Redraw);
        assert_eq!((l.selected, l.page), (None, 0));
    }

    #[test]
    fn search_filters_and_enter_launches_first() {
        let mut l = panel();
        for c in "fi".chars() {
            l.key(KeyPress::plain(Key::Char(c)));
        }
        assert_eq!(names(&l), ["Firefox", "Files"]);
        assert_eq!(l.pages(), 1);
        match press(&mut l, Key::Return) {
            Action::Launch(a) => assert_eq!(a.name, "Firefox"),
            other => panic!("{other:?}"),
        }
        assert_eq!(press(&mut l, Key::Escape), Action::Redraw);
        assert_eq!(names(&l).len(), 8);
        assert_eq!(press(&mut l, Key::Escape), Action::Close);
    }

    #[test]
    fn mouse() {
        let mut l = panel();
        let (x, y) = center(&l, 1);
        match l.click(x, y, 1) {
            Action::Launch(a) => assert_eq!(a.name, "Калькулятор"),
            other => panic!("{other:?}"),
        }
        assert_eq!(l.click(5, 5, 1), Action::Close);
        let (sx, sy, _, _) = l.layout.search;
        assert_eq!(l.click(sx as i32 + 5, sy as i32 + 5, 1), Action::Nothing);
        assert_eq!(l.click(x, y, 5), Action::Redraw);
        assert_eq!(l.page, 1);
        let (dx, dy) = l.dot_center(0);
        assert_eq!(l.click(dx as i32, dy as i32, 1), Action::Redraw);
        assert_eq!(l.page, 0);
        // Пустая клетка второй страницы — мимо иконок, закрывает.
        l.click(x, y, 5);
        let (ex, ey) = center(&l, 5);
        assert_eq!(l.click(ex, ey, 1), Action::Close);
    }

    #[test]
    fn super_a_closes() {
        let mut l = panel();
        let super_a = KeyPress {
            logo: true,
            ..KeyPress::plain(Key::Char('a'))
        };
        assert_eq!(l.key(super_a), Action::Close);
        assert!(l.query().is_empty());
    }
}
