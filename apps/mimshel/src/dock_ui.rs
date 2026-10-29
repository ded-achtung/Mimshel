//! Док на экране: окно-док во всю ширину у нижнего края, окна программ
//! (список, активное окно, иконки), клики, меню значка, перетаскивание,
//! миниатюры окон, «умное» скрытие и анимация.

use inotify::{Inotify, WatchMask};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use mimshel_apps::App;
use mimshel_core::config::{Config, DockHide, set_dgpu_apps, set_dock_apps};
use mimshel_dock::{
    Action, Dock, Look, Menu, MenuCommand, MenuResult, POOF, Preview, PreviewCard, Rect, THUMB_H,
    THUMB_W, poof_image, should_hide, thumbnail,
};
use mimshel_icons::Icons;
use mimshel_text::Fonts;
use mimshel_x11::{Display, DockWindow, Event, Popup, Property};

use crate::ui::{Msg, Result};

/// Шаг анимации (~60 кадров в секунду).
const FRAME: Duration = Duration::from_millis(16);
/// Насколько сдвинуть мышь с нажатой кнопкой, чтобы клик стал перетаскиванием.
const DRAG_START: f32 = 6.0;
/// Сколько держать мышь на значке, чтобы появились миниатюры окон.
const PREVIEW_DELAY: Duration = Duration::from_millis(500);
/// Мышь ушла и с дока, и с миниатюр — через сколько их закрыть (успеть
/// довести мышь от значка до миниатюр).
const PREVIEW_CLOSE: Duration = Duration::from_millis(300);

pub struct DockUi {
    window: DockWindow,
    dock: Dock,
    hide: DockHide,
    /// Окна программ, за которыми уже следим (свёрнуто, перемещение…).
    watched: HashSet<u32>,
    /// What each window in the client list looked like when last read
    /// (`None`: not shown in the dock). Only new windows and the window
    /// whose title, state or class changed are read again: re-reading all
    /// of them on every title change of a browser tab or a terminal meant
    /// ~5 X round trips and a /proc read per window on the main thread.
    known: HashMap<u32, Option<(mimshel_core::windows::WindowInfo, Option<String>)>>,
    /// Идёт ли таймер анимации.
    ticking: Arc<AtomicBool>,
    /// Нужно перерисовать (рисуем в [`DockUi::flush`], один раз на пачку событий).
    dirty: bool,
    /// The dock waits for a new window to draw before sliding away (see
    /// `update_hide`); look again at this time if it never draws.
    recheck_at: Option<Instant>,
    /// Меню значка (правый клик) и его окно; `menu_dirty` — перерисовать меню.
    menu: Option<Menu>,
    menu_popup: Popup,
    menu_dirty: bool,
    /// Левая кнопка нажата на значке: какой и где (координаты окна дока).
    /// Отпустили на месте — клик, повели — перетаскивание.
    press: Option<(usize, f32, f32)>,
    /// Окно со значком, который едет за мышью, и где мышь (координаты окна дока).
    drag_popup: Popup,
    drag_at: (f32, f32),
    drag_dirty: bool,
    /// Миниатюры окон над значком и их окно.
    preview: Option<Preview>,
    preview_popup: Popup,
    preview_dirty: bool,
    /// Значок под указателем и с какого момента.
    hover: Option<(String, Instant)>,
    /// После клика по значку миниатюры не открываются, пока мышь не уйдёт с него.
    preview_blocked: bool,
    over_preview: bool,
    /// Когда мышь ушла с дока или миниатюр.
    left_at: Option<Instant>,
    /// «Облачко» на месте убранного значка: где (координаты окна дока) и с какого момента.
    poof: Option<(f32, f32, Instant)>,
    /// Столбцы окна дока, где что-то нарисовано (после прошлого кадра).
    painted: Option<(u32, u32)>,
    tx: Sender<Msg>,
    terminal: Vec<String>,
    /// The primary monitor (`Display::primary_area`), in points: the dock
    /// sits at its bottom edge. The dock's own coordinates (bar, icons) are
    /// relative to its window, which spans this monitor's width.
    area: (i32, i32, u32, u32),
}

