//! Рабочий стол (вместо xfdesktop): окно под всеми окнами с обоями на
//! каждом мониторе и значками файлов из папки «Рабочий стол», меню по правому
//! щелчку и окно «Обои».
//!
//! - значки: щелчок выделяет, двойной открывает, перетаскивание — на другое
//!   место (запоминается) или в папку; меню на значке — «Открыть»,
//!   «Переименовать», «В корзину»; на пустом месте — «Новая папка» (там, где
//!   щёлкнули, и сразу ввод имени), «Вставить», «Упорядочить», «Открыть
//!   терминал здесь», «Изменить обои…»; переименование — полем прямо на столе;
//! - обои — в `desktop_wallpaper`, действия с файлами — в `desktop_files`.
//!
//! Картинка стола — обои со значками — лежит на X-сервере фоном окна. Новые
//! обои или другой список файлов — стол отправляется целиком; выделение —
//! только клетки затронутых значков.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use shell_core::config::Config;
use shell_desktop::files::{Item, Kind};
use shell_desktop::icon_grid::{self, Direction, IconGrid};
use shell_desktop::rename_box::{RenameBox, RenameResult};
use shell_desktop::{PickerResult, WallpaperPicker, wallpaper_picker};
use shell_icons::Icons;
use shell_render::Canvas;
use shell_text::Fonts;
use shell_widgets::shape::paint;
use shell_widgets::{Menu, MenuEntry, MenuResult};
use shell_x11::{
    DesktopWindow, Display, DragTarget, Event, Key, KeyPress, Popup, SelectionRenderer,
};
use tiny_skia::{IntRect, Pixmap};

use crate::desktop_files;
use crate::desktop_wallpaper::{WALLPAPERS, painter, pick_file, save_wallpaper, thumb};
use crate::ui::{Msg, Result};

/// Пункты меню стола и значков.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DesktopCommand {
    // На пустом месте.
    NewFolder,
    NewTextFile,
    Paste,
    Arrange,
    Terminal,
    ChangeWallpaper,
    // На значке.
    Open,
    Rename,
    Copy,
    Cut,
    Trash,
}

/// Что приходит столу из его потоков.
pub enum DesktopMsg {
    /// Обои во весь экран (в пикселях) для этого значения настройки и
    /// сколько они рисовались.
    Wallpaper(String, Pixmap, Duration),
    /// Миниатюра для окна «Обои».
    Thumb(String, Pixmap),
    /// Окно выбора файла закрыто: путь к картинке или `None` (отмена).
    Picked(Option<String>),
    /// Список файлов на столе (сразу и после каждой перемены в папке).
    Files(Vec<Item>),
    /// Завершённый перенос файлов из другой программы — подтвердить XDND.
    DropFinished { ticket: u64, success: bool },
}

/// Нажали на значок: где, и тащат ли его уже (сдвинули дальше [`DRAG_START`]).
struct Press {
    index: usize,
    /// Щёлкнули по одному из нескольких выделенных: если не потащат, при
    /// отпускании выделенным останется только он (как на Mac).
    collapse: bool,
    at: (f32, f32),
    /// Точка нажатия относительно угла клетки — значок едет за мышью так же.
    grab: (f32, f32),
    dragging: bool,
}

/// Сколько точек сдвинуть мышь с нажатой кнопкой, чтобы начать тащить.
const DRAG_START: f32 = 5.0;
/// Пауза в наборе букв дольше этой — следующая буква начинает слово заново.
const TYPE_AGAIN: Duration = Duration::from_millis(1000);
/// Два щелчка по одному значку быстрее этого — «открыть».
const DOUBLE_CLICK: Duration = Duration::from_millis(450);
/// Отступы области значков: справа, под строкой меню и над доком.
const MARGIN: f32 = 12.0;
const DOCK_ROOM: f32 = 96.0;

pub struct DesktopUi {
    window: DesktopWindow,
    menu_popup: Popup,
    menu: Option<Menu<DesktopCommand>>,
    menu_dirty: bool,
    picker_popup: Popup,
    picker: Option<WallpaperPicker>,
    picker_dirty: bool,
    /// Нынешние обои (значение `look.wallpaper`) и их картинка без значков
    /// (в пикселях экрана) — из неё перерисовываются клетки значков.
    wallpaper: String,
    wall: Option<Pixmap>,
    /// Значки и папка «Рабочий стол».
    grid: IconGrid,
    dir: PathBuf,
    /// Выделить, когда появится в списке (только что создан или переименован).
    pending_select: Option<PathBuf>,
    /// Последний щелчок по значку — для двойного щелчка.
    last_click: Option<(usize, Instant)>,
    /// Нажатие на значок и перетаскивание: окно с картинкой значка, где
    /// сейчас указатель, перерисовать ли картинку.
    press: Option<Press>,
    drag_popup: Popup,
    drag_at: (f32, f32),
    drag_dirty: bool,
    /// Переименование полем на столе; `rename_start` — начать, когда значок
    /// с этим путём появится (новая папка).
    rename_popup: Popup,
    rename: Option<RenameBox>,
    rename_dirty: bool,
    rename_start: Option<PathBuf>,
    /// Рамка выделения: где начали и где указатель; окно рамки.
    band: Option<((f32, f32), (f32, f32))>,
    band_popup: Popup,
    selection_renderer: Option<SelectionRenderer>,
    band_dirty: bool,
    /// Окно рамки (во весь экран, прозрачное) показано; где рамка нарисована
    /// в нём сейчас — этот участок и стираем при следующем движении.
    band_shown: bool,
    band_last: Option<(f32, f32, f32, f32)>,
    /// Где щёлкнули правой кнопкой — там и «Новая папка».
    menu_at: (f32, f32),
    /// Набранные на столе буквы (прыжок к значку) и когда набрали последнюю.
    typed: String,
    typed_at: Instant,
    /// Перерисовать: весь стол или клетки этих значков.
    full_dirty: bool,
    cells_dirty: Vec<usize>,
    terminal: Vec<String>,
    menubar_height: u32,
    /// Когда выбрали нынешние обои (для замера в журнале) и сколько прежних
    /// выборов не дождались своих обоев (выбрали другие раньше).
    chosen_at: Option<Instant>,
    skipped: u32,
    /// Рисовальщик обоев (один поток): (значение, размер экрана в пикселях).
    painter: Sender<(String, (u32, u32))>,
    tx: Sender<Msg>,
}

