//! Окна папок — свой Проводник в службе (часть `parts.folders`). Окно
//! открывается сразу: шрифты и значки уже в памяти службы, а список файлов
//! читается в потоке. Сам вид окна — `mimshel_folder::FolderView`; здесь —
//! окна на экране, слежение за папкой (inotify), меню по правому щелчку,
//! переименование полем, перетаскивание (внутри окна, в другие окна и на
//! стол — по XDND) и действия с файлами (общие со столом, `desktop_files`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use inotify::{Inotify, WatchMask};
use mimshel_core::config::Config;
use mimshel_desktop::files::{Item, MimeDb, scan, scan_with};
use mimshel_desktop::rename_box::{RenameBox, RenameResult};
use mimshel_folder::view::{DEFAULT_SIZE, MIN_SIZE};
use mimshel_folder::{Action, FolderView, Place};
use mimshel_icons::Icons;
use mimshel_text::Fonts;
use mimshel_widgets::{Menu, MenuEntry, MenuResult};
use mimshel_x11::{AppWindow, Display, DragTarget, Event, Popup};

use crate::desktop_files;
use crate::ui::{Msg, Result};

/// Класс окна (`WM_CLASS`): по нему док и строка меню узнают Проводник
/// (`StartupWMClass` в ярлыке `mimshel-folder.desktop`).
pub const WINDOW_CLASS: &str = "mimshel-folder";
/// Ярлык Проводника (без `.desktop`) — его пишет `mimshel apply folders`.
pub const DESKTOP_ID: &str = "mimshel-folder";
/// После события в папке — подождать тишины и тогда перечитать.
const QUIET: Duration = Duration::from_millis(150);
/// Как часто поток слежения проверяет, не закрыли ли окно.
const POLL: Duration = Duration::from_millis(250);

/// Что приходит окнам папок из потоков.
pub enum FolderMsg {
    /// Список файлов папки `dir` окна `window` (сразу и после перемен).
    Files {
        window: u32,
        dir: PathBuf,
        items: Vec<Item>,
        /// Свободное место на диске папки, байт.
        free: Option<u64>,
    },
    /// Подключили или извлекли диск — перечитать места в боковой панели.
    Places,
    /// Phones and cameras found by a background scan. `open` — the window
    /// that asked to mount this location: open the phone there.
    Phones {
        list: Vec<Place>,
        open: Option<(u32, String)>,
    },
}

/// Пункты меню по правому щелчку.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Command {
    Open,
    OpenWindow,
    Rename,
    Copy,
    Cut,
    Trash,
    NewFolder,
    NewTextFile,
    Paste,
    Terminal,
}

struct FolderWindow {
    win: AppWindow,
    view: FolderView,
    /// Остановить поток слежения за нынешней папкой.
    stop: Arc<AtomicBool>,
    dirty: bool,
    /// Когда попросили открыть — для замера в журнале (до первого кадра со
    /// списком файлов).
    opened: Option<std::time::Instant>,
}

/// Перетаскивание значков из окна: какое окно, где его угол на экране.
struct Drag {
    window: u32,
    origin: (f32, f32),
    at: (f32, f32),
}

pub struct FolderUi {
    windows: Vec<FolderWindow>,
    /// Places and disks plus `phones` — what the sidebars show.
    places: Vec<Place>,
    /// Phones and cameras (gvfs): scanned in a background thread.
    phones: Vec<Place>,
    /// A phone was just mounted for this window: open it on the next flush.
    open_phone: Option<(u32, PathBuf)>,
    /// Особые папки (Документы, Загрузки…) — своими значками в сетке.
    special: Vec<(PathBuf, &'static str)>,
    mime: Arc<MimeDb>,
    menu_popup: Popup,
    /// Открытое меню: в каком окне.
    menu: Option<(u32, Menu<Command>)>,
    menu_dirty: bool,
    rename_popup: Popup,
    rename: Option<(u32, RenameBox)>,
    rename_dirty: bool,
    /// Начать переименование, когда файл появится в списке окна.
    rename_start: Option<(u32, PathBuf)>,
    drag_popup: Popup,
    drag: Option<Drag>,
    drag_dirty: bool,
    terminal: Vec<String>,
    language: mimshel_core::i18n::Language,
    tx: Sender<Msg>,
}

impl FolderUi {
    pub fn new(d: &Display, config: &Config, tx: Sender<Msg>) -> Result<FolderUi> {
        let drag_popup = d.create_popup("folder-drag")?;
        // Картинка, которую тащат, не заслоняет окно под указателем.
        drag_popup.pass_through(d)?;
        Ok(FolderUi {
            windows: Vec::new(),
            places: mimshel_folder::places_lang(config.language.resolve()),
            phones: Vec::new(),
            open_phone: None,
            special: mimshel_folder::special_folders(),
            mime: Arc::new(MimeDb::load()),
            menu_popup: d.create_popup("folder-menu")?,
            menu: None,
            menu_dirty: false,
            rename_popup: d.create_popup("folder-rename")?,
            rename: None,
            rename_dirty: false,
            rename_start: None,
            drag_popup,
            drag: None,
            drag_dirty: false,
            terminal: crate::ui::terminal_command(config),
            language: config.language,
            tx,
        })
        .inspect(|ui| scan_phones(ui.tx.clone()))
    }