impl DockUi {
    pub fn new(d: &Display, config: &Config, apps: Vec<App>, tx: Sender<Msg>) -> Result<DockUi> {
        let area = d.primary_area();
        let look = Look {
            icon: config.dock_icon,
            zoom: config.dock_zoom.map_or(1.0, |p| p as f32 / 100.0),
            hide: config.dock_hide,
        };
        let mut ui = DockUi {
            window: d.create_dock("dock")?,
            dock: Dock::new(look, apps, config.dock_apps.clone(), area.2),
            hide: config.dock_hide,
            watched: HashSet::new(),
            known: HashMap::new(),
            ticking: Arc::new(AtomicBool::new(false)),
            dirty: true,
            recheck_at: None,
            menu: None,
            menu_popup: d.create_popup("dock-menu")?,
            menu_dirty: false,
            press: None,
            drag_popup: d.create_popup("dock-drag")?,
            drag_at: (0.0, 0.0),
            drag_dirty: false,
            preview: None,
            preview_popup: d.create_popup("dock-preview")?,
            preview_dirty: false,
            hover: None,
            preview_blocked: false,
            over_preview: false,
            left_at: None,
            poof: None,
            painted: None,
            tx,
            terminal: crate::ui::terminal_command(config),
            area,
        };
        if mimshel_apps::gpu::offload_available() {
            ui.dock.set_dgpu(Some((
                config.dgpu_apps.clone(),
                config.no_dgpu_apps.clone(),
            )));
        }
        ui.dock.set_language(config.language);
        ui.dock.set_trash(trash_full());
        watch_trash(ui.tx.clone());
        d.watch_root()?;
        d.watch_new_windows()?;
        ui.place(d)?;
        ui.window.show(d)?;
        ui.refresh_windows(d, None);
        Ok(ui)
    }

    /// Иконки значков, которые сейчас в доке, — заранее, в самом большом
    /// размере. Остальные нарисуются, когда программа появится в доке.
    pub fn preload(&self, icons: &mut Icons) {
        for item in self.dock.items() {
            if let Some(name) = item.app.as_ref().and_then(|a| a.icon.as_deref()) {
                icons.get(name, self.dock.max_icon());
            }
        }
    }

    /// Hot reload (v2): switch language without restarting the daemon.
    /// The dock is redrawn on the next flush.
    pub fn set_language(&mut self, lang: mimshel_core::i18n::Language) {
        self.dock.set_language(lang);
        self.dirty = true;
    }

    pub fn set_apps(&mut self, d: &Display, apps: Vec<App>) {
        self.dock.set_apps(apps);
        self.refresh_windows(d, None);
        self.dirty = true;
    }

    /// Корзина стала полной или пустой — другая иконка (и пункт «Очистить»).
    pub fn set_trash(&mut self, d: &Display, full: bool) -> Result<()> {
        self.dock.set_trash(full);
        self.place(d)?;
        self.dirty = true;
        Ok(())
    }

    /// Программа запущена (из «Поиска» или «Программ») — пусть значок прыгает.
    pub fn launched(&mut self, app: &App) {
        self.dock.launched(&app.id, Instant::now());
        self.start_ticking();
    }

    /// Это событие для дока?
    pub fn wants(&self, d: &Display, ev: &Event) -> bool {
        let own = self.window.id();
        match *ev {
            Event::Button { window, .. }
            | Event::Release { window, .. }
            | Event::Motion { window, .. }
            | Event::Enter { window, .. }
            | Event::Leave { window }
            | Event::Expose { window } => {
                window == own || window == self.menu_popup.id() || window == self.preview_popup.id()
            }
            // Клавиатура у дока, только пока открыто меню.
            Event::Key(_) => self.menu.is_some(),
            Event::Property { window, .. } | Event::Configure { window } => {
                window == d.root() || self.watched.contains(&window)
            }
            _ => false,
        }
    }