impl DesktopUi {
    pub fn new(d: &Display, config: &Config, tx: Sender<Msg>) -> Result<DesktopUi> {
        let window = d.create_desktop()?;
        // Xfwm4 обновляет _NET_WORKAREA при изменении резервов Dock/панелей.
        d.watch_root()?;
        window.show(d)?;
        let dir = desktop_files::desktop_dir();
        let band_popup = d.create_popup("desktop-band")?;
        let selection_renderer = SelectionRenderer::new(d, band_popup.id());
        let mut ui = DesktopUi {
            window,
            menu_popup: d.create_popup("desktop-menu")?,
            menu: None,
            menu_dirty: false,
            picker_popup: d.create_popup("wallpaper-picker")?,
            picker: None,
            picker_dirty: false,
            wallpaper: config.wallpaper.clone(),
            wall: None,
            grid: IconGrid::new((0.0, 0.0, 1.0, 1.0)),
            dir: dir.clone(),
            pending_select: None,
            last_click: None,
            press: None,
            drag_popup: d.create_popup("desktop-drag")?,
            drag_at: (0.0, 0.0),
            drag_dirty: false,
            rename_popup: d.create_popup("desktop-rename")?,
            rename: None,
            rename_dirty: false,
            rename_start: None,
            band: None,
            band_popup,
            selection_renderer,
            band_dirty: false,
            band_shown: false,
            band_last: None,
            menu_at: (0.0, 0.0),
            typed: String::new(),
            typed_at: Instant::now(),
            full_dirty: false,
            cells_dirty: Vec::new(),
            terminal: config.terminal.clone(),
            menubar_height: config.menubar_height,
            chosen_at: None,
            skipped: 0,
            painter: painter(tx.clone()),
            tx: tx.clone(),
        };
        // Картинка, которую тащат, не должна заслонять окно под указателем.
        ui.drag_popup.pass_through(d)?;
        ui.grid.set_saved(desktop_files::load_places());
        ui.place_icons(d);
        ui.load_wallpaper(d);
        desktop_files::watch(dir, tx);
        Ok(ui)
    }

    /// Где стоят значки: на первом мониторе, под строкой меню, над доком.
    fn place_icons(&mut self, d: &Display) {
        let k = d.scale();
        let (sw, sh) = d.screen_size();
        let (mx, my, mw, mh) = shell_setup::parts::monitor_rects()
            .first()
            .map(|&(x, y, w, h)| (x as f32 / k, y as f32 / k, w as f32 / k, h as f32 / k))
            .unwrap_or((0.0, 0.0, f32::from(sw), f32::from(sh)));
        // Приоритет — EWMH _NET_WORKAREA от оконного менеджера: фактические
        // резервы панелей, Plank и нашего Dock уже вычислены на стороне WM.
        // Пересекаем глобальную область с выбранным монитором, не смешивая
        // пиксели и точки на HiDPI.
        let bounds = d.workarea().and_then(|(x, y, w, h)| {
            let (left, top) = ((x as f32 / k).max(mx), (y as f32 / k).max(my));
            let (right, bottom) = (
                ((i64::from(x) + i64::from(w)) as f32 / k).min(mx + mw),
                ((i64::from(y) + i64::from(h)) as f32 / k).min(my + mh),
            );
            (right > left && bottom > top).then_some((left, top, right, bottom))
        });
        // Не выше строки меню и не ниже дока: док при «умном» скрытии места не
        // резервирует, и рабочая область от оконного менеджера его не учитывает —
        // нижний ряд значков ушёл бы под него.
        let (min_top, max_bottom) = (my + self.menubar_height as f32, my + mh - DOCK_ROOM);
        let (left, top, right, bottom) = bounds
            .map(|(l, t, r, b)| (l, t.max(min_top), r, b.min(max_bottom)))
            .unwrap_or((mx, min_top, mx + mw, max_bottom));
        let top = top + MARGIN;
        self.grid.set_area((
            left,
            top,
            (right - left - MARGIN).max(icon_grid::CELL_W),
            (bottom - top - MARGIN).max(icon_grid::CELL_H),
        ));
        self.full_dirty = true;
    }

    /// Нарисовать обои под нынешние мониторы (в потоке рисовальщика).
    fn load_wallpaper(&self, d: &Display) {
        let (sw, sh) = d.screen_pixels();
        let _ = self
            .painter
            .send((self.wallpaper.clone(), (u32::from(sw), u32::from(sh))));
    }

    /// Пришло из потока.
    pub fn message(&mut self, d: &Display, msg: DesktopMsg) -> Result<()> {
        match msg {
            // Пока рисовались одни обои, могли выбрать другие — старые не ставим.
            DesktopMsg::Wallpaper(value, image, drawn) if value == self.wallpaper => {
                // Сколько от выбора в окне «Обои» до обоев на столе.
                if let Some(chosen) = self.chosen_at.take() {
                    eprintln!(
                        "Обои «{value}»: готовы через {:?} после выбора (рисование {drawn:?}; \
                         пропущено прежних выборов: {})",
                        chosen.elapsed(),
                        std::mem::take(&mut self.skipped)
                    );
                }
                self.wall = Some(image);
                self.full_dirty = true;
            }
            DesktopMsg::Wallpaper(..) => {}
            DesktopMsg::Thumb(value, image) => {
                if let Some(picker) = &mut self.picker {
                    picker.set_thumb(value, image);
                    self.picker_dirty = true;
                }
            }
            DesktopMsg::Picked(path) => {
                if let Some(path) = path {
                    self.apply(d, &path);
                }
                self.open_picker(d)?;
            }
            DesktopMsg::DropFinished { ticket, success } => {
                d.finish_drop(ticket, success);
            }
            DesktopMsg::Files(items) => {
                self.grid.set_items(items);
                if let Some(path) = self.pending_select.take() {
                    self.grid.select_path(&path);
                    if self.grid.selected().is_none_or(|i| i.path != path) {
                        // Ещё не появился — подождём следующего списка.
                        self.pending_select = Some(path);
                    }
                }
                self.last_click = None;
                self.full_dirty = true;
            }
        }
        Ok(())
    }

