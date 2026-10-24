//! Окно папки без X: где что нарисовано, что под указателем, выделение,
//! прокрутка, история «назад/вперёд», клавиши — и картинка окна целиком.
//!
//! Раскладка, как в Finder: сверху панель с кнопками ‹ › ↑ и названием
//! папки, слева места, справа значки строками слева направо (папки — первыми),
//! внизу строка состояния. Всё в точках; холст сам переводит в пиксели.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use shell_core::input::{Key, KeyPress, latin};
use shell_desktop::files::{Item, Kind};
use shell_desktop::icon_grid::label_lines;
use shell_icons::Icons;
use shell_render::Canvas;
use shell_text::{Fonts, Style};
use shell_widgets::shape::{paint, rounded};
use tiny_skia::{FillRule, FilterQuality, Pixmap, PixmapPaint, Transform};

use crate::places::Place;

/// Размер окна при открытии и наименьший.
pub const DEFAULT_SIZE: (u32, u32) = (900, 560);
pub const MIN_SIZE: (u32, u32) = (520, 320);

const TOOLBAR_H: f32 = 48.0;
const SIDEBAR_W: f32 = 184.0;
const STATUS_H: f32 = 26.0;
/// Клетка значка (ширина растягивается, чтобы строка заполняла окно).
const CELL_W: f32 = 104.0;
const CELL_H: f32 = 100.0;
/// Картинка значка файла.
pub const ICON: u32 = 52;
const PAD: f32 = 12.0;
const ICON_TOP: f32 = 6.0;
const LABEL_TOP: f32 = ICON_TOP + ICON as f32 + 5.0;
/// Строка в боковой панели и где начинается список мест.
const ROW_H: f32 = 28.0;
const PLACES_TOP: f32 = TOOLBAR_H + 36.0;
/// Кнопки ‹ › ↑ на панели.
const BUTTON: f32 = 28.0;
const BUTTONS_X: f32 = 12.0;
const BUTTON_GAP: f32 = 4.0;
const SIDE_ICON: u32 = 16;
/// Колёсико мыши — на столько точек.
const SCROLL_STEP: f32 = 56.0;
const DOUBLE_CLICK: Duration = Duration::from_millis(450);
const DRAG_START: f32 = 5.0;
const TYPE_AGAIN: Duration = Duration::from_millis(1000);

const BG: [u8; 4] = [0x1e, 0x1e, 0x1e, 0xff];
const SIDEBAR_BG: [u8; 4] = [0x26, 0x26, 0x28, 0xff];
const TOOLBAR_BG: [u8; 4] = [0x2c, 0x2c, 0x2e, 0xff];
const LINE: [u8; 4] = [0x00, 0x00, 0x00, 0x80];
const HOVER: [u8; 4] = [0xff, 0xff, 0xff, 0x14];
const CURRENT: [u8; 4] = [0xff, 0xff, 0xff, 0x24];
const ICON_SELECTED: [u8; 4] = [0xff, 0xff, 0xff, 0x22];
const LABEL_SELECTED: [u8; 4] = [0x0a, 0x64, 0xd6, 0xff];
const SCROLLBAR: [u8; 4] = [0xff, 0xff, 0xff, 0x50];
const BAND_FILL: [u8; 4] = [0xff, 0xff, 0xff, 0x22];
const BAND_EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x90];
const SIDE_ICON_COLOR: [u8; 4] = [0x4c, 0x9b, 0xff, 0xff];
const BUTTON_COLOR: [u8; 4] = [0xe0, 0xe0, 0xe0, 0xff];
const BUTTON_OFF: [u8; 4] = [0xe0, 0xe0, 0xe0, 0x50];

const TITLE: Style = Style {
    size: 15.0,
    color: [0xff, 0xff, 0xff, 0xff],
    weight: 700,
};
const LABEL: Style = Style {
    size: 12.5,
    color: [0xf0, 0xf0, 0xf0, 0xff],
    weight: 400,
};
const SIDE: Style = Style {
    size: 13.0,
    color: [0xe8, 0xe8, 0xe8, 0xff],
    weight: 400,
};
const HEADING: Style = Style {
    size: 11.0,
    color: [0xff, 0xff, 0xff, 0x80],
    weight: 700,
};
const STATUS: Style = Style {
    size: 12.0,
    color: [0xff, 0xff, 0xff, 0xa0],
    weight: 400,
};
const EMPTY: Style = Style {
    size: 14.0,
    color: [0xff, 0xff, 0xff, 0x60],
    weight: 400,
};

/// Прямоугольник в точках окна: (x, y, ширина, высота).
pub type Rect = (f32, f32, f32, f32);

/// Что под указателем.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Back,
    Forward,
    Up,
    Place(usize),
    Item(usize),
    /// Пустое место среди значков.
    Empty,
    Nothing,
}

/// Что сделать службе после события.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Nothing,
    Redraw,
    /// Перешли в другую папку: прочитать её и следить за ней.
    Load,
    /// Открыть программами (ярлык — запустить).
    Open(Vec<Item>),
    /// Меню по правому щелчку в точке окна: на значке или на пустом месте.
    Menu {
        x: f32,
        y: f32,
        on_item: bool,
    },
    /// Начали тащить выделенные значки; дальше — где указатель и где
    /// отпустили (в точках окна).
    DragStart,
    DragMove {
        x: f32,
        y: f32,
    },
    Drop {
        x: f32,
        y: f32,
    },
    Rename(PathBuf),
    Trash(Vec<PathBuf>),
    Copy(Vec<PathBuf>),
    Cut(Vec<PathBuf>),
    Paste,
    NewFolder,
    NewWindow,
    Close,
}