    /// Значки окна папки — заранее: первое окно открывается без задержки.
    pub fn preload(&self, icons: &mut Icons) {
        mimshel_folder::view::preload(icons, &self.places);
    }

    /// Нарисовать окно папки невидимо (буквы всех размеров окна попадут в
    /// кеш шрифтов).
    pub fn warm_up(&self, fonts: &mut Fonts, icons: &mut Icons) {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let mut view = FolderView::new(home, self.places.clone(), DEFAULT_SIZE);
        view.set_language(self.language);
        let locale = mimshel_apps::Locale::from_env();
        let mut items = scan(view.dir(), &self.mime, locale.as_ref());
        items.truncate(40);
        view.set_items(items);
        view.render(fonts, icons);
    }

    /// Открыть папку в новом окне.
    pub fn open(&mut self, d: &Display, dir: PathBuf) -> Result<()> {
        let opened = std::time::Instant::now();
        // A phone may have been plugged in while no window was watching.
        scan_phones(self.tx.clone());
        let mut view = FolderView::new(dir.clone(), self.places.clone(), DEFAULT_SIZE);
        view.set_language(self.language);
        view.set_special(self.special.clone());
        let win = d.create_app_window(WINDOW_CLASS, &view.title(), DEFAULT_SIZE, MIN_SIZE)?;
        win.show(d)?;
        if mimshel_core::names::flag("TIMING") {
            eprintln!("Окно папки: создано за {:?}", opened.elapsed());
        }
        let stop = Arc::new(AtomicBool::new(false));
        watch(
            win.id(),
            dir,
            false,
            self.mime.clone(),
            stop.clone(),
            self.tx.clone(),
        );
        self.windows.push(FolderWindow {
            win,
            view,
            stop,
            dirty: true,
            opened: Some(opened),
        });
        Ok(())
    }

    fn update_places(&mut self) {
        self.places = mimshel_folder::places_lang(self.language.resolve());
        self.places.extend(self.phones.iter().cloned());
        for w in &mut self.windows {
            if w.view.set_places(self.places.clone()) {
                w.dirty = true;
            }
        }
    }

    fn find(&mut self, window: u32) -> Option<&mut FolderWindow> {
        self.windows.iter_mut().find(|w| w.win.id() == window)
    }

    fn own(&self, window: u32) -> bool {
        self.windows.iter().any(|w| w.win.id() == window)
    }

    pub fn wants(&self, d: &Display, ev: &Event) -> bool {
        let popups = |w: u32| w == self.menu_popup.id() || w == self.rename_popup.id();
        let open = self.menu.is_some() || self.rename.is_some() || self.drag.is_some();
        match *ev {
            Event::Button { window, .. }
            | Event::Release { window, .. }
            | Event::Motion { window, .. }
            | Event::Expose { window } => self.own(window) || popups(window) || open,
            Event::Key(_) => {
                open || (!self.windows.is_empty() && d.active_window().is_some_and(|w| self.own(w)))
            }
            Event::Leave { window }
            | Event::Configure { window }
            | Event::Close { window }
            | Event::DropHover { window, .. }
            | Event::DropLeave { window }
            | Event::Dropped { window, .. } => self.own(window),
            _ => false,
        }
    }

    pub fn message(&mut self, msg: FolderMsg) {
        match msg {
            FolderMsg::Places => {
                self.update_places();
                scan_phones(self.tx.clone());
            }
            FolderMsg::Phones { list, open } => {
                if let Some((window, uri)) = open
                    && let Some(p) = list.iter().find(|p| p.remote.as_ref() == Some(&uri))
                {
                    self.open_phone = Some((window, p.path.clone()));
                }
                self.phones = list;
                self.update_places();
            }
            FolderMsg::Files {
                window,
                dir,
                items,
                free,
            } => {
                if mimshel_core::names::flag("TIMING")
                    && let Some(w) = self.windows.iter().find(|w| w.win.id() == window)
                {
                    eprintln!(
                        "Окно папки: список пришёл через {:?}",
                        w.opened.map(|o| o.elapsed())
                    );
                }
                if let Some(w) = self.find(window)
                    && w.view.dir() == dir
                {
                    w.view.set_items(items);
                    w.view.set_free(free);
                    w.dirty = true;
                }
            }
        }
    }