    /// Обработать событие. Рисует потом [`DockUi::flush`].
    pub fn event(&mut self, d: &Display, fonts: &mut Fonts, ev: Event) -> Result<()> {
        let now = Instant::now();
        if mimshel_core::names::flag("DEBUG_DOCK") {
            eprintln!("док: {ev:?}");
        }
        // Пока открыто меню, мышь и клавиатура — его; окна программ и экран
        // отслеживаем как обычно.
        if self.menu.is_some() && !matches!(ev, Event::Property { .. } | Event::Configure { .. }) {
            return self.menu_event(d, fonts, ev);
        }
        if let Event::Button { window, .. }
        | Event::Release { window, .. }
        | Event::Motion { window, .. }
        | Event::Enter { window, .. }
        | Event::Leave { window }
        | Event::Expose { window } = ev
        {
            if window == self.preview_popup.id() {
                return self.preview_event(d, ev);
            }
            // Запоздалые события уже закрытого меню.
            if window == self.menu_popup.id() {
                return Ok(());
            }
        }
        match ev {
            // Кнопка нажата на значке — может, это начало перетаскивания.
            Event::Motion { x, y, .. } if self.press.is_some() => {
                self.press_motion(d, f32::from(x), f32::from(y));
            }
            Event::Motion { x, y, .. } | Event::Enter { x, y, .. } => {
                self.dock.set_pointer(Some(f32::from(x)));
                self.update_hide(d);
                let key = self
                    .dock
                    .item_at(f32::from(x), f32::from(y))
                    .map(|i| self.dock.items()[i].key.clone());
                self.set_hover(key, now);
            }
            // Пока кнопка нажата, мышь может уйти с дока — это не «ушла».
            Event::Leave { .. } if self.press.is_some() => return Ok(()),
            Event::Leave { .. } => {
                self.dock.set_pointer(None);
                self.update_hide(d);
                self.set_hover(None, now);
            }
            Event::Button { x, y, button, .. } => {
                let (x, y) = (f32::from(x), f32::from(y));
                // Любой клик по доку убирает миниатюры.
                self.close_preview(d)?;
                self.preview_blocked = true;
                let Some(i) = self.dock.item_at(x, y) else {
                    return Ok(());
                };
                match button {
                    // Левая — решим при отпускании: клик или перетаскивание.
                    1 => self.press = Some((i, x, y)),
                    3 => self.open_menu(d, fonts, i)?,
                    _ => {
                        let action = self.dock.click(i, button, now);
                        self.perform(d, action)?;
                    }
                }
            }
            Event::Release {
                x, y, button: 1, ..
            } => {
                let (x, y) = (f32::from(x), f32::from(y));
                let press = self.press.take();
                if self.dock.dragging() {
                    self.dock.drag_to(x, y);
                    // Убрали из дока — на месте значка «облачко» (в том же окне).
                    if self.dock.drag_removes() {
                        self.poof = Some((x, y, now));
                        self.start_ticking();
                    } else {
                        self.drag_popup.hide_passive(d)?;
                    }
                    let action = self.dock.drop_drag();
                    self.perform(d, action)?;
                    self.sync_pointer(d);
                } else if let Some((i, ..)) = press
                    && self.dock.item_at(x, y) == Some(i)
                {
                    let action = self.dock.click(i, 1, now);
                    self.perform(d, action)?;
                }
            }
            Event::Expose { .. } => {}
            // A new window drew for the first time: it may slide the dock away now.
            Event::Drawn { .. } => self.update_hide(d),
            Event::Configure { window } if window == d.root() => {
                self.area = d.primary_area();
                self.dock.set_screen_width(self.area.2);
                self.place(d)?;
                self.update_hide(d);
            }
            // Активное окно сдвинулось — может, теперь оно заходит на док.
            Event::Configure { window } => {
                if Some(window) == self.dock.active() {
                    self.update_hide(d);
                } else {
                    return Ok(());
                }
            }
            Event::Property { window, what } => match what {
                Property::ClientList => {
                    // Окна поменялись — миниатюры устарели (откроются заново).
                    self.close_preview(d)?;
                    self.refresh_windows(d, None)
                }
                Property::State | Property::Class | Property::Title => {
                    self.refresh_windows(d, Some(window))
                }
                Property::ActiveWindow => {
                    self.dock.set_active(d.active_window());
                    self.update_hide(d);
                }
                _ => return Ok(()),
            },
            _ => return Ok(()),
        }
        self.dirty = true;
        Ok(())
    }

    /// Значок под указателем сменился (`None` — мышь ушла с дока).
    fn set_hover(&mut self, key: Option<String>, now: Instant) {
        if self.hover.as_ref().map(|(k, _)| k) == key.as_ref() {
            return;
        }
        self.preview_blocked = false;
        match key {
            Some(key) => {
                self.hover = Some((key, now));
                self.left_at = None;
                self.wake(PREVIEW_CLOSE);
                self.wake(PREVIEW_DELAY);
            }
            None => {
                self.hover = None;
                self.left_at = Some(now);
                self.wake(PREVIEW_CLOSE);
            }
        }
    }

    /// События окна миниатюр.
    fn preview_event(&mut self, d: &Display, ev: Event) -> Result<()> {
        let Some(preview) = &mut self.preview else {
            return Ok(());
        };
        match ev {
            Event::Enter { x, y, .. } | Event::Motion { x, y, .. } => {
                self.over_preview = true;
                self.left_at = None;
                if preview.motion(f32::from(x), f32::from(y)) {
                    self.preview_dirty = true;
                }
            }
            Event::Leave { .. } => {
                self.over_preview = false;
                preview.hover = None;
                self.preview_dirty = true;
                self.left_at = Some(Instant::now());
                self.wake(PREVIEW_CLOSE);
            }
            Event::Button {
                x, y, button: 1, ..
            } => {
                if let Some(i) = preview.card_at(f32::from(x), f32::from(y)) {
                    let w = preview.cards[i].window;
                    self.close_preview(d)?;
                    self.preview_blocked = true;
                    d.activate(w)?;
                }
            }
            Event::Expose { .. } => self.preview_dirty = true,
            _ => {}
        }
        Ok(())
    }