/// Нажатие кнопки мыши: на значке (`index`) или на пустом месте.
struct Press {
    index: usize,
    at: (f32, f32),
    dragging: bool,
    /// Нажали на один из нескольких выделенных: не потащили — останется он.
    collapse: bool,
}

pub struct FolderView {
    dir: PathBuf,
    items: Vec<Item>,
    places: Vec<Place>,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    size: (f32, f32),
    scroll: f32,
    /// Выделенные (по возрастанию) и с какого начинать выделение с Shift.
    selected: Vec<usize>,
    anchor: Option<usize>,
    hover: Hit,
    press: Option<Press>,
    /// Рамка выделения в точках окна: где начали и где указатель.
    band: Option<((f32, f32), (f32, f32))>,
    /// Папка под переносимыми файлами — подсвечена.
    drop_target: Option<usize>,
    /// Значок, который переименовывают: подпись не рисуется (поверх — поле).
    renaming: Option<PathBuf>,
    /// Список папки прочитан (до того — не «Папка пуста»).
    loaded: bool,
    last_click: Option<(usize, Instant)>,
    typed: String,
    typed_at: Instant,
    /// Выделить, когда появится в списке (создан или переименован; после
    /// «назад» — папка, из которой вышли).
    pending_select: Option<PathBuf>,
}