    pub fn event(&mut self, d: &Display, fonts: &mut Fonts, ev: Event) -> Result<()> {
        if self.rename.is_some() {
            return self.rename_event(d, ev);
        }
        if self.menu.is_some() {
            return self.menu_event(d, fonts, ev);
        }
        match ev {
            Event::Expose { window } => {
                if let Some(w) = self.find(window) {
                    w.dirty = true;
                }
            }
            Event::Configure { window } => {
                if let Some(w) = self.find(window)
                    && let Some(size) = w.win.size(d)
                    && w.view.set_size(size)
                {
                    w.dirty = true;
                }
            }
            Event::Close { window } => self.close(d, window)?,
            Event::Leave { window } => {
                if let Some(w) = self.find(window) {
                    let action = w.view.leave();
                    self.act(d, fonts, window, action)?;
                }
            }
            Event::Button {
                window,
                x,
                y,
                button,
            } => {
                let (ctrl, shift) = d.modifiers();
                if let Some(w) = self.find(window) {
                    let action = w
                        .view
                        .press(f32::from(x), f32::from(y), button, ctrl, shift);
                    self.act(d, fonts, window, action)?;
                }
            }
            Event::Motion { window, x, y } => {
                if let Some(w) = self.find(window) {
                    let action = w.view.motion(f32::from(x), f32::from(y));
                    self.act(d, fonts, window, action)?;
                }
            }
            Event::Release { window, x, y, .. } => {
                if let Some(w) = self.find(window) {
                    let action = w.view.release(f32::from(x), f32::from(y));
                    self.act(d, fonts, window, action)?;
                }
            }
            Event::Key(k) => {
                if let Some(window) = d.active_window().filter(|&w| self.own(w))
                    && let Some(w) = self.find(window)
                {
                    let action = w.view.key(k);
                    self.act(d, fonts, window, action)?;
                }
            }
            Event::DropHover { window, x, y } => {
                if let Some(w) = self.find(window) {
                    let target = w.view.folder_at(f32::from(x), f32::from(y), false);
                    if w.view.set_drop_target(target) {
                        w.dirty = true;
                    }
                }
            }
            Event::DropLeave { window } => {
                if let Some(w) = self.find(window)
                    && w.view.set_drop_target(None)
                {
                    w.dirty = true;
                }
            }
            Event::Dropped {
                window,
                x,
                y,
                moving,
                ticket,
            } => self.dropped(d, window, f32::from(x), f32::from(y), moving, ticket),
            _ => {}
        }
        Ok(())
    }

    /// Выполнить то, что попросил вид окна `window`.
    fn act(&mut self, d: &Display, fonts: &mut Fonts, window: u32, action: Action) -> Result<()> {
        let Some(w) = self.find(window) else {
            return Ok(());
        };
        match action {
            Action::Nothing => {}
            Action::Redraw => w.dirty = true,
            // Новая папка или показ скрытых: прежнее слежение — остановить,
            // новое — начать.
            Action::Load | Action::ToggleHidden => {
                w.stop.store(true, Ordering::Relaxed);
                w.stop = Arc::new(AtomicBool::new(false));
                w.win.set_title(d, &w.view.title())?;
                w.dirty = true;
                let dir = w.view.dir().to_path_buf();
                let (hidden, stop) = (w.view.show_hidden(), w.stop.clone());
                watch(
                    window,
                    dir,
                    hidden,
                    self.mime.clone(),
                    stop,
                    self.tx.clone(),
                );
            }
            Action::Eject(path) => {
                w.dirty = true;
                let remote = self
                    .places
                    .iter()
                    .find(|p| p.path == path)
                    .and_then(|p| p.remote.clone());
                eject(path, remote, self.tx.clone());
            }
            Action::Mount(uri) => {
                let name = self
                    .places
                    .iter()
                    .find(|p| p.remote.as_ref() == Some(&uri))
                    .map_or_else(|| uri.clone(), |p| p.name.clone());
                mount(window, uri, name, self.tx.clone());
            }
            Action::Open(items) => {
                for item in &items {
                    desktop_files::open(item);
                }
            }
            Action::Menu { x, y, on_item } => {
                w.dirty = true;
                let origin = w.win.origin(d).unwrap_or((0, 0));
                let at = (
                    origin.0.saturating_add(x as i16),
                    origin.1.saturating_add(y as i16),
                );
                self.open_menu(d, fonts, window, at, on_item)?;
            }
            Action::DragStart => {
                let paths: Vec<PathBuf> = w
                    .view
                    .selected_items()
                    .into_iter()
                    .map(|i| i.path)
                    .collect();
                let origin = w.win.origin(d).unwrap_or((0, 0));
                d.drag_begin(window, &paths);
                self.drag = Some(Drag {
                    window,
                    origin: (f32::from(origin.0), f32::from(origin.1)),
                    at: (0.0, 0.0),
                });
            }
            Action::DragMove { x, y } => self.drag_move(d, window, x, y),
            Action::Drop { x, y } => self.drag_drop(d, window, x, y)?,
            Action::Rename(path) => self.rename_start = Some((window, path)),
            Action::Trash(paths) => trash(&paths),
            Action::Copy(paths) => mimshel_x11::copy_files(paths, false),
            Action::Cut(paths) => mimshel_x11::copy_files(paths, true),
            Action::Paste => desktop_files::paste(w.view.dir().to_path_buf()),
            Action::NewFolder => {
                let dir = w.view.dir().to_path_buf();
                self.created(window, desktop_files::new_folder(&dir));
            }
            Action::NewWindow => {
                let dir = w.view.dir().to_path_buf();
                self.open(d, dir)?;
            }
            Action::Close => self.close(d, window)?,
        }
        Ok(())
    }