    pub fn wants(&self, d: &Display, ev: &Event) -> bool {
        let own = |w: u32| {
            w == self.window.id()
                || w == self.menu_popup.id()
                || w == self.picker_popup.id()
                || w == self.rename_popup.id()
        };
        let open = self.menu.is_some() || self.picker.is_some() || self.rename.is_some();
        match *ev {
            Event::Button { window, .. }
            | Event::Release { window, .. }
            | Event::Motion { window, .. }
            | Event::Expose { window } => own(window) || open,
            // Клавиши — стол активен (по нему щёлкнули) или открыто его окно.
            Event::Key(_) => open || d.active_window() == Some(self.window.id()),
            // Файлы из других программ (Проводник) несут или бросили на стол.
            Event::DropHover { window, .. }
            | Event::DropLeave { window }
            | Event::Dropped { window, .. } => window == self.window.id(),
            // Сменился размер экрана (корневое окно) — стол во весь экран заново.
            Event::Configure { window } => window == d.root(),
            Event::Property {
                window,
                what: shell_x11::Property::WorkArea,
            } => window == d.root(),
            _ => false,
        }
    }

    pub fn event(&mut self, d: &Display, fonts: &mut Fonts, ev: Event) -> Result<()> {
        if let Event::Configure { window } = ev
            && window == d.root()
        {
            self.window.show(d)?;
            self.place_icons(d);
            self.load_wallpaper(d);
            return Ok(());
        }
        if let Event::Property {
            window,
            what: shell_x11::Property::WorkArea,
        } = ev
            && window == d.root()
        {
            self.place_icons(d);
            return Ok(());
        }
        if self.rename.is_some() {
            return self.rename_event(d, ev);
        }
        if self.picker.is_some() {
            return self.picker_event(d, ev);
        }
        if self.menu.is_some() {
            return self.menu_event(d, ev);
        }
        match ev {
            Event::DropHover { x, y, .. } => {
                let target = self
                    .grid
                    .item_at(f32::from(x), f32::from(y))
                    .filter(|&t| self.grid.items()[t].kind == Kind::Folder);
                let changed = self.grid.set_drop_target(target);
                self.cells_dirty.extend(changed);
                Ok(())
            }
            Event::DropLeave { .. } => {
                let changed = self.grid.set_drop_target(None);
                self.cells_dirty.extend(changed);
                Ok(())
            }
            Event::Dropped {
                x,
                y,
                moving,
                ticket,
                ..
            } => {
                let changed = self.grid.set_drop_target(None);
                self.cells_dirty.extend(changed);
                // Выполняем именно подтверждённое источнику действие.
                self.dropped(
                    d.take_drop(ticket),
                    f32::from(x),
                    f32::from(y),
                    moving,
                    ticket,
                );
                Ok(())
            }
            Event::Key(k) => self.key(k),
            Event::Motion { window, x, y } if window == self.window.id() => {
                self.drag_motion(d, f32::from(x), f32::from(y));
                Ok(())
            }
            Event::Release {
                window,
                x,
                y,
                button: 1,
            } if window == self.window.id() => self.drop(d, f32::from(x), f32::from(y)),
            Event::Button {
                window,
                x,
                y,
                button,
            } if window == self.window.id() => self.button(d, fonts, x, y, button),
            _ => Ok(()),
        }
    }