impl FolderView {
    pub fn new(dir: PathBuf, places: Vec<Place>, (w, h): (u32, u32)) -> FolderView {
        FolderView {
            dir,
            items: Vec::new(),
            places,
            back: Vec::new(),
            forward: Vec::new(),
            size: (w as f32, h as f32),
            scroll: 0.0,
            selected: Vec::new(),
            anchor: None,
            hover: Hit::Nothing,
            press: None,
            band: None,
            drop_target: None,
            renaming: None,
            loaded: false,
            last_click: None,
            typed: String::new(),
            typed_at: Instant::now(),
            pending_select: None,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Заголовок окна — имя папки (у корня — «/»).
    pub fn title(&self) -> String {
        match self.dir.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => self.dir.display().to_string(),
        }
    }

    /// Список папки уже прочитан.
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn selected_items(&self) -> Vec<Item> {
        self.selected
            .iter()
            .filter_map(|&i| self.items.get(i).cloned())
            .collect()
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_items().into_iter().map(|i| i.path).collect()
    }

    /// Новый список файлов папки (сразу после перехода и после перемен).
    /// Выделение остаётся на тех же файлах.
    pub fn set_items(&mut self, items: Vec<Item>) {
        let keep = self.selected_paths();
        self.items = items;
        self.selected = (0..self.items.len())
            .filter(|&i| keep.contains(&self.items[i].path))
            .collect();
        self.loaded = true;
        self.drop_target = None;
        self.last_click = None;
        if let Some(path) = self.pending_select.take() {
            match self.items.iter().position(|i| i.path == path) {
                Some(i) => {
                    self.select(Some(i));
                    self.ensure_visible(i);
                }
                None => self.pending_select = Some(path),
            }
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }

    /// Выделить файл, когда он появится в списке.
    pub fn select_later(&mut self, path: PathBuf) {
        self.pending_select = Some(path);
    }

    pub fn set_renaming(&mut self, path: Option<PathBuf>) {
        self.renaming = path;
    }

    pub fn set_size(&mut self, (w, h): (u32, u32)) -> bool {
        let size = (w as f32, h as f32);
        if size == self.size {
            return false;
        }
        self.size = size;
        self.scroll = self.scroll.min(self.max_scroll());
        true
    }

    /// Перейти в папку `path` (запоминается для «назад»).
    pub fn navigate(&mut self, path: PathBuf) -> Action {
        if path == self.dir {
            return Action::Nothing;
        }
        let old = std::mem::replace(&mut self.dir, path);
        self.back.push(old);
        self.forward.clear();
        self.entered();
        Action::Load
    }

    fn go_back(&mut self) -> Action {
        let Some(prev) = self.back.pop() else {
            return Action::Nothing;
        };
        let old = std::mem::replace(&mut self.dir, prev);
        self.forward.push(old.clone());
        self.entered();
        self.pending_select = Some(old).filter(|o| o.parent() == Some(&self.dir));
        Action::Load
    }

    fn go_forward(&mut self) -> Action {
        let Some(next) = self.forward.pop() else {
            return Action::Nothing;
        };
        let old = std::mem::replace(&mut self.dir, next);
        self.back.push(old);
        self.entered();
        Action::Load
    }

    /// В папку выше; выделена будет та, из которой вышли.
    fn go_up(&mut self) -> Action {
        let Some(parent) = self.dir.parent().map(Path::to_path_buf) else {
            return Action::Nothing;
        };
        let from = self.dir.clone();
        let action = self.navigate(parent);
        self.pending_select = Some(from);
        action
    }

    /// Только что перешли: список ещё не прочитан.
    fn entered(&mut self) {
        self.items.clear();
        self.selected.clear();
        self.anchor = None;
        self.scroll = 0.0;
        self.loaded = false;
        self.press = None;
        self.band = None;
        self.pending_select = None;
        self.hover = Hit::Nothing;
    }

    // ---- Раскладка ----

    /// Где значки: (x, y, ширина, высота).
    fn content(&self) -> Rect {
        let (w, h) = self.size;
        (
            SIDEBAR_W,
            TOOLBAR_H,
            (w - SIDEBAR_W).max(1.0),
            (h - TOOLBAR_H - STATUS_H).max(1.0),
        )
    }

    fn columns(&self) -> usize {
        let (_, _, cw, _) = self.content();
        (((cw - 2.0 * PAD) / CELL_W).floor() as usize).max(1)
    }

    fn cell_w(&self) -> f32 {
        let (_, _, cw, _) = self.content();
        ((cw - 2.0 * PAD) / self.columns() as f32).max(CELL_W.min(cw))
    }

    fn max_scroll(&self) -> f32 {
        let rows = self.items.len().div_ceil(self.columns());
        let (_, _, _, ch) = self.content();
        (2.0 * PAD + rows as f32 * CELL_H - ch).max(0.0)
    }

    /// Клетка значка `i` в точках окна (с учётом прокрутки).
    pub fn cell(&self, i: usize) -> Rect {
        let (cx, cy, _, _) = self.content();
        let cols = self.columns();
        let cw = self.cell_w();
        let (col, row) = (i % cols, i / cols);
        (
            cx + PAD + col as f32 * cw,
            cy + PAD + row as f32 * CELL_H - self.scroll,
            cw,
            CELL_H,
        )
    }

    /// Где подпись значка `i` (для поля переименования).
    pub fn label_rect(&self, i: usize) -> Rect {
        let (x, y, w, _) = self.cell(i);
        (x, y + LABEL_TOP, w, LABEL.line_height())
    }

    /// Где значок виден: картинка и две строки подписи.
    fn item_rect(&self, i: usize) -> Rect {
        let (x, y, w, _) = self.cell(i);
        (
            x + 4.0,
            y + ICON_TOP - 2.0,
            w - 8.0,
            LABEL_TOP - ICON_TOP + 2.0 + 2.0 * LABEL.line_height(),
        )
    }

    fn in_content(&self, x: f32, y: f32) -> bool {
        let (cx, cy, cw, ch) = self.content();
        x >= cx && x < cx + cw && y >= cy && y < cy + ch
    }

    /// Значок под точкой: картинка или подпись, не вся клетка.
    pub fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.in_content(x, y) {
            return None;
        }
        let (cx, cy, _, _) = self.content();
        let col = ((x - cx - PAD) / self.cell_w()).floor();
        let row = ((y - cy - PAD + self.scroll) / CELL_H).floor();
        if col < 0.0 || row < 0.0 || col as usize >= self.columns() {
            return None;
        }
        let i = row as usize * self.columns() + col as usize;
        if i >= self.items.len() {
            return None;
        }
        let (ix, iy, iw, ih) = self.cell(i);
        let icon_x = ix + (iw - ICON as f32) / 2.0 - 4.0;
        let on_icon = x >= icon_x
            && x < icon_x + ICON as f32 + 8.0
            && y >= iy + ICON_TOP - 2.0
            && y < iy + LABEL_TOP;
        let on_label = x >= ix + 4.0
            && x < ix + iw - 4.0
            && y >= iy + LABEL_TOP
            && y < (iy + LABEL_TOP + 2.0 * LABEL.line_height()).min(iy + ih);
        (on_icon || on_label).then_some(i)
    }

    fn button_rect(n: usize) -> Rect {
        (
            BUTTONS_X + n as f32 * (BUTTON + BUTTON_GAP),
            (TOOLBAR_H - BUTTON) / 2.0,
            BUTTON,
            BUTTON,
        )
    }

    fn place_rect(&self, n: usize) -> Rect {
        (8.0, PLACES_TOP + n as f32 * ROW_H, SIDEBAR_W - 16.0, ROW_H)
    }

    pub fn hit(&self, x: f32, y: f32) -> Hit {
        let inside = |(rx, ry, rw, rh): Rect| x >= rx && x < rx + rw && y >= ry && y < ry + rh;
        if y < TOOLBAR_H {
            return [Hit::Back, Hit::Forward, Hit::Up]
                .into_iter()
                .enumerate()
                .find(|&(n, _)| inside(Self::button_rect(n)))
                .map_or(Hit::Nothing, |(_, h)| h);
        }
        if x < SIDEBAR_W {
            return (0..self.places.len())
                .find(|&n| inside(self.place_rect(n)))
                .map_or(Hit::Nothing, Hit::Place);
        }
        if !self.in_content(x, y) {
            return Hit::Nothing;
        }
        self.item_at(x, y).map_or(Hit::Empty, Hit::Item)
    }

    /// Папка под точкой (не из переносимых) — сюда упадут файлы.
    pub fn folder_at(&self, x: f32, y: f32, except_selected: bool) -> Option<usize> {
        self.item_at(x, y)
            .filter(|i| !(except_selected && self.selected.contains(i)))
            .filter(|&i| self.items[i].kind == Kind::Folder)
    }

    /// Подсветить папку, куда упадут файлы (`None` — снять). `true` —
    /// поменялось, перерисовать.
    pub fn set_drop_target(&mut self, i: Option<usize>) -> bool {
        std::mem::replace(&mut self.drop_target, i) != i
    }

    /// Место в боковой панели под точкой (бросить файлы туда).
    pub fn place_at(&self, x: f32, y: f32) -> Option<&Path> {
        match self.hit(x, y) {
            Hit::Place(n) => self.places.get(n).map(|p| p.path.as_path()),
            _ => None,
        }
    }

    // ---- Выделение ----

    fn select(&mut self, i: Option<usize>) {
        self.selected = i.into_iter().collect();
        self.anchor = i;
    }

    fn set_selection(&mut self, mut list: Vec<usize>) {
        list.sort_unstable();
        list.dedup();
        self.selected = list;
    }

    fn select_all(&mut self) {
        self.selected = (0..self.items.len()).collect();
    }

    fn ensure_visible(&mut self, i: usize) {
        let (_, cy, _, ch) = self.content();
        let (_, y, _, h) = self.cell(i);
        if y < cy {
            self.scroll -= cy - y + PAD;
        } else if y + h > cy + ch {
            self.scroll += y + h - (cy + ch) + PAD;
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    pub fn scroll_by(&mut self, dy: f32) -> bool {
        let new = (self.scroll + dy).clamp(0.0, self.max_scroll());
        std::mem::replace(&mut self.scroll, new) != new
    }

    /// Открыть выделенное: папку — в этом же окне, остальное — программами.
    fn open_selected(&mut self) -> Action {
        let items = self.selected_items();
        if let Some(folder) = items.iter().find(|i| i.kind == Kind::Folder) {
            let path = folder.path.clone();
            return self.navigate(path);
        }
        if items.is_empty() {
            Action::Nothing
        } else {
            Action::Open(items)
        }
    }

    // ---- Мышь ----

    /// Нажали кнопку мыши в точке окна (Ctrl, Shift — зажаты ли).
    pub fn press(&mut self, x: f32, y: f32, button: u8, ctrl: bool, shift: bool) -> Action {
        match button {
            4 => return redraw_if(self.scroll_by(-SCROLL_STEP)),
            5 => return redraw_if(self.scroll_by(SCROLL_STEP)),
            // Боковые кнопки мыши «назад» и «вперёд».
            8 => return self.go_back(),
            9 => return self.go_forward(),
            1 | 3 => {}
            _ => return Action::Nothing,
        }
        let hit = self.hit(x, y);
        if button == 3 {
            return match hit {
                Hit::Item(i) => {
                    if !self.selected.contains(&i) {
                        self.select(Some(i));
                    }
                    Action::Menu {
                        x,
                        y,
                        on_item: true,
                    }
                }
                Hit::Empty => {
                    self.select(None);
                    Action::Menu {
                        x,
                        y,
                        on_item: false,
                    }
                }
                _ => Action::Nothing,
            };
        }
        match hit {
            Hit::Back => self.go_back(),
            Hit::Forward => self.go_forward(),
            Hit::Up => self.go_up(),
            Hit::Place(n) => {
                let path = self.places[n].path.clone();
                self.navigate(path)
            }
            Hit::Item(i) if ctrl => {
                match self.selected.iter().position(|&s| s == i) {
                    Some(p) => {
                        self.selected.remove(p);
                    }
                    None => self.set_selection([self.selected.clone(), vec![i]].concat()),
                }
                self.anchor = Some(i);
                self.last_click = None;
                Action::Redraw
            }
            Hit::Item(i) if shift => {
                let from = self.anchor.unwrap_or(i);
                self.set_selection((from.min(i)..=from.max(i)).collect());
                self.last_click = None;
                Action::Redraw
            }
            Hit::Item(i) => {
                let now = Instant::now();
                let double = self
                    .last_click
                    .is_some_and(|(j, t)| j == i && now - t < DOUBLE_CLICK);
                if double {
                    self.last_click = None;
                    return self.open_selected();
                }
                self.last_click = Some((i, now));
                let keep = self.selected.contains(&i) && self.selected.len() > 1;
                if !keep {
                    self.select(Some(i));
                }
                self.press = Some(Press {
                    index: i,
                    at: (x, y),
                    dragging: false,
                    collapse: keep,
                });
                Action::Redraw
            }
            Hit::Empty => {
                if !ctrl && !shift {
                    self.select(None);
                }
                self.last_click = None;
                self.band = Some(((x, y), (x, y)));
                Action::Redraw
            }
            Hit::Nothing => Action::Nothing,
        }
    }

    pub fn motion(&mut self, x: f32, y: f32) -> Action {
        if let Some((start, _)) = self.band {
            self.band = Some((start, (x, y)));
            let (bx, by, bw, bh) = band_rect(start, (x, y));
            let inside: Vec<usize> = (0..self.items.len())
                .filter(|&i| {
                    let (ix, iy, iw, ih) = self.item_rect(i);
                    ix < bx + bw && bx < ix + iw && iy < by + bh && by < iy + ih
                })
                .collect();
            self.set_selection(inside);
            return Action::Redraw;
        }
        if let Some(press) = &mut self.press {
            if !press.dragging {
                let (dx, dy) = (x - press.at.0, y - press.at.1);
                if dx * dx + dy * dy < DRAG_START * DRAG_START {
                    return Action::Nothing;
                }
                press.dragging = true;
                self.last_click = None;
                return Action::DragStart;
            }
            return Action::DragMove { x, y };
        }
        let hit = match self.hit(x, y) {
            // Подсвечиваются только кнопки и места.
            h @ (Hit::Back | Hit::Forward | Hit::Up | Hit::Place(_)) => h,
            _ => Hit::Nothing,
        };
        redraw_if(std::mem::replace(&mut self.hover, hit) != hit)
    }

    pub fn release(&mut self, x: f32, y: f32) -> Action {
        if self.band.take().is_some() {
            return Action::Redraw;
        }
        let Some(press) = self.press.take() else {
            return Action::Nothing;
        };
        if press.dragging {
            return Action::Drop { x, y };
        }
        if press.collapse {
            self.select(Some(press.index));
            return Action::Redraw;
        }
        Action::Nothing
    }

    /// Значок, который тащат (ведущий), если тащат.
    pub fn dragged(&self) -> Option<usize> {
        self.press.as_ref().filter(|p| p.dragging).map(|p| p.index)
    }

    /// Где схватили значок: точка нажатия относительно угла его клетки.
    pub fn grab(&self) -> Option<(f32, f32)> {
        let p = self.press.as_ref()?;
        let (cx, cy, _, _) = self.cell(p.index);
        Some((p.at.0 - cx, p.at.1 - cy))
    }

    pub fn leave(&mut self) -> Action {
        redraw_if(std::mem::replace(&mut self.hover, Hit::Nothing) != Hit::Nothing)
    }

    // ---- Клавиши ----

    pub fn key(&mut self, k: KeyPress) -> Action {
        if k.ctrl {
            let Key::Char(c) = k.key else {
                return Action::Nothing;
            };
            return match latin(c) {
                'a' => {
                    self.select_all();
                    Action::Redraw
                }
                'c' if !self.selected.is_empty() => Action::Copy(self.selected_paths()),
                'x' if !self.selected.is_empty() => Action::Cut(self.selected_paths()),
                'v' => Action::Paste,
                'n' if k.shift => Action::NewFolder,
                'n' => Action::NewWindow,
                'w' => Action::Close,
                _ => Action::Nothing,
            };
        }
        if k.alt {
            return match k.key {
                Key::Left => self.go_back(),
                Key::Right => self.go_forward(),
                Key::Up => self.go_up(),
                Key::Down => self.open_selected(),
                _ => Action::Nothing,
            };
        }
        let current = self.selected.last().copied();
        let cols = self.columns();
        let n = self.items.len();
        let moved = |to: Option<usize>| to.filter(|&t| t < n);
        let target = match k.key {
            Key::Left => moved(current.map_or(Some(0), |c| c.checked_sub(1))),
            Key::Right => moved(current.map_or(Some(0), |c| Some(c + 1))),
            Key::Up => moved(current.map_or(Some(0), |c| c.checked_sub(cols))),
            Key::Down => moved(current.map_or(Some(0), |c| Some(c + cols))),
            Key::Home => moved(Some(0)),
            Key::End => n.checked_sub(1),
            _ => None,
        };
        if let Some(t) = target {
            self.select(Some(t));
            self.ensure_visible(t);
            return Action::Redraw;
        }
        let (_, _, _, page) = self.content();
        match k.key {
            Key::PageUp => redraw_if(self.scroll_by(-page)),
            Key::PageDown => redraw_if(self.scroll_by(page)),
            Key::Return => self.open_selected(),
            Key::BackSpace => self.go_back(),
            Key::F2 => match self.selected[..] {
                [i] => Action::Rename(self.items[i].path.clone()),
                _ => Action::Nothing,
            },
            Key::Delete if !self.selected.is_empty() => Action::Trash(self.selected_paths()),
            Key::Escape => {
                self.select(None);
                Action::Redraw
            }
            // Буквы — к файлу с таким началом имени, набор подряд продолжает
            // слово (как в Finder).
            Key::Char(c) if !k.logo && !c.is_control() => {
                let again = self.typed_at.elapsed() > TYPE_AGAIN;
                if again {
                    self.typed.clear();
                }
                self.typed.push(c);
                self.typed_at = Instant::now();
                let start = match (again, current) {
                    (true, Some(c)) => c + 1,
                    (false, Some(c)) => c,
                    (_, None) => 0,
                };
                let prefix = self.typed.to_lowercase();
                let found = (0..n)
                    .map(|k| (start + k) % n)
                    .find(|&i| self.items[i].name.to_lowercase().starts_with(&prefix));
                match found {
                    Some(i) => {
                        self.select(Some(i));
                        self.ensure_visible(i);
                        Action::Redraw
                    }
                    None => Action::Nothing,
                }
            }
            _ => Action::Nothing,
        }
    }

    // ---- Рисование ----

    /// Строка состояния: сколько всего и сколько выделено.
    fn status(&self) -> String {
        let n = self.items.len();
        match self.selected.len() {
            0 => format!("{n} {}", objects(n)),
            k => format!("Выбрано: {k} из {n}"),
        }
    }

    /// Окно целиком.
    pub fn render(&self, fonts: &mut Fonts, icons: &mut Icons) -> Canvas {
        let (w, h) = (self.size.0.max(1.0) as u32, self.size.1.max(1.0) as u32);
        let mut pm = Canvas::new(w, h).expect("размер не нулевой");
        let (wf, hf) = self.size;
        fill(&mut pm, (0.0, 0.0, wf, hf), BG);
        self.draw_items(&mut pm, fonts, icons);
        if let Some((a, b)) = self.band {
            let (x, y, bw, bh) = band_rect(a, b);
            fill(&mut pm, (x, y, bw, bh), BAND_FILL);
            stroke(&mut pm, (x, y, bw, bh), BAND_EDGE);
        }
        self.draw_scrollbar(&mut pm);
        self.draw_sidebar(&mut pm, fonts, icons);
        self.draw_toolbar(&mut pm, fonts, icons);
        // Строка состояния.
        let (cx, _, cw, _) = self.content();
        fill(&mut pm, (cx, hf - STATUS_H, cw, STATUS_H), TOOLBAR_BG);
        fill(&mut pm, (cx, hf - STATUS_H, cw, 1.0), LINE);
        let text = self.status();
        let tw = fonts.measure(&text, &STATUS);
        fonts.draw(
            &mut pm,
            &text,
            &STATUS,
            (cx + (cw - tw) / 2.0).round(),
            hf - STATUS_H + (STATUS_H - STATUS.line_height()) / 2.0,
            None,
        );
        pm
    }

    fn draw_items(&self, pm: &mut Canvas, fonts: &mut Fonts, icons: &mut Icons) {
        let (cx, cy, cw, ch) = self.content();
        if self.loaded && self.items.is_empty() {
            let text = "Папка пуста";
            let tw = fonts.measure(text, &EMPTY);
            fonts.draw(
                pm,
                text,
                &EMPTY,
                (cx + (cw - tw) / 2.0).round(),
                cy + ch / 2.0 - 10.0,
                None,
            );
            return;
        }
        // Только видимые строки: в большой папке — тысячи файлов.
        let cols = self.columns();
        let first_row = ((self.scroll - PAD) / CELL_H).floor().max(0.0) as usize;
        let rows = (ch / CELL_H).ceil() as usize + 2;
        let from = first_row * cols;
        let to = ((first_row + rows) * cols).min(self.items.len());
        for i in from..to {
            self.draw_item(pm, fonts, icons, i, (0.0, 0.0));
        }
    }

    /// Значок `i` на холсте, левый верхний угол которого — в точке `origin`
    /// окна (для картинки перетаскивания — угол клетки).
    pub fn draw_item(
        &self,
        pm: &mut Canvas,
        fonts: &mut Fonts,
        icons: &mut Icons,
        i: usize,
        origin: (f32, f32),
    ) {
        let Some(item) = self.items.get(i) else {
            return;
        };
        let (x, y, cw, _) = self.cell(i);
        let (x, y) = (x - origin.0, y - origin.1);
        let selected = self.selected.contains(&i) || self.drop_target == Some(i);
        let ix = x + (cw - ICON as f32) / 2.0;
        if selected {
            fill_rounded(
                pm,
                (
                    ix - 5.0,
                    y + ICON_TOP - 5.0,
                    ICON as f32 + 10.0,
                    ICON as f32 + 10.0,
                ),
                6.0,
                ICON_SELECTED,
            );
        }
        if let Some(img) = item
            .icons
            .iter()
            .find_map(|name| icons.get(name, ICON).cloned())
        {
            draw_pixmap(pm, &img, ix, y + ICON_TOP, ICON as f32);
        }
        if self.renaming.as_ref() == Some(&item.path) {
            return;
        }
        let width = cw - 8.0;
        let lh = LABEL.line_height();
        for (n, line) in label_lines(fonts, &item.name, &LABEL, width)
            .iter()
            .enumerate()
        {
            let tw = fonts.measure(line, &LABEL).min(width);
            let tx = (x + (cw - tw) / 2.0).round();
            let ty = y + LABEL_TOP + n as f32 * lh;
            if selected {
                fill_rounded(pm, (tx - 4.0, ty, tw + 8.0, lh), 4.0, LABEL_SELECTED);
            }
            fonts.draw(pm, line, &LABEL, tx, ty, Some(width));
        }
    }

    /// Картинка значка, который тащат (полупрозрачная, как в Finder).
    pub fn drag_image(&self, fonts: &mut Fonts, icons: &mut Icons) -> Option<Canvas> {
        let i = self.dragged()?;
        let (cx, cy, cw, ch) = self.cell(i);
        let mut pm = Canvas::new(cw as u32, ch as u32)?;
        self.draw_item(&mut pm, fonts, icons, i, (cx, cy));
        for px in pm.pixmap_mut().data_mut() {
            *px = (u16::from(*px) * 3 / 4) as u8;
        }
        Some(pm)
    }

    fn draw_scrollbar(&self, pm: &mut Canvas) {
        let max = self.max_scroll();
        if max <= 0.0 {
            return;
        }
        let (cx, cy, cw, ch) = self.content();
        let total = ch + max;
        let bar_h = (ch * ch / total).max(24.0);
        let y = cy + 2.0 + (ch - 4.0 - bar_h) * (self.scroll / max);
        fill_rounded(pm, (cx + cw - 8.0, y, 5.0, bar_h), 2.5, SCROLLBAR);
    }

    fn draw_sidebar(&self, pm: &mut Canvas, fonts: &mut Fonts, icons: &mut Icons) {
        let (_, hf) = self.size;
        fill(pm, (0.0, TOOLBAR_H, SIDEBAR_W, hf - TOOLBAR_H), SIDEBAR_BG);
        fill(pm, (SIDEBAR_W - 1.0, TOOLBAR_H, 1.0, hf - TOOLBAR_H), LINE);
        fonts.draw(pm, "Места", &HEADING, 18.0, TOOLBAR_H + 14.0, None);
        for (n, place) in self.places.iter().enumerate() {
            let (x, y, w, h) = self.place_rect(n);
            if self.dir == place.path {
                fill_rounded(pm, (x, y + 1.0, w, h - 2.0), 6.0, CURRENT);
            } else if self.hover == Hit::Place(n) {
                fill_rounded(pm, (x, y + 1.0, w, h - 2.0), 6.0, HOVER);
            }
            if let Some(img) = icons.get(place.icon, SIDE_ICON) {
                let img = tint(img, SIDE_ICON_COLOR);
                let s = SIDE_ICON as f32;
                draw_pixmap(pm, &img, x + 10.0, y + (h - s) / 2.0, s);
            }
            fonts.draw(
                pm,
                &place.name,
                &SIDE,
                x + 34.0,
                y + (h - SIDE.line_height()) / 2.0,
                Some(w - 40.0),
            );
        }
    }

    fn draw_toolbar(&self, pm: &mut Canvas, fonts: &mut Fonts, icons: &mut Icons) {
        let (wf, _) = self.size;
        fill(pm, (0.0, 0.0, wf, TOOLBAR_H), TOOLBAR_BG);
        fill(pm, (0.0, TOOLBAR_H - 1.0, wf, 1.0), LINE);
        let buttons = [
            (Hit::Back, "go-previous-symbolic", !self.back.is_empty()),
            (Hit::Forward, "go-next-symbolic", !self.forward.is_empty()),
            (Hit::Up, "go-up-symbolic", self.dir.parent().is_some()),
        ];
        for (n, (hit, icon, enabled)) in buttons.into_iter().enumerate() {
            let (x, y, w, h) = Self::button_rect(n);
            if enabled && self.hover == hit {
                fill_rounded(pm, (x, y, w, h), 6.0, HOVER);
            }
            if let Some(img) = icons.get(icon, SIDE_ICON) {
                let img = tint(img, if enabled { BUTTON_COLOR } else { BUTTON_OFF });
                let s = SIDE_ICON as f32;
                draw_pixmap(pm, &img, x + (w - s) / 2.0, y + (h - s) / 2.0, s);
            }
        }
        let (bx, _, bw, _) = Self::button_rect(2);
        let tx = bx + bw + 16.0;
        fonts.draw(
            pm,
            &self.title(),
            &TITLE,
            tx,
            (TOOLBAR_H - TITLE.line_height()) / 2.0,
            Some(wf - tx - 16.0),
        );
    }
}

/// Загрузить значки окна заранее (места, кнопки, частые типы файлов).
pub fn preload(icons: &mut Icons, places: &[Place]) {
    for place in places {
        icons.get(place.icon, SIDE_ICON);
    }
    for name in ["go-previous-symbolic", "go-next-symbolic", "go-up-symbolic"] {
        icons.get(name, SIDE_ICON);
    }
    for name in [
        "folder",
        "text-x-generic",
        "image-x-generic",
        "image-png",
        "image-jpeg",
        "application-pdf",
        "application-x-executable",
        "audio-x-generic",
        "video-x-generic",
        "package-x-generic",
        "unknown",
    ] {
        icons.get(name, ICON);
    }
}

fn redraw_if(changed: bool) -> Action {
    if changed {
        Action::Redraw
    } else {
        Action::Nothing
    }
}

/// «объект», «объекта», «объектов» — по числу.
fn objects(n: usize) -> &'static str {
    match (n % 10, n % 100) {
        (1, r) if r != 11 => "объект",
        (2..=4, r) if !(12..=14).contains(&r) => "объекта",
        _ => "объектов",
    }
}

/// Прямоугольник рамки между точкой начала и указателем.
fn band_rect(a: (f32, f32), b: (f32, f32)) -> Rect {
    (
        a.0.min(b.0),
        a.1.min(b.1),
        (a.0 - b.0).abs(),
        (a.1 - b.1).abs(),
    )
}

fn fill(pm: &mut Canvas, (x, y, w, h): Rect, color: [u8; 4]) {
    if let Some(r) = tiny_skia::Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &paint(color), Transform::identity(), None);
    }
}