    /// Открыть, сменить или закрыть миниатюры — по тому, где мышь и сколько
    /// она там.
    fn update_preview(&mut self, d: &Display, icons: &mut Icons) -> Result<()> {
        let now = Instant::now();
        let current = self.preview.as_ref().map(|p| p.key.clone());
        let busy = self.menu.is_some() || self.press.is_some() || self.dock.dragging();
        let running = |key: &str| {
            self.dock
                .items()
                .iter()
                .any(|i| i.key == key && !i.windows.is_empty())
        };
        let want = if busy {
            None
        } else if self.over_preview {
            current.clone()
        } else if let Some((key, since)) = &self.hover {
            // Миниатюры уже открыты — переходят на другой значок сразу.
            let waited = current.is_some() || now.duration_since(*since) >= PREVIEW_DELAY;
            if running(key) && waited && !self.preview_blocked {
                Some(key.clone())
            } else if now.duration_since(*since) < PREVIEW_CLOSE {
                // Мышь задела соседний значок по пути к миниатюрам — не сразу.
                current.clone()
            } else {
                None
            }
        } else if self
            .left_at
            .is_some_and(|t| now.duration_since(t) < PREVIEW_CLOSE)
        {
            current.clone()
        } else {
            None
        };
        if want == current {
            return Ok(());
        }
        if mimshel_core::names::flag("DEBUG_DOCK") {
            eprintln!("док: миниатюры {current:?} → {want:?}");
        }
        self.close_preview(d)?;
        if let Some(key) = want {
            self.open_preview(d, icons, &key)?;
        }
        Ok(())
    }