    /// Создали папку или файл в окне: выделить и сразу поле для имени.
    fn created(&mut self, window: u32, path: Option<PathBuf>) {
        let Some(path) = path else {
            return;
        };
        if let Some(w) = self.find(window) {
            w.view.select_later(path.clone());
        }
        self.rename_start = Some((window, path));
    }

    fn close(&mut self, d: &Display, window: u32) -> Result<()> {
        if let Some(i) = self.windows.iter().position(|w| w.win.id() == window) {
            let w = self.windows.remove(i);
            w.stop.store(true, Ordering::Relaxed);
            w.win.destroy(d)?;
        }
        if self.rename.as_ref().is_some_and(|(w, _)| *w == window) {
            self.close_rename(d)?;
        }
        if self.menu.as_ref().is_some_and(|(w, _)| *w == window) {
            self.close_menu(d)?;
        }
        Ok(())
    }

    // ---- Перетаскивание из окна ----

    fn drag_move(&mut self, d: &Display, window: u32, x: f32, y: f32) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        drag.at = (x, y);
        self.drag_dirty = true;
        let (sx, sy) = (drag.origin.0 + x, drag.origin.1 + y);
        let over = d.drag_motion(sx.round() as i16, sy.round() as i16, d.modifiers().0);
        if let Some(w) = self.find(window) {
            // Над своим окном — подсветить папку, куда упадут.
            let target = (over == DragTarget::Own)
                .then(|| w.view.folder_at(x, y, true))
                .flatten();
            if w.view.set_drop_target(target) {
                w.dirty = true;
            }
        }
    }

    /// Отпустили значки: над другой программой (или другим окном, столом) —
    /// файлы её; над своим окном — в папку под указателем или в место
    /// боковой панели; иначе — ничего (значки остаются на месте).
    fn drag_drop(&mut self, d: &Display, window: u32, x: f32, y: f32) -> Result<()> {
        self.drag = None;
        self.drag_popup.hide_passive(d)?;
        let over = d.drag_end();
        let Some(w) = self.find(window) else {
            return Ok(());
        };
        w.view.set_drop_target(None);
        w.dirty = true;
        if over != DragTarget::Own {
            return Ok(());
        }
        let target = w
            .view
            .folder_at(x, y, true)
            .map(|i| w.view.items()[i].path.clone())
            .or_else(|| w.view.place_at(x, y).map(Path::to_path_buf));
        let Some(folder) = target.filter(|f| f != w.view.dir()) else {
            return Ok(());
        };
        let copy = d.modifiers().0;
        let pairs = pairs_into(
            w.view.selected_items().into_iter().map(|i| i.path),
            &folder,
            !copy,
        );
        desktop_files::transfer_async(pairs, !copy);
        Ok(())
    }

    /// Бросили файлы из другой программы или окна: в папку под указателем,
    /// в место боковой панели или в папку окна.
    fn dropped(&mut self, d: &Display, window: u32, x: f32, y: f32, moving: bool, ticket: u64) {
        let paths = d.take_drop(ticket);
        let Some(w) = self.find(window) else {
            d.finish_drop(ticket, false);
            return;
        };
        w.view.set_drop_target(None);
        w.dirty = true;
        let into = w
            .view
            .folder_at(x, y, false)
            .map(|i| w.view.items()[i].path.clone())
            .or_else(|| w.view.place_at(x, y).map(Path::to_path_buf))
            .unwrap_or_else(|| w.view.dir().to_path_buf());
        if paths.is_empty() {
            d.finish_drop(ticket, false);
            return;
        }
        let pairs = pairs_into(paths.into_iter(), &into, moving);
        desktop_files::bring(pairs, moving, ticket, self.tx.clone());
    }