    fn button(&mut self, d: &Display, fonts: &mut Fonts, x: i16, y: i16, button: u8) -> Result<()> {
        let (fx, fy) = (f32::from(x), f32::from(y));
        let hit = self.grid.item_at(fx, fy);
        // Ctrl+щелчок — добавить к выделенным или убрать, Shift+щелчок —
        // добавить (как в Finder и Проводнике Windows).
        if button == 1
            && let Some(i) = hit
        {
            let (ctrl, shift) = d.modifiers();
            if ctrl || shift {
                let mut sel = self.grid.selected_indices().to_vec();
                if ctrl && sel.contains(&i) {
                    sel.retain(|&s| s != i);
                } else {
                    sel.push(i);
                }
                let changed = self.grid.set_selection(sel);
                self.cells_dirty.extend(changed);
                self.last_click = None;
                return Ok(());
            }
        }
        // По уже выделенному из нескольких — выделение не трогаем: их можно
        // потащить все или открыть меню для всех.
        let keep =
            hit.is_some_and(|i| self.grid.is_selected(i)) && self.grid.selected_indices().len() > 1;
        if !keep {
            let changed = self.grid.select(hit);
            self.cells_dirty.extend(changed);
        }
        match button {
            1 => {
                let now = Instant::now();
                let double = hit.is_some()
                    && self
                        .last_click
                        .is_some_and(|(i, t)| Some(i) == hit && now - t < DOUBLE_CLICK);
                if double {
                    self.last_click = None;
                    self.press = None;
                    for item in self.grid.selected_items() {
                        desktop_files::open(item);
                    }
                } else {
                    self.last_click = hit.map(|i| (i, now));
                    self.press = hit.map(|index| {
                        let (cx, cy, _, _) = self.grid.cell(index);
                        Press {
                            index,
                            collapse: keep,
                            at: (fx, fy),
                            grab: (fx - cx, fy - cy),
                            dragging: false,
                        }
                    });
                    // По пустому месту — рамка выделения.
                    if hit.is_none() {
                        self.band = Some(((fx, fy), (fx, fy)));
                    }
                }
            }
            3 => {
                self.menu_at = (fx, fy);
                self.open_menu(d, fonts, x, y, hit.is_some())?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Мышь с нажатой кнопкой: сдвинули значок достаточно — тащим.
    fn drag_motion(&mut self, d: &Display, x: f32, y: f32) {
        if let Some((start, _)) = self.band {
            self.band = Some((start, (x, y)));
            let changed = self
                .grid
                .set_selection(self.grid.items_in(band_rect(start, (x, y))));
            self.cells_dirty.extend(changed);
            self.band_dirty = true;
            return;
        }
        let Some(press) = &mut self.press else {
            return;
        };
        if !press.dragging {
            let (dx, dy) = (x - press.at.0, y - press.at.1);
            if dx * dx + dy * dy < DRAG_START * DRAG_START {
                return;
            }
            press.dragging = true;
            self.last_click = None;
            // Стол — источник XDND: значки можно бросить и в окно Проводника.
            let paths: Vec<PathBuf> = self
                .grid
                .selected_items()
                .iter()
                .map(|i| i.path.clone())
                .collect();
            d.drag_begin(self.window.id(), &paths);
        }
        self.drag_at = (x, y);
        self.drag_dirty = true;
        // Стол во весь экран от угла: его точки — точки экрана.
        let over = d.drag_motion(x.round() as i16, y.round() as i16, d.modifiers().0);
        if over != DragTarget::Own {
            let changed = self.grid.set_drop_target(None);
            self.cells_dirty.extend(changed);
            return;
        }
        // Папка под указателем (не из переносимых) — подсветить: туда упадут.
        let moving = self.grid.selected_indices().to_vec();
        let target = self
            .grid
            .item_at(x, y)
            .filter(|t| !moving.contains(t))
            .filter(|&t| self.grid.items()[t].kind == Kind::Folder);
        let changed = self.grid.set_drop_target(target);
        self.cells_dirty.extend(changed);
    }

    /// Отпустили: на папке — переложить в неё, иначе — значок в клетку, куда
    /// его принесли (занята — в ближайшую свободную).
    fn drop(&mut self, d: &Display, x: f32, y: f32) -> Result<()> {
        if self.band.take().is_some() {
            if std::mem::take(&mut self.band_shown) {
                if let Some(renderer) = &mut self.selection_renderer {
                    renderer.reset(d)?;
                }
                self.band_popup.hide_passive(d)?;
            }
            self.band_last = None;
            return Ok(());
        }
        let Some(press) = self.press.take() else {
            return Ok(());
        };
        if !press.dragging {
            if press.collapse {
                let changed = self.grid.select(Some(press.index));
                self.cells_dirty.extend(changed);
            }
            return Ok(());
        }
        self.drag_popup.hide_passive(d)?;
        let changed = self.grid.set_drop_target(None);
        self.cells_dirty.extend(changed);
        // Над окном другой программы: согласна — файлы её (переместит или
        // скопирует сама, стол узнает из слежения за папкой); нет, или окно
        // файлы не принимает — значки остаются, где были (как в Finder).
        if d.drag_end() != DragTarget::Own {
            return Ok(());
        }
        let moving: Vec<usize> = self.grid.selected_indices().to_vec();
        let target = self
            .grid
            .item_at(x, y)
            .filter(|t| !moving.contains(t))
            .and_then(|t| self.grid.items().get(t))
            .filter(|t| t.kind == Kind::Folder)
            .map(|t| t.path.clone());
        if let Some(folder) = target {
            for item in self.grid.selected_items() {
                desktop_files::move_into(item.path.clone(), folder.clone());
            }
            // Убрать их со стола сразу, не дожидаясь, пока слежение за
            // папкой заметит перенос.
            let rest: Vec<Item> = self
                .grid
                .items()
                .iter()
                .enumerate()
                .filter(|(i, _)| !moving.contains(i))
                .map(|(_, it)| it.clone())
                .collect();
            self.grid.set_items(rest);
            self.full_dirty = true;
            return Ok(());
        }
        // Клетка — по середине значка, который несли.
        let cx = x - press.grab.0 + icon_grid::CELL_W / 2.0;
        let cy = y - press.grab.1 + icon_grid::CELL_H / 2.0;
        let slot = self.grid.slot_at(cx, cy);
        if std::env::var_os("SHELL_DEBUG_DESKTOP").is_some() {
            eprintln!(
                "Стол: отпустили в ({x}, {y}), захват {:?}, клетка {slot:?}",
                press.grab
            );
        }
        self.grid.move_items(press.index, slot, &moving);
        desktop_files::save_places(self.grid.saved());
        self.full_dirty = true;
        Ok(())
    }

    /// Бросили файлы из другой программы: на папку — в неё, иначе — на стол,
    /// значками там, где отпустили (несколько — рядом друг с другом).
    fn dropped(&mut self, paths: Vec<PathBuf>, x: f32, y: f32, moving: bool, ticket: u64) {
        if paths.is_empty() {
            let _ = self
                .tx
                .send(Msg::Desktop(Box::new(DesktopMsg::DropFinished {
                    ticket,
                    success: false,
                })));
            return;
        }
        let folder = self
            .grid
            .item_at(x, y)
            .map(|t| &self.grid.items()[t])
            .filter(|t| t.kind == Kind::Folder)
            .map(|t| t.path.clone());
        let into = folder.clone().unwrap_or_else(|| self.dir.clone());
        let mut pairs = Vec::new();
        // Имена всех файлов должны быть разными ещё до начала копирования.
        let mut reserved = HashSet::new();
        let mut slot = self.grid.slot_at(x, y);
        for src in paths {
            let Some(name) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            // Папку саму в себя не кладём.
            if src == into {
                continue;
            }
            let dest = if moving && src.parent() == Some(into.as_path()) {
                // Перемещение на свой стол меняет только положение значка.
                src.clone()
            } else {
                shell_desktop::files::free_name_reserved(&into, &name, &reserved)
            };
            reserved.insert(dest.clone());
            if folder.is_none() {
                slot = self.grid.free_slot_near(slot, None);
                if let Some(n) = dest.file_name() {
                    self.grid.place_name(&n.to_string_lossy(), slot);
                }
                // Следующий — в клетку ниже.
                slot.1 += 1;
            }
            if dest != src {
                pairs.push((src, dest));
            }
        }
        if folder.is_none() {
            desktop_files::save_places(self.grid.saved());
            self.full_dirty = true;
        }
        // В одном рабочем потоке: один результат на весь пакет, затем XDND ack.
        desktop_files::bring(pairs, moving, ticket, self.tx.clone());
    }

    /// Клавиши, когда стол активен. Сочетания с Ctrl — по месту клавиши,
    /// в русской раскладке тоже (Ctrl+С = Ctrl+C).
    fn key(&mut self, k: KeyPress) -> Result<()> {
        let selected: Vec<PathBuf> = self
            .grid
            .selected_items()
            .iter()
            .map(|i| i.path.clone())
            .collect();
        if k.ctrl {
            if let Key::Char(c) = k.key {
                match latin(c) {
                    'a' => {
                        let changed = self.grid.select_all();
                        self.cells_dirty.extend(changed);
                    }
                    'c' if !selected.is_empty() => shell_x11::copy_files(selected, false),
                    'x' if !selected.is_empty() => shell_x11::copy_files(selected, true),
                    'v' => desktop_files::paste(self.dir.clone()),
                    _ => {}
                }
            }
            return Ok(());
        }
        let arrow = match k.key {
            Key::Up => Some(Direction::Up),
            Key::Down => Some(Direction::Down),
            Key::Left => Some(Direction::Left),
            Key::Right => Some(Direction::Right),
            _ => None,
        };
        if let Some(dir) = arrow {
            let from = self.grid.selected_indices().last().copied();
            if let Some(next) = self.grid.neighbor(from, dir) {
                let changed = self.grid.select(Some(next));
                self.cells_dirty.extend(changed);
            }
            return Ok(());
        }
        match k.key {
            Key::Return => {
                for item in self.grid.selected_items() {
                    desktop_files::open(item);
                }
            }
            Key::F2 => {
                if let Some(path) = self.grid.selected().map(|i| i.path.clone()) {
                    self.start_rename(path);
                }
            }
            Key::Delete => {
                for item in self.grid.selected_items() {
                    desktop_files::trash(item);
                }
            }
            Key::Escape => {
                let changed = self.grid.select(None);
                self.cells_dirty.extend(changed);
            }
            // Буквы — к значку с таким началом имени; набор подряд (пауза
            // меньше секунды) продолжает слово, как в Finder.
            Key::Char(c) if !k.alt && !k.logo && !c.is_control() => {
                let again = self.typed_at.elapsed() > TYPE_AGAIN;
                if again {
                    self.typed.clear();
                }
                self.typed.push(c);
                self.typed_at = Instant::now();
                let current = self.grid.selected_indices().first().copied();
                // Новое слово — со следующего значка (та же буква ещё раз —
                // к следующему на неё); продолжение слова — с нынешнего.
                let after = if again {
                    current
                } else {
                    current.and_then(|c| c.checked_sub(1))
                };
                if let Some(i) = self.grid.find_prefix(&self.typed, after) {
                    let changed = self.grid.select(Some(i));
                    self.cells_dirty.extend(changed);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Начать переименование значка с этим путём (поле — в `flush`).
    fn start_rename(&mut self, path: PathBuf) {
        self.rename_start = Some(path);
    }

    fn rename_event(&mut self, d: &Display, ev: Event) -> Result<()> {
        let Some(rename) = &mut self.rename else {
            return Ok(());
        };
        let popup = self.rename_popup.id();
        let result = match ev {
            Event::Key(k) => rename.key(k),
            // Щелчок мимо поля — сохранить, как на Mac.
            Event::Button { window, .. } if window != popup => rename.commit(),
            Event::Expose { window } if window == popup => RenameResult::Redraw,
            _ => RenameResult::Nothing,
        };
        match result {
            RenameResult::Nothing => {}
            RenameResult::Redraw => self.rename_dirty = true,
            RenameResult::Cancel => self.close_rename(d)?,
            RenameResult::Commit(name) => {
                let path = rename.path.clone();
                self.close_rename(d)?;
                match desktop_files::rename(&path, &name) {
                    Ok(target) => {
                        if let Some(old) = path.file_name() {
                            self.grid.rename_saved(&old.to_string_lossy(), &name);
                            desktop_files::save_places(self.grid.saved());
                        }
                        self.pending_select = Some(target);
                    }
                    Err(e) => desktop_files::error(&e),
                }
            }
        }
        Ok(())
    }

    fn close_rename(&mut self, d: &Display) -> Result<()> {
        if self.rename.take().is_some() {
            self.rename_popup.hide(d)?;
            let changed = self.grid.set_renaming(None);
            self.cells_dirty.extend(changed);
        }
        Ok(())
    }

    /// Меню по правому щелчку: на значке — что сделать с ним, на пустом месте
    /// — со столом.
    fn open_menu(
        &mut self,
        d: &Display,
        fonts: &mut Fonts,
        x: i16,
        y: i16,
        on_icon: bool,
    ) -> Result<()> {
        let entry = |label: &str, command, separator_before| MenuEntry {
            label: label.to_string(),
            command,
            checked: false,
            separator_before,
            enabled: true,
            indent: false,
        };
        let entries = if on_icon {
            let mut e = vec![entry("Открыть", DesktopCommand::Open, false)];
            // Переименовать можно только один значок.
            if self.grid.selected().is_some() {
                e.push(entry("Переименовать", DesktopCommand::Rename, true));
            }
            e.push(entry("Копировать", DesktopCommand::Copy, true));
            e.push(entry("Вырезать", DesktopCommand::Cut, false));
            e.push(entry("В корзину", DesktopCommand::Trash, true));
            e
        } else {
            vec![
                entry("Новая папка", DesktopCommand::NewFolder, false),
                entry("Новый текстовый файл", DesktopCommand::NewTextFile, false),
                entry("Вставить", DesktopCommand::Paste, false),
                entry("Упорядочить", DesktopCommand::Arrange, true),
                entry("Открыть терминал здесь", DesktopCommand::Terminal, true),
                entry("Изменить обои…", DesktopCommand::ChangeWallpaper, true),
            ]
        };
        let mut menu = Menu::new("desktop".into(), entries);
        let (w, h) = menu.measure(fonts);
        // Меню под указателем; у края экрана — сдвинуть, чтобы влезло.
        let (sw, sh) = d.screen_size();
        let mx = i32::from(x).min(i32::from(sw) - w as i32 - 4).max(0);
        let my = i32::from(y).min(i32::from(sh) - h as i32 - 4).max(0);
        self.menu_popup
            .show(d, mx as i16, my as i16, w as u16, h as u16)?;
        self.menu = Some(menu);
        self.menu_dirty = true;
        Ok(())
    }

    fn menu_event(&mut self, d: &Display, ev: Event) -> Result<()> {
        let Some(menu) = &mut self.menu else {
            return Ok(());
        };
        let popup = self.menu_popup.id();
        let result = match ev {
            Event::Motion { window, x, y } if window == popup => {
                if menu.motion(f32::from(x), f32::from(y)) {
                    MenuResult::Redraw
                } else {
                    MenuResult::Nothing
                }
            }
            Event::Button { window, x, y, .. } if window == popup => {
                menu.click(f32::from(x), f32::from(y))
            }
            // Щелчок мимо меню — закрыть.
            Event::Button { .. } => MenuResult::Close,
            Event::Key(k) => menu.key(k.key),
            Event::Expose { window } if window == popup => MenuResult::Redraw,
            _ => MenuResult::Nothing,
        };
        match result {
            MenuResult::Nothing => {}
            MenuResult::Redraw => self.menu_dirty = true,
            MenuResult::Close => self.close_menu(d)?,
            MenuResult::Run(command) => {
                self.close_menu(d)?;
                self.run(d, command)?;
            }
        }
        Ok(())
    }

    fn run(&mut self, d: &Display, command: DesktopCommand) -> Result<()> {
        let item = self.grid.selected().cloned();
        match command {
            DesktopCommand::NewFolder => {
                let path = desktop_files::new_folder(&self.dir);
                self.created(path);
            }
            DesktopCommand::NewTextFile => {
                let path = desktop_files::new_text_file(&self.dir);
                self.created(path);
            }
            DesktopCommand::Paste => desktop_files::paste(self.dir.clone()),
            DesktopCommand::Arrange => {
                self.grid.arrange();
                desktop_files::save_places(self.grid.saved());
                self.full_dirty = true;
            }
            DesktopCommand::Terminal => desktop_files::terminal(&self.terminal, &self.dir),
            DesktopCommand::ChangeWallpaper => self.open_picker(d)?,
            DesktopCommand::Open => {
                for item in self.grid.selected_items() {
                    desktop_files::open(item);
                }
            }
            DesktopCommand::Rename => {
                if let Some(item) = &item {
                    self.start_rename(item.path.clone());
                }
            }
            DesktopCommand::Copy | DesktopCommand::Cut => {
                let paths: Vec<PathBuf> = self
                    .grid
                    .selected_items()
                    .iter()
                    .map(|i| i.path.clone())
                    .collect();
                shell_x11::copy_files(paths, command == DesktopCommand::Cut);
            }
            DesktopCommand::Trash => {
                for item in self.grid.selected_items() {
                    desktop_files::trash(item);
                }
            }
        }
        Ok(())
    }

    /// Создали папку или файл: значок — в клетке под правым щелчком (занята
    /// — рядом), выделен, и сразу поле для имени.
    fn created(&mut self, path: Option<PathBuf>) {
        let Some(path) = path else {
            return;
        };
        let slot = self
            .grid
            .free_slot_near(self.grid.slot_at(self.menu_at.0, self.menu_at.1), None);
        if let Some(name) = path.file_name() {
            self.grid.place_name(&name.to_string_lossy(), slot);
            desktop_files::save_places(self.grid.saved());
        }
        self.pending_select = Some(path.clone());
        self.start_rename(path);
    }

    fn close_menu(&mut self, d: &Display) -> Result<()> {
        if self.menu.take().is_some() {
            self.menu_popup.hide(d)?;
        }
        Ok(())
    }

    /// Окно «Обои» посередине экрана; миниатюры рисуются в фоне.
    fn open_picker(&mut self, d: &Display) -> Result<()> {
        let picker = WallpaperPicker::new(WALLPAPERS, &self.wallpaper);
        let (w, h) = picker.size();
        let (sw, sh) = d.screen_size();
        let x = (i32::from(sw) - w as i32) / 2;
        let y = (i32::from(sh) - h as i32) / 3;
        self.picker_popup
            .show(d, x.max(0) as i16, y.max(0) as i16, w as u16, h as u16)?;
        let values = picker.values();
        self.picker = Some(picker);
        self.picker_dirty = true;
        let tx = self.tx.clone();
        let scale = d.scale();
        std::thread::spawn(move || {
            let tw = shell_render::to_px(wallpaper_picker::THUMB_W as u32, scale);
            let th = shell_render::to_px(wallpaper_picker::THUMB_H as u32, scale);
            for value in values {
                if let Some(image) = thumb(&value, tw, th) {
                    let msg = DesktopMsg::Thumb(value, image);
                    if tx.send(Msg::Desktop(Box::new(msg))).is_err() {
                        return;
                    }
                }
            }
        });
        Ok(())
    }

    fn picker_event(&mut self, d: &Display, ev: Event) -> Result<()> {
        let Some(picker) = &mut self.picker else {
            return Ok(());
        };
        let popup = self.picker_popup.id();
        let result = match ev {
            Event::Motion { window, x, y } if window == popup => {
                if picker.motion(f32::from(x), f32::from(y)) {
                    PickerResult::Redraw
                } else {
                    PickerResult::Nothing
                }
            }
            Event::Button { window, x, y, .. } if window == popup => {
                picker.click(f32::from(x), f32::from(y))
            }
            Event::Button { .. } => PickerResult::Close,
            Event::Key(k) => picker.key(k.key),
            Event::Expose { window } if window == popup => PickerResult::Redraw,
            _ => PickerResult::Nothing,
        };
        match result {
            PickerResult::Nothing => {}
            PickerResult::Redraw => self.picker_dirty = true,
            PickerResult::Close => self.close_picker(d)?,
            PickerResult::Apply(value) => {
                self.apply(d, &value);
                if let Some(picker) = &mut self.picker {
                    picker.set_current(&value);
                }
                self.picker_dirty = true;
            }
            PickerResult::PickFile => {
                // Окно выбора файла не получит мышь, пока «Обои» её держат.
                self.close_picker(d)?;
                pick_file(self.tx.clone());
            }
        }
        Ok(())
    }

    fn close_picker(&mut self, d: &Display) -> Result<()> {
        if self.picker.take().is_some() {
            self.picker_popup.hide(d)?;
        }
        Ok(())
    }

    /// Поставить обои: запомнить в настройках, перерисовать стол и фон
    /// блокировки. Служба не перезапускается.
    fn apply(&mut self, d: &Display, value: &str) {
        if let Err(e) = save_wallpaper(value) {
            eprintln!("Обои не сохранены в настройках: {e}");
        }
        self.wallpaper = value.to_string();
        if self.chosen_at.replace(Instant::now()).is_some() {
            self.skipped += 1;
        }
        self.load_wallpaper(d);
    }

    /// Закрыть меню и окно «Обои» (например, перед открытием «Поиска»).
    pub fn close_popups(&mut self, d: &Display) -> Result<()> {
        self.close_menu(d)?;
        self.close_rename(d)?;
        self.close_picker(d)
    }

    pub fn flush(&mut self, d: &Display, fonts: &mut Fonts, icons: &mut Icons) -> Result<()> {
        if self.menu_dirty {
            self.menu_dirty = false;
            if let Some(menu) = &self.menu {
                self.menu_popup.present(d, &menu.render(fonts))?;
            }
        }
        if self.picker_dirty {
            self.picker_dirty = false;
            if let Some(picker) = &self.picker {
                self.picker_popup.present(d, &picker.render(fonts))?;
            }
        }
        self.flush_rename(d, fonts)?;
        self.flush_drag(d, fonts, icons)?;
        self.flush_band(d)?;
        if self.full_dirty {
            self.full_dirty = false;
            self.cells_dirty.clear();
            self.draw_all(d, fonts, icons)?;
        } else if !self.cells_dirty.is_empty() {
            let mut cells = std::mem::take(&mut self.cells_dirty);
            cells.sort_unstable();
            cells.dedup();
            for i in cells {
                self.draw_cell(d, fonts, icons, i)?;
            }
        }
        Ok(())
    }

    /// Поле переименования: открыть (когда значок появился) или перерисовать.
    fn flush_rename(&mut self, d: &Display, fonts: &mut Fonts) -> Result<()> {
        if let Some(path) = self.rename_start.take() {
            match self.grid.items().iter().position(|i| i.path == path) {
                Some(i) => {
                    let is_dir = self.grid.items()[i].kind == Kind::Folder;
                    self.close_menu(d)?;
                    let changed = self.grid.set_renaming(Some(path.clone()));
                    self.cells_dirty.extend(changed);
                    self.rename = Some(RenameBox::new(path, is_dir));
                    self.rename_dirty = true;
                    self.place_rename(d, fonts, i)?;
                }
                // Ещё не появился в списке — подождём.
                None => self.rename_start = Some(path),
            }
        }
        if self.rename_dirty {
            self.rename_dirty = false;
            if let Some(rename) = &self.rename {
                // Ширина поля растёт с текстом.
                let (w, h) = rename.size(fonts);
                if let Some(i) = self.grid.items().iter().position(|i| i.path == rename.path) {
                    let (x, y) = self.rename_pos(i, w);
                    self.rename_popup
                        .set_geometry(d, x, y, w as u16, h as u16)?;
                }
                self.rename_popup.present(d, &rename.render(fonts))?;
            }
        }
        Ok(())
    }

    /// Где поле переименования: по середине клетки, на месте подписи.
    fn rename_pos(&self, i: usize, w: u32) -> (i16, i16) {
        let (lx, ly, lw, _) = self.grid.label_rect(i);
        (
            (lx + (lw - w as f32) / 2.0).round() as i16,
            (ly - 3.0).round() as i16,
        )
    }

    fn place_rename(&mut self, d: &Display, fonts: &mut Fonts, i: usize) -> Result<()> {
        let Some(rename) = &self.rename else {
            return Ok(());
        };
        let (w, h) = rename.size(fonts);
        let (x, y) = self.rename_pos(i, w);
        // Поле забирает клавиатуру и мышь: щелчок мимо — сохранить.
        self.rename_popup.show(d, x, y, w as u16, h as u16)
    }

    /// Рамка выделения. Окно рамки — прозрачное во весь экран (показывается
    /// один раз), на каждое движение перерисовывается только участок, где
    /// рамка была и где стала: окно не меняет размер — рамка не мерцает.
    fn flush_band(&mut self, d: &Display) -> Result<()> {
        if !self.band_dirty {
            return Ok(());
        }
        self.band_dirty = false;
        let Some((start, end)) = self.band else {
            return Ok(());
        };
        let (sw, sh) = d.screen_size();
        if !self.band_shown {
            self.band_popup.show_passive(d, 0, 0, sw, sh)?;
            self.band_shown = true;
        }
        let rect = band_rect(start, end);
        if let Some(renderer) = &mut self.selection_renderer {
            renderer.paint(d, rect)?;
            return Ok(());
        }
        // Совместимость с X-серверами без XRender/XFixes.
        // Участок: старая рамка и новая (с запасом на край и сглаживание).
        let (mut x0, mut y0) = (rect.0, rect.1);
        let (mut x1, mut y1) = (rect.0 + rect.2, rect.1 + rect.3);
        if let Some((lx, ly, lw, lh)) = self.band_last {
            x0 = x0.min(lx);
            y0 = y0.min(ly);
            x1 = x1.max(lx + lw);
            y1 = y1.max(ly + lh);
        }
        let x0 = (x0 - 2.0).floor().max(0.0);
        let y0 = (y0 - 2.0).floor().max(0.0);
        let x1 = (x1 + 2.0).ceil().min(f32::from(sw));
        let y1 = (y1 + 2.0).ceil().min(f32::from(sh));
        let (w, h) = (((x1 - x0) as u32).max(1), ((y1 - y0) as u32).max(1));
        let mut pm = Canvas::new(w, h).expect("размер не нулевой");
        let (bx, by, bw, bh) = (rect.0 - x0, rect.1 - y0, rect.2, rect.3);
        let id = tiny_skia::Transform::identity();
        if let Some(r) = tiny_skia::Rect::from_xywh(bx, by, bw.max(1.0), bh.max(1.0)) {
            pm.fill_rect(r, &paint(BAND_FILL), id, None);
        }
        if let Some(r) =
            tiny_skia::Rect::from_xywh(bx + 0.5, by + 0.5, (bw - 1.0).max(0.5), (bh - 1.0).max(0.5))
        {
            let path = tiny_skia::PathBuilder::from_rect(r);
            let stroke = tiny_skia::Stroke {
                width: 1.0,
                ..tiny_skia::Stroke::default()
            };
            pm.stroke_path(&path, &paint(BAND_EDGE), &stroke, id, None);
        }
        let k = d.scale();
        self.band_popup
            .present_at(d, &pm, (x0 * k).round() as i32, (y0 * k).round() as i32)?;
        self.band_last = Some(rect);
        Ok(())
    }

    /// Картинка значка, который тащат, — за указателем.
    fn flush_drag(&mut self, d: &Display, fonts: &mut Fonts, icons: &mut Icons) -> Result<()> {
        if !self.drag_dirty {
            return Ok(());
        }
        self.drag_dirty = false;
        let Some(press) = &self.press else {
            return Ok(());
        };
        let i = press.index;
        let Some(item) = self.grid.items().get(i) else {
            return Ok(());
        };
        let (cx, cy, cw, ch) = self.grid.cell(i);
        let mut pm = Canvas::new(cw as u32, ch as u32).expect("размер не нулевой");
        let icon = item_icon(icons, item);
        self.grid.draw(&mut pm, (cx, cy), i, icon.as_ref(), fonts);
        // Полупрозрачная, как на Mac.
        for px in pm.pixmap_mut().data_mut() {
            *px = (u16::from(*px) * 3 / 4) as u8;
        }
        let (x, y) = (self.drag_at.0 - press.grab.0, self.drag_at.1 - press.grab.1);
        self.drag_popup.show_passive(
            d,
            x.round() as i16,
            y.round() as i16,
            cw as u16,
            ch as u16,
        )?;
        self.drag_popup.present(d, &pm)
    }

    /// Весь стол: обои и все значки — новой картинкой-фоном.
    fn draw_all(&self, d: &Display, fonts: &mut Fonts, icons: &mut Icons) -> Result<()> {
        let Some(wall) = &self.wall else {
            return Ok(());
        };
        let start = Instant::now();
        let mut pm = Canvas::from_pixmap(wall.clone(), d.scale());
        for i in 0..self.grid.items().len() {
            let icon = item_icon(icons, &self.grid.items()[i]);
            self.grid.draw(&mut pm, (0.0, 0.0), i, icon.as_ref(), fonts);
        }
        self.window.set_wallpaper(d, pm.pixmap())?;
        if std::env::var_os("SHELL_TIMING").is_some() {
            eprintln!(
                "Стол: {} значков, перерисован за {:?}",
                self.grid.items().len(),
                start.elapsed()
            );
        }
        Ok(())
    }

    /// Только клетка значка `i`: кусок обоев и значок на нём.
    fn draw_cell(&self, d: &Display, fonts: &mut Fonts, icons: &mut Icons, i: usize) -> Result<()> {
        let (Some(wall), Some(item)) = (&self.wall, self.grid.items().get(i)) else {
            return Ok(());
        };
        let k = d.scale();
        let (cx, cy, cw, ch) = self.grid.cell(i);
        let px = |v: f32| (v * k).round() as i32;
        let (x0, y0) = (px(cx).max(0), px(cy).max(0));
        let (x1, y1) = (
            px(cx + cw).min(wall.width() as i32),
            px(cy + ch).min(wall.height() as i32),
        );
        let Some(rect) = IntRect::from_ltrb(x0, y0, x1, y1) else {
            return Ok(());
        };
        let Some(piece) = wall.clone_rect(rect) else {
            return Ok(());
        };
        let mut pm = Canvas::from_pixmap(piece, k);
        let icon = item_icon(icons, item);
        self.grid.draw(
            &mut pm,
            (x0 as f32 / k, y0 as f32 / k),
            i,
            icon.as_ref(),
            fonts,
        );
        self.window.update(d, pm.pixmap(), x0, y0)
    }
}

/// Буква сочетания по месту на клавиатуре: в русской раскладке Ctrl+Ф —
/// это Ctrl+A, Ctrl+С — Ctrl+C и так далее (ЙЦУКЕН → QWERTY).
fn latin(c: char) -> char {
    const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбю";
    const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.";
    let c = c.to_lowercase().next().unwrap_or(c);
    RU.chars()
        .position(|r| r == c)
        .and_then(|i| EN.chars().nth(i))
        .unwrap_or(c)
}

/// Рамка выделения: светлая полупрозрачная с краем, как на Mac.
const BAND_FILL: [u8; 4] = [0xff, 0xff, 0xff, 0x28];
const BAND_EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x90];

/// Прямоугольник рамки между точкой начала и указателем.
fn band_rect(a: (f32, f32), b: (f32, f32)) -> (f32, f32, f32, f32) {
    (
        a.0.min(b.0),
        a.1.min(b.1),
        (a.0 - b.0).abs(),
        (a.1 - b.1).abs(),
    )
}

/// Картинка значка: первое найденное имя из списка.
fn item_icon(icons: &mut Icons, item: &Item) -> Option<Pixmap> {
    item.icons
        .iter()
        .find_map(|name| icons.get(name, icon_grid::ICON).cloned())
}