    fn open_preview(&mut self, d: &Display, icons: &mut Icons, key: &str) -> Result<()> {
        let Some(index) = self.dock.items().iter().position(|i| i.key == key) else {
            return Ok(());
        };
        let item = &self.dock.items()[index];
        let mut windows = item.windows.clone();
        windows.sort_unstable();
        windows.truncate(Preview::fits(self.area.2));
        let app_icon = item
            .app
            .as_ref()
            .and_then(|a| a.icon.as_deref())
            .and_then(|n| icons.get(n, 64))
            .cloned();
        let cards = windows
            .iter()
            .map(|&w| PreviewCard {
                window: w,
                title: self
                    .dock
                    .window(w)
                    .map(|i| i.title.clone())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| item.name.clone()),
                // Свёрнутое окно не нарисовано — вместо снимка иконка.
                image: d
                    .capture_window(w)
                    // Миниатюра — в пикселях экрана (при масштабе крупнее).
                    .map(|p| thumbnail(&p, d.px(THUMB_W), d.px(THUMB_H)))
                    .or_else(|| app_icon.clone())
                    .or_else(|| d.window_icon(w, 64)),
            })
            .collect();
        let preview = Preview {
            key: key.to_string(),
            cards,
            hover: None,
        };
        let Some((cx, icon_top)) = self.dock.anchor(index, self.dock.hidden_at(Instant::now()))
        else {
            return Ok(());
        };
        let (w, h) = preview.size();
        let (x, y) = self.above_icon(cx, icon_top, (w, h));
        self.preview_popup
            .show_passive(d, x as i16, y as i16, w as u16, h as u16)?;
        self.preview = Some(preview);
        self.preview_dirty = true;
        self.dock.set_popup_open(true);
        self.dirty = true;
        Ok(())
    }

    fn close_preview(&mut self, d: &Display) -> Result<()> {
        if self.preview.take().is_none() {
            return Ok(());
        }
        self.preview_popup.hide_passive(d)?;
        self.over_preview = false;
        self.dock.set_popup_open(self.menu.is_some());
        self.update_hide(d);
        self.dirty = true;
        Ok(())
    }

    /// Разбудить службу через `delay` (проверить, не пора ли открыть или
    /// закрыть миниатюры).
    fn wake(&self, delay: Duration) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay + Duration::from_millis(5));
            let _ = tx.send(Msg::Tick);
        });
    }

    /// Мышь движется с нажатой на значке кнопкой.
    fn press_motion(&mut self, d: &Display, x: f32, y: f32) {
        let Some((i, px, py)) = self.press else {
            return;
        };
        if !self.dock.dragging() {
            if (x - px).abs() + (y - py).abs() < DRAG_START {
                self.dock.set_pointer(Some(x));
                return;
            }
            self.dock.start_drag(i, x, y);
        }
        self.dock.drag_to(x, y);
        // Утащили прочь — увеличение под мышью ни к чему.
        self.dock.set_pointer((!self.dock.drag_out()).then_some(x));
        self.update_hide(d);
        self.drag_at = (x, y);
        self.drag_dirty = true;
    }

    /// События, пока открыто меню значка.
    fn menu_event(&mut self, d: &Display, fonts: &mut Fonts, ev: Event) -> Result<()> {
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
            // Правый клик по другому значку — его меню, как на Mac.
            Event::Button {
                x, y, button: 3, ..
            } => {
                let next = self.dock.item_at(f32::from(x), f32::from(y));
                self.close_menu(d)?;
                if let Some(i) = next {
                    self.open_menu(d, fonts, i)?;
                }
                return Ok(());
            }
            // Любой другой клик мимо меню (в том числе по доку) — закрыть.
            Event::Button { .. } => MenuResult::Close,
            Event::Key(k) => menu.key(k.key),
            Event::Expose { window } if window == popup => MenuResult::Redraw,
            Event::Expose { .. } => {
                self.dirty = true;
                MenuResult::Nothing
            }
            // Движение по доку не трогаем: значки «замирают», пока открыто меню.
            _ => MenuResult::Nothing,
        };
        match result {
            MenuResult::Nothing => {}
            MenuResult::Redraw => self.menu_dirty = true,
            MenuResult::Close => self.close_menu(d)?,
            MenuResult::Run(command) => {
                let key = menu.key.clone();
                self.close_menu(d)?;
                self.run_command(d, &key, command)?;
            }
        }
        Ok(())
    }

    /// Открыть меню значка `index` над ним.
    fn open_menu(&mut self, d: &Display, fonts: &mut Fonts, index: usize) -> Result<()> {
        let now = Instant::now();
        self.close_preview(d)?;
        let (Some(mut menu), Some((cx, icon_top))) = (
            self.dock.menu(index),
            self.dock.anchor(index, self.dock.hidden_at(now)),
        ) else {
            return Ok(());
        };
        let (w, h) = menu.measure(fonts);
        let (x, y) = self.above_icon(cx, icon_top, (w, h));
        self.menu_popup
            .show(d, x as i16, y as i16, w as u16, h as u16)?;
        self.menu = Some(menu);
        self.menu_dirty = true;
        self.dock.set_popup_open(true);
        self.dirty = true;
        Ok(())
    }

    /// Закрыть меню и миниатюры (если открыты): например, перед открытием «Поиска».
    pub fn close_popups(&mut self, d: &Display) -> Result<()> {
        self.close_preview(d)?;
        self.preview_blocked = true;
        self.close_menu(d)
    }

    /// Закрыть меню (если открыто).
    fn close_menu(&mut self, d: &Display) -> Result<()> {
        if self.menu.take().is_none() {
            return Ok(());
        }
        self.menu_popup.hide(d)?;
        self.dock.set_popup_open(false);
        self.sync_pointer(d);
        self.dirty = true;
        Ok(())
    }

    /// Док какое-то время не следил за мышью (меню, перетаскивание) — где она теперь?
    fn sync_pointer(&mut self, d: &Display) {
        let (ax, ay, _, ah) = self.area;
        let bar = self.dock.bar_rect(ah);
        let over = d
            .pointer_position()
            .map(|(x, y)| (i32::from(x) - ax, i32::from(y) - ay))
            .filter(|&(x, y)| y >= bar.y && x >= bar.x && x < bar.x + bar.w as i32);
        self.dock.set_pointer(over.map(|(x, _)| x as f32));
        self.update_hide(d);
    }

    fn run_command(&mut self, d: &Display, key: &str, command: MenuCommand) -> Result<()> {
        let action = self.dock.command(key, command, Instant::now());
        self.perform(d, action)
    }

    fn perform(&mut self, d: &Display, action: Action) -> Result<()> {
        match action {
            Action::Nothing => {}
            Action::Launch(app) => {
                if let Err(e) = crate::ui::launch_app(&app, &self.terminal) {
                    eprintln!("Не удалось запустить «{}»: {e}", app.name);
                }
                self.start_ticking();
            }
            Action::Activate(w) => d.activate(w)?,
            Action::Minimize(ws) => {
                for w in ws {
                    d.minimize(w)?;
                }
            }
            Action::Close(ws) => {
                for w in ws {
                    d.close(w)?;
                }
            }
            Action::OpenTrash => trash_call("DisplayTrash"),
            Action::EmptyTrash => trash_call("EmptyTrash"),
            Action::ToggleDgpu { app_id, prefers } => {
                let config = crate::ui::load_config();
                let (yes, no) = config.toggled_dgpu(&app_id, prefers);
                if let Some(path) = crate::ui::config_path()
                    && let Err(e) = save_dgpu(&path, &yes, &no)
                {
                    eprintln!(
                        "Не удалось сохранить выбор видеокарты в {}: {e}",
                        path.display()
                    );
                }
                self.dock.set_dgpu(Some((yes, no)));
            }
            Action::Pinned(list) => {
                // Полоса стала шире или уже — обновить резерв места и скрытие.
                self.place(d)?;
                self.update_hide(d);
                if let Some(path) = crate::ui::config_path()
                    && let Err(e) = save_pinned(&path, &list)
                {
                    eprintln!("Не удалось сохранить значки дока в {}: {e}", path.display());
                }
            }
        }
        Ok(())
    }

    /// Кадр анимации. Сам кадр рисует [`DockUi::flush`]; если анимация
    /// закончилась, это будет последний кадр — в конечном положении.
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.dock.settle(now);
        if !self.dock.animating(now) && self.poof.is_none() {
            self.ticking.store(false, Ordering::SeqCst);
        }
        self.dirty = true;
    }

    /// Перерисовать, если что-то изменилось.
    pub fn flush(&mut self, d: &Display, fonts: &mut Fonts, icons: &mut Icons) -> Result<()> {
        if self.recheck_at.is_some_and(|t| Instant::now() >= t) {
            self.recheck_at = None;
            self.update_hide(d);
        }
        self.update_preview(d, icons)?;
        if self.preview_dirty {
            self.preview_dirty = false;
            if let Some(preview) = &self.preview {
                self.preview_popup.present(d, &preview.render(fonts))?;
            }
        }
        if self.dirty {
            // Not while a new window that has not drawn covers the dock: it
            // shows the dock as it was when it appeared (see `frozen`).
            match self.frozen(d) {
                Some(left) => self.wake_in(left),
                None => {
                    self.dirty = false;
                    self.redraw(d, fonts, icons)?;
                }
            }
        }
        if self.drag_dirty {
            self.drag_dirty = false;
            if let Some((image, (cx, cy))) = self.dock.drag_image(fonts, icons) {
                let (left, top) = self.window_origin();
                let (x, y) = self.drag_at;
                self.drag_popup.show_passive(
                    d,
                    (left + x - cx).round() as i16,
                    (top + y - cy).round() as i16,
                    image.width() as u16,
                    image.height() as u16,
                )?;
                self.drag_popup.present(d, &image)?;
            }
        }
        if let Some((x, y, start)) = self.poof {
            let t = start.elapsed().as_secs_f32() / POOF.as_secs_f32();
            if t >= 1.0 {
                self.poof = None;
                self.drag_popup.hide_passive(d)?;
            } else {
                let size = (self.dock.max_icon() as f32 * 1.4) as u32;
                let image = poof_image(size, t);
                let (left, top) = self.window_origin();
                let half = size as f32 / 2.0;
                self.drag_popup.show_passive(
                    d,
                    (left + x - half).round() as i16,
                    (top + y - half).round() as i16,
                    size as u16,
                    size as u16,
                )?;
                self.drag_popup.present(d, &image)?;
            }
        }
        if self.menu_dirty {
            self.menu_dirty = false;
            if let Some(menu) = &self.menu {
                self.menu_popup.present(d, &menu.render(fonts))?;
            }
        }
        Ok(())
    }

    /// Look at hiding and drawing again after `left` (a new window that
    /// never draws).
    fn wake_in(&mut self, left: Duration) {
        let at = Instant::now() + left;
        if self.recheck_at.is_some_and(|t| t <= at) {
            return;
        }
        self.recheck_at = Some(at);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(left);
            let _ = tx.send(Msg::Tick);
        });
    }

    fn start_ticking(&self) {
        if self.ticking.swap(true, Ordering::SeqCst) {
            return;
        }
        let ticking = self.ticking.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            while ticking.load(Ordering::SeqCst) {
                std::thread::sleep(FRAME);
                if tx.send(Msg::Tick).is_err() {
                    break;
                }
            }
        });
    }

    /// Перечитать окна программ.
    /// Re-read the window list; `changed` — a window whose title, state or
    /// class changed and must be read again (new windows always are).
    fn refresh_windows(&mut self, d: &Display, changed: Option<u32>) {
        // Своё окно дока тоже в списке (оно управляемое) — пропускаем: подписка
        // на изменения заменила бы его собственный набор событий, и док
        // перестал бы получать движение указателя.
        let own = self.window.id();
        let list: Vec<u32> = d.client_list().into_iter().filter(|&w| w != own).collect();
        self.watched.retain(|w| list.contains(w));
        self.known.retain(|w, _| list.contains(w));
        let mut infos = Vec::new();
        for &w in &list {
            if self.watched.insert(w) {
                let _ = d.watch_window(w);
            }
            if changed == Some(w) || !self.known.contains_key(&w) {
                let read = d.window_info(w).map(|info| {
                    let hint = info
                        .pid
                        .and_then(mimshel_apps::process::launched_from)
                        .map(|p| p.to_string_lossy().into_owned());
                    (info, hint)
                });
                self.known.insert(w, read);
            }
            if let Some(Some(entry)) = self.known.get(&w) {
                infos.push(entry.clone());
            }
        }
        self.dock.set_windows(infos);
        for w in self.dock.windows_without_icon() {
            if let Some(icon) = d.window_icon(w, self.dock.max_icon()) {
                self.dock.set_window_icon(w, icon);
            }
        }
        self.dock.set_active(d.active_window());
        self.update_hide(d);
    }

    /// Прятать или показывать — по режиму, активному окну и указателю.
    fn update_hide(&mut self, d: &Display) {
        // The bar on the screen, to compare with the active window's frame.
        let (ax, ay, _, ah) = self.area;
        let bar = self.dock.bar_rect(ah);
        let bar = Rect {
            x: bar.x + ax,
            y: bar.y + ay,
            ..bar
        };
        let active = self.dock.active().and_then(|w| {
            let info = self.dock.window(w)?;
            if info.minimized {
                return None;
            }
            let (x, y, ww, hh) = d.frame_geometry(w)?;
            Some((Rect { x, y, w: ww, h: hh }, info.maximized))
        });
        // Пока тащат значок или открыто меню или миниатюры, док не прячется.
        let busy = self.dock.pointer().is_some()
            || self.dock.dragging()
            || self.menu.is_some()
            || self.preview.is_some();
        let hide = should_hide(self.hide, bar, active, busy);
        // Sliding away under a window that has not drawn yet left frozen
        // copies of the dock in it: wait for its first drawing.
        if hide && let Some(left) = self.frozen(d) {
            self.wake_in(left);
            return;
        }
        let now = Instant::now();
        self.dock.slide_to(hide, now);
        if self.dock.animating(now) {
            self.start_ticking();
        }
    }

    /// A new window that has not drawn yet covers the dock: how much longer
    /// to keep the dock exactly as it is. Until its first drawing (Chrome:
    /// up to a second) the window shows a frozen copy of the screen from when
    /// it appeared; whatever of the dock changes under it — an icon bouncing,
    /// the dock sliding away — stays visible there as a ghost. `Event::Drawn`
    /// (or the time limit, if it never draws) lets the dock go on.
    fn frozen(&self, d: &Display) -> Option<Duration> {
        let (left, top) = self.window_origin();
        let scale = d.scale();
        d.undrawn_over((
            (left * scale).round() as i32,
            (top * scale).round() as i32,
            d.px(self.area.2),
            d.px(self.dock.window_height()),
        ))
    }

    /// The dock window's top-left corner on the screen.
    fn window_origin(&self) -> (f32, f32) {
        let (ax, ay, _, ah) = self.area;
        (
            ax as f32,
            (ay + ah as i32) as f32 - self.dock.window_height() as f32,
        )
    }

    /// Where a popup of size `(w, h)` goes above the icon centred at `cx`
    /// (window coordinates) whose top is `icon_top`: kept on the monitor.
    fn above_icon(&self, cx: f32, icon_top: f32, (w, h): (u32, u32)) -> (f32, f32) {
        let (_, ay, aw, _) = self.area;
        let (left, top) = self.window_origin();
        let x = (cx - w as f32 / 2.0).clamp(4.0, (aw as f32 - w as f32 - 4.0).max(4.0));
        let y = (top + icon_top - h as f32 - 10.0).max(ay as f32);
        (left + x, y)
    }

    /// Окно дока у нижнего края; при «никогда не прятать» — резерв места,
    /// чтобы развёрнутые окна заканчивались над доком.
    fn place(&self, d: &Display) -> Result<()> {
        let (ax, ay, aw, ah) = self.area;
        let h = self.dock.window_height();
        self.window
            .set_geometry(d, ax, ay + ah as i32 - h as i32, aw, h)?;
        let bar = self.dock.bar_rect(ah);
        if self.hide == DockHide::Never {
            // The bottom strut counts from the bottom of the whole screen:
            // a shorter monitor next to a taller one reserves more.
            let (_, screen_h) = d.screen_size();
            let reserve = (i32::from(screen_h) - (ay + bar.y)).max(0) as u32;
            let x0 = (ax + bar.x).max(0) as u32;
            self.window.set_strut(d, reserve, x0, x0 + bar.w)?;
        } else {
            self.window.set_strut(d, 0, 0, 0)?;
        }
        Ok(())
    }

    fn redraw(&mut self, d: &Display, fonts: &mut Fonts, icons: &mut Icons) -> Result<()> {
        let now = Instant::now();
        let frame = self.dock.render(fonts, icons, now);
        let t1 = Instant::now();
        // Окно во всю ширину экрана, а нарисовано обычно меньше: отправляем
        // только занятые столбцы — и те, что были заняты прошлым кадром
        // (их нужно стереть).
        let painted = painted_columns(frame.pixmap());
        let send = match (painted, self.painted) {
            (Some((a, b)), Some((c, e))) => Some((a.min(c), b.max(e))),
            (one, other) => one.or(other),
        };
        if let Some((x0, x1)) = send {
            self.window.present_columns(d, &frame, x0, x1)?;
        }
        self.painted = painted;
        let t2 = Instant::now();
        self.window
            .set_input(d, &self.dock.input_rects(self.dock.hidden_at(now)))?;
        if mimshel_core::names::flag("TIMING") {
            d.sync()?;
            eprintln!(
                "кадр {}x{}: рисование {:?}, вывод {:?}, до конца {:?}",
                frame.width(),
                frame.height(),
                t1 - now,
                t2 - t1,
                now.elapsed()
            );
        }
        Ok(())
    }
}