    // ---- Меню ----

    fn open_menu(
        &mut self,
        d: &Display,
        fonts: &mut Fonts,
        window: u32,
        (x, y): (i16, i16),
        on_item: bool,
    ) -> Result<()> {
        let entry = |label: &str, command, separator_before| MenuEntry {
            label: label.to_string(),
            command,
            checked: false,
            separator_before,
            enabled: true,
            indent: false,
        };
        let one = self
            .windows
            .iter()
            .find(|w| w.win.id() == window)
            .is_some_and(|w| w.view.selected_items().len() == 1);
        let entries = if on_item {
            let mut e = vec![
                entry(
                    mimshel_core::i18n::t(self.language, "folder.open"),
                    Command::Open,
                    false,
                ),
                entry(
                    mimshel_core::i18n::t(self.language, "folder.open_in_new_window"),
                    Command::OpenWindow,
                    false,
                ),
            ];
            if one {
                e.push(entry(
                    mimshel_core::i18n::t(self.language, "folder.rename"),
                    Command::Rename,
                    true,
                ));
            }
            e.push(entry(
                mimshel_core::i18n::t(self.language, "folder.copy"),
                Command::Copy,
                true,
            ));
            e.push(entry(
                mimshel_core::i18n::t(self.language, "folder.cut"),
                Command::Cut,
                false,
            ));
            e.push(entry(
                mimshel_core::i18n::t(self.language, "folder.move_to_trash"),
                Command::Trash,
                true,
            ));
            e
        } else {
            vec![
                entry(
                    mimshel_core::i18n::t(self.language, "folder.new_folder"),
                    Command::NewFolder,
                    false,
                ),
                entry(
                    mimshel_core::i18n::t(self.language, "folder.new_text_file"),
                    Command::NewTextFile,
                    false,
                ),
                entry(
                    mimshel_core::i18n::t(self.language, "folder.paste"),
                    Command::Paste,
                    true,
                ),
                entry(
                    mimshel_core::i18n::t(self.language, "folder.open_terminal_here"),
                    Command::Terminal,
                    true,
                ),
            ]
        };
        let mut menu = Menu::new("folder".into(), entries);
        let (w, h) = menu.measure(fonts);
        let (sw, sh) = d.screen_size();
        let mx = i32::from(x).min(i32::from(sw) - w as i32 - 4).max(0);
        let my = i32::from(y).min(i32::from(sh) - h as i32 - 4).max(0);
        self.menu_popup
            .show(d, mx as i16, my as i16, w as u16, h as u16)?;
        self.menu = Some((window, menu));
        self.menu_dirty = true;
        Ok(())
    }

