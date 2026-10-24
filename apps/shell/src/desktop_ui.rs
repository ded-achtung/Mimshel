//! Рабочий стол (вместо xfdesktop): окно под всеми окнами с обоями на
//! каждом мониторе и значками файлов из папки «Рабочий стол», меню по правому
//! щелчку и окно «Обои».
//!
//! - значки: щелчок выделяет, двойной открывает; меню на значке — «Открыть»,
//!   «Переименовать…», «В корзину»; на пустом месте — «Новая папка»,
//!   «Вставить», «Открыть терминал здесь», «Изменить обои…»;
//! - обои — в `desktop_wallpaper`, действия с файлами — в `desktop_files`.
//!
//! Картинка стола — обои со значками — лежит на X-сервере фоном окна. Новые
//! обои или другой список файлов — стол отправляется целиком; выделение —
//! только клетки затронутых значков.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use shell_core::config::Config;
use shell_desktop::files::Item;
use shell_desktop::icon_grid::{self, IconGrid};
use shell_desktop::{PickerResult, WallpaperPicker, wallpaper_picker};
use shell_icons::Icons;
use shell_render::Canvas;
use shell_text::Fonts;
use shell_widgets::{Menu, MenuEntry, MenuResult};
use shell_x11::{DesktopWindow, Display, Event, Popup};
use tiny_skia::{IntRect, Pixmap};

use crate::desktop_files;
use crate::desktop_wallpaper::{WALLPAPERS, painter, pick_file, save_wallpaper, thumb};
use crate::ui::{Msg, Result};

/// Пункты меню стола и значков.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DesktopCommand {
    // На пустом месте.
    NewFolder,
    Paste,
    Terminal,
    ChangeWallpaper,
    // На значке.
    Open,
    Rename,
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
    /// Выделить этот файл, когда он появится (переименовали, создали).
    Select(PathBuf),
}

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
        window.show(d)?;
        let dir = desktop_files::desktop_dir();
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
            full_dirty: false,
            cells_dirty: Vec::new(),
            terminal: config.terminal.clone(),
            menubar_height: config.menubar_height,
            chosen_at: None,
            skipped: 0,
            painter: painter(tx.clone()),
            tx: tx.clone(),
        };
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
        let top = my + self.menubar_height as f32 + MARGIN;
        self.grid.set_area((
            mx,
            top,
            mw - MARGIN,
            (my + mh - DOCK_ROOM - top).max(icon_grid::CELL_H),
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
            DesktopMsg::Select(path) => {
                let changed = self.grid.select_path(&path);
                if self.grid.selected().is_none_or(|i| i.path != path) {
                    self.pending_select = Some(path);
                }
                self.cells_dirty.extend(changed);
            }
        }
        Ok(())
    }

    pub fn wants(&self, d: &Display, ev: &Event) -> bool {
        let own = |w: u32| {
            w == self.window.id() || w == self.menu_popup.id() || w == self.picker_popup.id()
        };
        let open = self.menu.is_some() || self.picker.is_some();
        match *ev {
            Event::Button { window, .. }
            | Event::Release { window, .. }
            | Event::Motion { window, .. }
            | Event::Expose { window } => own(window) || open,
            Event::Key(_) => open,
            // Сменился размер экрана (корневое окно) — стол во весь экран заново.
            Event::Configure { window } => window == d.root(),
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
        if self.picker.is_some() {
            return self.picker_event(d, ev);
        }
        if self.menu.is_some() {
            return self.menu_event(d, ev);
        }
        let Event::Button {
            window,
            x,
            y,
            button,
        } = ev
        else {
            return Ok(());
        };
        if window != self.window.id() {
            return Ok(());
        }
        let hit = self.grid.item_at(f32::from(x), f32::from(y));
        let changed = self.grid.select(hit);
        self.cells_dirty.extend(changed);
        match button {
            1 => {
                let now = Instant::now();
                let double = hit.is_some()
                    && self
                        .last_click
                        .is_some_and(|(i, t)| Some(i) == hit && now - t < DOUBLE_CLICK);
                if double {
                    self.last_click = None;
                    if let Some(item) = self.grid.selected() {
                        desktop_files::open(item);
                    }
                } else {
                    self.last_click = hit.map(|i| (i, now));
                }
            }
            3 => self.open_menu(d, fonts, x, y, hit.is_some())?,
            _ => {}
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
            vec![
                entry("Открыть", DesktopCommand::Open, false),
                entry("Переименовать…", DesktopCommand::Rename, true),
                entry("В корзину", DesktopCommand::Trash, true),
            ]
        } else {
            vec![
                entry("Новая папка", DesktopCommand::NewFolder, false),
                entry("Вставить", DesktopCommand::Paste, false),
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
                if let Some(path) = desktop_files::new_folder(&self.dir) {
                    self.pending_select = Some(path);
                }
            }
            DesktopCommand::Paste => desktop_files::paste(self.dir.clone()),
            DesktopCommand::Terminal => desktop_files::terminal(&self.terminal, &self.dir),
            DesktopCommand::ChangeWallpaper => self.open_picker(d)?,
            DesktopCommand::Open => {
                if let Some(item) = &item {
                    desktop_files::open(item);
                }
            }
            DesktopCommand::Rename => {
                if let Some(item) = &item {
                    desktop_files::rename(item, self.tx.clone());
                }
            }
            DesktopCommand::Trash => {
                if let Some(item) = &item {
                    desktop_files::trash(item);
                }
            }
        }
        Ok(())
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

/// Картинка значка: первое найденное имя из списка.
fn item_icon(icons: &mut Icons, item: &Item) -> Option<Pixmap> {
    item.icons
        .iter()
        .find_map(|name| icons.get(name, icon_grid::ICON).cloned())
}