/// Папка с файлами корзины (стандарт freedesktop: `$XDG_DATA_HOME/Trash/files`).
fn trash_dir() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("Trash/files"))
}

fn trash_full() -> bool {
    trash_dir()
        .and_then(|d| std::fs::read_dir(d).ok())
        .is_some_and(|mut entries| entries.next().is_some())
}

/// Состояние корзины меняется по событиям inotify без опроса каждые
/// две секунды. Если каталог временно отсутствует, редко проверяем его
/// появление и восстанавливаем наблюдение.
fn watch_trash(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let mut full = trash_full();
        let mut watcher: Option<Inotify> = None;
        let mut buffer = [0u8; 4096];
        loop {
            if watcher.is_none() {
                watcher = trash_dir().and_then(|dir| {
                    let io = Inotify::init().ok()?;
                    io.watches()
                        .add(
                            &dir,
                            WatchMask::CREATE
                                | WatchMask::DELETE
                                | WatchMask::MOVED_FROM
                                | WatchMask::MOVED_TO
                                | WatchMask::DELETE_SELF
                                | WatchMask::MOVE_SELF,
                        )
                        .ok()?;
                    Some(io)
                });
            }
            if let Some(io) = watcher.as_mut() {
                match io.read_events_blocking(&mut buffer) {
                    Ok(events) => {
                        let directory_gone = events.into_iter().any(|ev| {
                            ev.mask.intersects(
                                inotify::EventMask::DELETE_SELF
                                    | inotify::EventMask::MOVE_SELF
                                    | inotify::EventMask::IGNORED,
                            )
                        });
                        if directory_gone {
                            watcher = None;
                        }
                    }
                    Err(_) => watcher = None,
                }
            } else {
                // На новых системах ~/.local/share/Trash/files может ещё
                // отсутствовать. Избегаем двух пробуждений CPU в секунду.
                std::thread::sleep(Duration::from_secs(15));
            }
            let now = trash_full();
            if now != full {
                full = now;
                if tx.send(Msg::Trash(full)).is_err() {
                    return;
                }
            }
        }
    });
}