    fn menu_event(&mut self, d: &Display, fonts: &mut Fonts, ev: Event) -> Result<()> {
        if let Event::Expose { window } = ev
            && let Some(w) = self.find(window)
        {
            w.dirty = true;
            return Ok(());
        }
        let Some((window, menu)) = &mut self.menu else {
            return Ok(());
        };
        let window = *window;
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
                self.run(d, fonts, window, command)?;
            }
        }
        Ok(())
    }

    fn run(&mut self, d: &Display, fonts: &mut Fonts, window: u32, command: Command) -> Result<()> {
        let Some(w) = self.find(window) else {
            return Ok(());
        };
        let items = w.view.selected_items();
        let paths: Vec<PathBuf> = items.iter().map(|i| i.path.clone()).collect();
        let dir = w.view.dir().to_path_buf();
        match command {
            Command::Open => {
                let action = w
                    .view
                    .key(mimshel_x11::KeyPress::plain(mimshel_x11::Key::Return));
                self.act(d, fonts, window, action)?;
            }
            Command::OpenWindow => {
                for item in items {
                    if item.kind == mimshel_desktop::files::Kind::Folder {
                        self.open(d, item.path)?;
                    } else {
                        desktop_files::open(&item);
                    }
                }
            }
            Command::Rename => {
                if let [path] = &paths[..] {
                    self.rename_start = Some((window, path.clone()));
                }
            }
            Command::Copy => mimshel_x11::copy_files(paths, false),
            Command::Cut => mimshel_x11::copy_files(paths, true),
            Command::Trash => trash(&paths),
            Command::NewFolder => self.created(window, desktop_files::new_folder(&dir)),
            Command::NewTextFile => self.created(window, desktop_files::new_text_file(&dir)),
            Command::Paste => desktop_files::paste(dir),
            Command::Terminal => desktop_files::terminal(&self.terminal, &dir),
        }
        Ok(())
    }

    fn close_menu(&mut self, d: &Display) -> Result<()> {
        if self.menu.take().is_some() {
            self.menu_popup.hide(d)?;
        }
        Ok(())
    }

    // ---- Переименование ----

    fn rename_event(&mut self, d: &Display, ev: Event) -> Result<()> {
        // Пока открыто поле, окна всё равно перерисовываются.
        if let Event::Expose { window } = ev
            && let Some(w) = self.find(window)
        {
            w.dirty = true;
            return Ok(());
        }
        let Some((window, rename)) = &mut self.rename else {
            return Ok(());
        };
        let window = *window;
        let popup = self.rename_popup.id();
        let result = match ev {
            Event::Key(k) => rename.key(k),
            // Щелчок мимо поля — сохранить, как в Finder.
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
                        if let Some(w) = self.find(window) {
                            w.view.select_later(target);
                        }
                    }
                    Err(e) => desktop_files::error(&e),
                }
            }
        }
        Ok(())
    }

    fn close_rename(&mut self, d: &Display) -> Result<()> {
        if let Some((window, _)) = self.rename.take() {
            self.rename_popup.hide(d)?;
            if let Some(w) = self.find(window) {
                w.view.set_renaming(None);
                w.dirty = true;
            }
        }
        Ok(())
    }

    /// Поле переименования: открыть (когда файл появился в окне) или
    /// перерисовать на месте подписи.
    fn flush_rename(&mut self, d: &Display, fonts: &mut Fonts) -> Result<()> {
        if let Some((window, path)) = self.rename_start.take() {
            let Some(w) = self.windows.iter_mut().find(|w| w.win.id() == window) else {
                return Ok(());
            };
            match w.view.items().iter().position(|i| i.path == path) {
                Some(i) => {
                    let is_dir = w.view.items()[i].kind == mimshel_desktop::files::Kind::Folder;
                    w.view.set_renaming(Some(path.clone()));
                    w.dirty = true;
                    let rename = RenameBox::new(path, is_dir);
                    let (x, y) = rename_pos(d, w, i, rename.size(fonts).0);
                    let (bw, bh) = rename.size(fonts);
                    self.rename = Some((window, rename));
                    self.rename_popup.show(d, x, y, bw as u16, bh as u16)?;
                    self.rename_dirty = true;
                }
                // Ещё не появился в списке — подождём.
                None => self.rename_start = Some((window, path)),
            }
        }
        if std::mem::take(&mut self.rename_dirty)
            && let Some((window, rename)) = &self.rename
        {
            let (bw, bh) = rename.size(fonts);
            if let Some(w) = self.windows.iter().find(|w| w.win.id() == *window)
                && let Some(i) = w.view.items().iter().position(|i| i.path == rename.path)
            {
                let (x, y) = rename_pos(d, w, i, bw);
                self.rename_popup
                    .set_geometry(d, x, y, bw as u16, bh as u16)?;
            }
            self.rename_popup.present(d, &rename.render(fonts))?;
        }
        Ok(())
    }

    /// Закрыть меню и поле (например, перед открытием «Поиска»).
    pub fn close_popups(&mut self, d: &Display) -> Result<()> {
        self.close_menu(d)?;
        self.close_rename(d)
    }

    pub fn flush(&mut self, d: &Display, fonts: &mut Fonts, icons: &mut Icons) -> Result<()> {
        if let Some((window, path)) = self.open_phone.take()
            && let Some(w) = self.find(window)
            && w.view.navigate(path) == Action::Load
        {
            self.act(d, fonts, window, Action::Load)?;
        }
        if std::mem::take(&mut self.menu_dirty)
            && let Some((_, menu)) = &self.menu
        {
            self.menu_popup.present(d, &menu.render(fonts))?;
        }
        self.flush_rename(d, fonts)?;
        if std::mem::take(&mut self.drag_dirty)
            && let Some(drag) = &self.drag
            && let Some(w) = self.windows.iter().find(|w| w.win.id() == drag.window)
            && let (Some(pm), Some(grab)) = (w.view.drag_image(fonts, icons), w.view.grab())
        {
            let x = drag.origin.0 + drag.at.0 - grab.0;
            let y = drag.origin.1 + drag.at.1 - grab.1;
            self.drag_popup.show_passive(
                d,
                x.round() as i16,
                y.round() as i16,
                pm.width() as u16,
                pm.height() as u16,
            )?;
            self.drag_popup.present(d, &pm)?;
        }
        for w in &mut self.windows {
            if std::mem::take(&mut w.dirty) {
                let t = std::time::Instant::now();
                let frame = w.view.render(fonts, icons);
                let drawn = t.elapsed();
                w.win.present(d, &frame)?;
                if mimshel_core::names::flag("TIMING") {
                    eprintln!(
                        "Окно папки: кадр {drawn:?}, вывод {:?}, от открытия {:?}",
                        t.elapsed() - drawn,
                        w.opened.map(|o| o.elapsed())
                    );
                }
                if w.view.loaded()
                    && let Some(t) = w.opened.take()
                {
                    eprintln!(
                        "Окно папки «{}»: {} файлов, показано за {:?}",
                        w.view.title(),
                        w.view.items().len(),
                        t.elapsed()
                    );
                }
            }
        }
        Ok(())
    }
}