fn fill_rounded(pm: &mut Canvas, (x, y, w, h): Rect, r: f32, color: [u8; 4]) {
    if let Some(path) = rounded(x, y, w, h, r) {
        pm.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn stroke(pm: &mut Canvas, (x, y, w, h): Rect, color: [u8; 4]) {
    let Some(r) =
        tiny_skia::Rect::from_xywh(x + 0.5, y + 0.5, (w - 1.0).max(0.5), (h - 1.0).max(0.5))
    else {
        return;
    };
    let path = tiny_skia::PathBuilder::from_rect(r);
    let s = tiny_skia::Stroke {
        width: 1.0,
        ..tiny_skia::Stroke::default()
    };
    pm.stroke_path(&path, &paint(color), &s, Transform::identity(), None);
}

/// Картинку (в пикселях экрана) — квадратом `size` точек в точке `x`, `y`.
fn draw_pixmap(pm: &mut Canvas, img: &Pixmap, x: f32, y: f32, size: f32) {
    let k = size / img.width().max(1) as f32;
    let t = Transform::from_scale(k, k).post_translate(x, y);
    let quality = PixmapPaint {
        quality: FilterQuality::Bicubic,
        ..PixmapPaint::default()
    };
    pm.draw_pixmap(0, 0, img.as_ref(), &quality, t, None);
}

/// Одноцветный значок — в нужный цвет (форма — по прозрачности).
fn tint(img: &Pixmap, [r, g, b, a]: [u8; 4]) -> Pixmap {
    let mut out = img.clone();
    for px in out.pixels_mut() {
        let alpha = (u16::from(px.alpha()) * u16::from(a) / 255) as u8;
        let mul = |c: u8| (u16::from(c) * u16::from(alpha) / 255) as u8;
        if let Some(c) = tiny_skia::PremultipliedColorU8::from_rgba(mul(r), mul(g), mul(b), alpha) {
            *px = c;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, kind: Kind) -> Item {
        Item {
            path: PathBuf::from(format!("/дом/{name}")),
            name: name.into(),
            kind,
            icons: vec![],
        }
    }

    fn view(n: usize) -> FolderView {
        let mut v = FolderView::new(PathBuf::from("/дом"), vec![], (900, 560));
        let mut items = vec![item("Папка", Kind::Folder)];
        items.extend((1..n).map(|k| item(&format!("файл{k}.txt"), Kind::File)));
        v.set_items(items);
        v
    }

    /// Середина картинки значка `i`.
    fn center(v: &FolderView, i: usize) -> (f32, f32) {
        let (x, y, w, _) = v.cell(i);
        (x + w / 2.0, y + ICON_TOP + 20.0)
    }

    #[test]
    fn grid_rows_left_to_right_and_hit() {
        let v = view(20);
        let cols = v.columns();
        assert!(cols >= 5, "в окне 900 точек — не меньше 5 столбцов: {cols}");
        let (x0, y0, _, _) = v.cell(0);
        let (x1, y1, _, _) = v.cell(1);
        assert!(x1 > x0 && y1 == y0, "второй — правее в той же строке");
        assert!(v.cell(cols).1 > y0, "следующая строка ниже");
        let (cx, cy) = center(&v, 3);
        assert_eq!(v.hit(cx, cy), Hit::Item(3));
        assert_eq!(v.hit(x0 + 2.0, y0 + 2.0), Hit::Empty, "угол клетки — пусто");
        assert_eq!(v.hit(20.0, 20.0), Hit::Back);
        assert_eq!(objects(1), "объект");
        assert_eq!(objects(3), "объекта");
        assert_eq!(objects(11), "объектов");
        assert_eq!(objects(22), "объекта");
    }

    #[test]
    fn click_select_double_open_and_back() {
        let mut v = view(5);
        let (x, y) = center(&v, 2);
        assert_eq!(v.press(x, y, 1, false, false), Action::Redraw);
        assert_eq!(v.release(x, y), Action::Nothing);
        assert_eq!(v.selected, [2]);
        // Второй щелчок вскоре — двойной: открыть программой.
        match v.press(x, y, 1, false, false) {
            Action::Open(items) => assert_eq!(items[0].name, "файл2.txt"),
            other => panic!("{other:?}"),
        }
        // Двойной щелчок по папке — перейти в неё; «назад» — обратно, и
        // папка, из которой вышли, выделена.
        let (x, y) = center(&v, 0);
        v.press(x, y, 1, false, false);
        assert_eq!(v.press(x, y, 1, false, false), Action::Load);
        assert_eq!(v.dir(), Path::new("/дом/Папка"));
        assert!(v.items().is_empty());
        let k = KeyPress {
            alt: true,
            ..KeyPress::plain(Key::Left)
        };
        assert_eq!(v.key(k), Action::Load);
        assert_eq!(v.dir(), Path::new("/дом"));
        v.set_items(view(5).items);
        assert_eq!(v.selected, [0], "выделена папка, из которой вышли");
        assert_eq!(v.key(KeyPress::plain(Key::Right)), Action::Redraw);
        assert_eq!(v.selected, [1]);
    }

    #[test]
    fn ctrl_shift_band_and_drag() {
        let mut v = view(8);
        let (x1, y1) = center(&v, 1);
        let (x3, y3) = center(&v, 3);
        v.press(x1, y1, 1, false, false);
        v.release(x1, y1);
        v.press(x3, y3, 1, false, true);
        assert_eq!(v.selected, [1, 2, 3], "Shift — от первого до этого");
        v.press(x1, y1, 1, true, false);
        assert_eq!(v.selected, [2, 3], "Ctrl — убрать");
        // Рамка с пустого места.
        let (cx, cy, _, _) = v.cell(0);
        v.press(cx + 1.0, cy + 1.0, 1, false, false);
        assert!(v.selected.is_empty());
        let (x2, y2) = center(&v, 2);
        assert_eq!(v.motion(x2, y2), Action::Redraw);
        assert_eq!(v.selected, [0, 1, 2]);
        assert_eq!(v.release(x2, y2), Action::Redraw);
        // Перетаскивание: сдвиг больше порога — начать.
        v.press(x2, y2, 1, false, false);
        assert_eq!(v.motion(x2 + 2.0, y2), Action::Nothing);
        assert_eq!(v.motion(x2 + 30.0, y2), Action::DragStart);
        assert_eq!(v.dragged(), Some(2));
        assert_eq!(
            v.motion(x2 + 40.0, y2),
            Action::DragMove {
                x: x2 + 40.0,
                y: y2
            }
        );
        let (fx, fy) = center(&v, 0);
        assert_eq!(v.folder_at(fx, fy, false), Some(0));
        assert_eq!(
            v.folder_at(fx, fy, true),
            None,
            "папку саму в себя не кладём"
        );
        assert!(matches!(v.release(x2 + 40.0, y2), Action::Drop { .. }));
    }

    #[test]
    fn scroll_keys_and_prefix() {
        let mut v = view(200);
        assert!(v.max_scroll() > 0.0);
        assert_eq!(v.press(400.0, 200.0, 5, false, false), Action::Redraw);
        assert_eq!(v.scroll, SCROLL_STEP);
        assert_eq!(v.key(KeyPress::plain(Key::End)), Action::Redraw);
        assert_eq!(v.scroll, v.max_scroll(), "к последнему — прокрутка вниз");
        v.key(KeyPress::plain(Key::Home));
        assert_eq!(v.scroll, 0.0);
        v.key(KeyPress::plain(Key::Char('ф')));
        assert_eq!(v.selected, [1], "к первому на «ф»");
        let ctrl_c = KeyPress {
            ctrl: true,
            ..KeyPress::plain(Key::Char('с'))
        };
        assert!(matches!(v.key(ctrl_c), Action::Copy(p) if p.len() == 1));
        assert!(matches!(v.key(KeyPress::plain(Key::F2)), Action::Rename(_)));
    }

    #[test]
    fn renders() {
        let mut fonts = Fonts::new("Inter");
        let mut icons = Icons::with_dirs("none", vec![], vec![]);
        let mut v = view(30);
        v.selected = vec![1];
        let pm = v.render(&mut fonts, &mut icons);
        assert_eq!((pm.width(), pm.height()), (900, 560));
        // Под выделенной подписью — синяя подложка.
        let (lx, ly, lw, _) = v.label_rect(1);
        let p = pm
            .pixel((lx + lw / 2.0) as u32 - 10, ly as u32 + 3)
            .expect("внутри");
        assert!(p.blue() > p.red() + 60, "{p:?}");
    }
}