/// Открыть или очистить корзину через Проводник (Thunar, D-Bus
/// `org.xfce.Trash`). Очищая, он сам спрашивает подтверждение, поэтому без
/// Проводника корзина не очищается. Открыть можно и без него (`gio open`).
/// В отдельном потоке: Проводник может отвечать долго.
fn trash_call(method: &'static str) {
    std::thread::spawn(move || {
        let display = std::env::var("DISPLAY").unwrap_or_default();
        let ok = Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.xfce.FileManager",
                "/org/xfce/FileManager",
                &format!("org.xfce.Trash.{method}"),
                &format!("string:{display}"),
                "string:",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return;
        }
        if method == "DisplayTrash" {
            let _ = Command::new("gio")
                .args(["open", "trash:///"])
                .stdin(Stdio::null())
                .status();
        } else {
            eprintln!("Проводник не ответил — корзину не очищаю (нужно его подтверждение)");
        }
    });
}

/// Столбцы `x0..x1`, в которых есть хоть одна непрозрачная точка.
fn painted_columns(pm: &tiny_skia::Pixmap) -> Option<(u32, u32)> {
    let w = pm.width() as usize;
    let (mut x0, mut x1) = (w, 0);
    for row in pm.data().chunks(w * 4) {
        let px = row.as_chunks::<4>().0;
        if let Some(first) = px[..x0].iter().position(|p| p[3] != 0) {
            x0 = first;
        }
        if let Some(last) = px[x1..].iter().rposition(|p| p[3] != 0) {
            x1 = x1 + last + 1;
        }
    }
    (x0 < x1).then_some((x0 as u32, x1 as u32))
}