/// Нарисовать окно папки `dir` в PNG (проверка вида без экрана).
pub fn snapshot(dir: &Path, file: &str) -> Result<()> {
    let config = crate::ui::load_config();
    let mut fonts = Fonts::new(&config.font);
    let mut icons = Icons::new(&config.icon_theme);
    let mut view = FolderView::new(
        dir.to_path_buf(),
        mimshel_folder::places_lang(config.language.resolve()),
        DEFAULT_SIZE,
    );
    view.set_language(config.language);
    let locale = mimshel_apps::Locale::from_env();
    view.set_items(scan(dir, &MimeDb::load(), locale.as_ref()));
    let start = std::time::Instant::now();
    view.render(&mut fonts, &mut icons);
    eprintln!("Рисование: первый раз {:?}", start.elapsed());
    let start = std::time::Instant::now();
    let frame = view.render(&mut fonts, &mut icons);
    eprintln!("Рисование: второй раз {:?}", start.elapsed());
    frame.save_png(file)?;
    Ok(())
}

/// Открыть папку прежним Проводником (Thunar): свой выключен или служба не
/// запущена.
pub fn open_elsewhere(path: &Path) {
    desktop_files::spawn(&["thunar", &path.display().to_string()], None);
}

/// Папки из командной строки (`mimshel folder %U` в ярлыке): пути и адреса
/// `file://`; у файла — его папка. Ничего не дали — домашняя.
pub fn folder_args(args: &[String]) -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let cwd = std::env::current_dir().unwrap_or_else(|_| home.clone());
    let mut out: Vec<PathBuf> = args
        .iter()
        .filter_map(|a| {
            let path = match a.strip_prefix("file://") {
                Some(rest) => PathBuf::from(mimshel_x11::percent_decode(rest)),
                None if a.contains("://") => return None,
                None => cwd.join(a),
            };
            let path = path.canonicalize().unwrap_or(path);
            if path.is_dir() {
                Some(path)
            } else {
                path.parent().map(Path::to_path_buf)
            }
        })
        .collect();
    if out.is_empty() {
        out.push(home);
    }
    out
}

/// Где поле переименования (в точках экрана): по середине подписи.
fn rename_pos(d: &Display, w: &FolderWindow, i: usize, width: u32) -> (i16, i16) {
    let (ox, oy) = w.win.origin(d).unwrap_or((0, 0));
    let (lx, ly, lw, _) = w.view.label_rect(i);
    (
        ox + (lx + (lw - width as f32) / 2.0).round() as i16,
        oy + (ly - 3.0).round() as i16,
    )
}

fn trash(paths: &[PathBuf]) {
    for path in paths {
        desktop_files::spawn(&["gio", "trash", &path.display().to_string()], None);
    }
}

/// Пары «откуда → куда» для переноса в папку `into`: имена не повторяются
/// (занято — «имя 2»), саму папку в себя не кладём, перенос туда, где файл
/// уже лежит, — пропускаем.
fn pairs_into(
    paths: impl Iterator<Item = PathBuf>,
    into: &Path,
    moving: bool,
) -> Vec<(PathBuf, PathBuf)> {
    let mut reserved = HashSet::new();
    let mut pairs = Vec::new();
    for src in paths {
        let Some(name) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if src == into || into.starts_with(&src) {
            continue;
        }
        if moving && src.parent() == Some(into) {
            continue;
        }
        let dest = mimshel_desktop::files::free_name_reserved(into, &name, &reserved);
        reserved.insert(dest.clone());
        pairs.push((src, dest));
    }
    pairs
}

/// Свободное место на диске папки (`df`: в стандартной библиотеке Rust
/// такого нет, а `statvfs` напрямую — небезопасный код).
fn free_space(dir: &Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .args(["-B1", "--output=avail", "--"])
        .arg(dir)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .nth(1)?
        .trim()
        .parse()
        .ok()
}