/// Записать списки `[apps] dgpu` и `no_dgpu`; остальное в файле не меняется.
fn save_dgpu(path: &Path, yes: &[String], no: &[String]) -> std::io::Result<()> {
    crate::ui::config_store(path.to_path_buf()).edit(|text| {
        set_dgpu_apps(text, yes, no).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "в файле ошибка, исправьте её — тогда выбор сохранится",
            )
        })
    })
}

/// Записать закреплённые значки в `[dock] apps` файла настроек; остальное
/// в файле не меняется.
fn save_pinned(path: &Path, apps: &[String]) -> std::io::Result<()> {
    crate::ui::config_store(path.to_path_buf()).edit(|text| {
        set_dock_apps(text, apps).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "в файле ошибка, исправьте её — тогда значки сохранятся",
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::painted_columns;
    use tiny_skia::{Color, Pixmap, Rect, Transform};

    #[test]
    fn painted_columns_finds_content() {
        let mut pm = Pixmap::new(100, 20).unwrap();
        assert_eq!(painted_columns(&pm), None);
        let mut paint = tiny_skia::Paint::default();
        paint.set_color(Color::from_rgba8(255, 255, 255, 255));
        let rect = |x, y, w| Rect::from_xywh(x, y, w, 2.0).unwrap();
        pm.fill_rect(rect(30.0, 15.0, 10.0), &paint, Transform::identity(), None);
        pm.fill_rect(rect(55.0, 2.0, 5.0), &paint, Transform::identity(), None);
        assert_eq!(painted_columns(&pm), Some((30, 60)));
    }
}