/// Извлечь диск (`gio mount -e`), потом перечитать места в боковой панели.
/// A phone (`remote`) cannot be ejected, only unmounted (`gio mount -u`).
fn eject(path: PathBuf, remote: Option<String>, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let mut gio = std::process::Command::new("gio");
        match &remote {
            Some(uri) => gio.args(["mount", "-u", "--", uri]),
            None => gio.args(["mount", "-e", "--"]).arg(&path),
        };
        let ok = gio.status().is_ok_and(|s| s.success());
        if !ok {
            desktop_files::error(&format!("Не удалось извлечь «{}».", path.display()));
        }
        let _ = tx.send(Msg::Folder(Box::new(FolderMsg::Places)));
    });
}

/// Mount a phone or camera (`gio mount`), then open it in `window`.
fn mount(window: u32, uri: String, name: String, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let ok = std::process::Command::new("gio")
            .args(["mount", "--", &uri])
            .stdin(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            desktop_files::error(&format!(
                "Не удалось открыть «{name}». Разблокируйте телефон и выберите \
                 на нём «Передача файлов»."
            ));
        }
        let list = mimshel_folder::phones();
        let open = ok.then_some((window, uri));
        let _ = tx.send(Msg::Folder(Box::new(FolderMsg::Phones { list, open })));
    });
}

/// Rescan phones and cameras in a background thread (`gio` is slow).
fn scan_phones(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let list = mimshel_folder::phones();
        let _ = tx.send(Msg::Folder(Box::new(FolderMsg::Phones {
            list,
            open: None,
        })));
    });
}

/// What the sidebar «Devices» depends on, cheap to read: mounted disks, gvfs
/// mounts (phones) and plugged USB devices (a phone before it is mounted).
fn devices_state() -> String {
    let mut state = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let mut names = |dir: &Path| {
        let mut list: Vec<String> = std::fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        list.sort();
        for n in list {
            state.push_str(&n);
            state.push('\n');
        }
    };
    names(&mimshel_folder::gvfs_dir());
    if let Ok(buses) = std::fs::read_dir("/dev/bus/usb") {
        for bus in buses.flatten() {
            names(&bus.path());
        }
    }
    state
}

/// Следить за папкой окна: сразу и после каждой перемены присылать список
/// файлов. Поток заканчивается, когда окно закрыли или ушли из папки.
fn watch(
    window: u32,
    dir: PathBuf,
    hidden: bool,
    mime: Arc<MimeDb>,
    stop: Arc<AtomicBool>,
    tx: Sender<Msg>,
) {
    std::thread::spawn(move || {
        let locale = mimshel_apps::Locale::from_env();
        let send = |tx: &Sender<Msg>| {
            let items = scan_with(&dir, &mime, locale.as_ref(), hidden);
            let msg = FolderMsg::Files {
                window,
                dir: dir.clone(),
                items,
                free: free_space(&dir),
            };
            tx.send(Msg::Folder(Box::new(msg))).is_ok()
        };
        if !send(&tx) {
            return;
        }
        let Ok(mut inotify) = Inotify::init() else {
            return;
        };
        let mask = WatchMask::CREATE
            | WatchMask::DELETE
            | WatchMask::MOVED_FROM
            | WatchMask::MOVED_TO
            | WatchMask::CLOSE_WRITE
            | WatchMask::ATTRIB;
        if inotify.watches().add(&dir, mask).is_err() {
            return;
        }
        let mut buffer = vec![0; 4096];
        let mut changed = |inotify: &mut Inotify| {
            inotify
                .read_events(&mut buffer)
                .is_ok_and(|mut events| events.next().is_some())
        };
        // Подключённые диски: раз в ~2 с сверить — вставили флешку, и она
        // появится в боковой панели.
        let mounts = devices_state;
        let mut known = mounts();
        let mut polls = 0u32;
        while !stop.load(Ordering::Relaxed) {
            if !changed(&mut inotify) {
                std::thread::sleep(POLL);
                polls += 1;
                if polls.is_multiple_of(8) {
                    let now = mounts();
                    if now != known {
                        known = now;
                        let _ = tx.send(Msg::Folder(Box::new(FolderMsg::Places)));
                    }
                }
                continue;
            }
            // Ждём тишины: пока идут события (копируется пачка файлов),
            // перечитывать рано.
            loop {
                std::thread::sleep(QUIET);
                if !changed(&mut inotify) {
                    break;
                }
            }
            if stop.load(Ordering::Relaxed) || !send(&tx) {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_skip_self_and_same_folder() {
        let into = Path::new("/нет/папка");
        let pairs = pairs_into(
            [
                PathBuf::from("/нет/папка"),
                PathBuf::from("/нет/папка/уже.txt"),
                PathBuf::from("/нет/другое.txt"),
                PathBuf::from("/нет"),
            ]
            .into_iter(),
            into,
            true,
        );
        assert_eq!(
            pairs,
            [(
                PathBuf::from("/нет/другое.txt"),
                PathBuf::from("/нет/папка/другое.txt")
            )]
        );
    }
}
