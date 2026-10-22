//! Док на экране: окно-док во всю ширину у нижнего края, окна программ
//! (список, активное окно, иконки), клики, «умное» скрытие и анимация.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use shell_apps::App;
use shell_core::config::{Config, DockHide};
use shell_dock::{Action, Dock, Look, Rect, should_hide};
use shell_icons::Icons;
use shell_text::Fonts;
use shell_x11::{Display, DockWindow, Event, Property};

use crate::ui::{Msg, Result};

/// Шаг анимации (~60 кадров в секунду).
const FRAME: Duration = Duration::from_millis(16);

pub struct DockUi {
    window: DockWindow,
    dock: Dock,
    hide: DockHide,
    /// Окна программ, за которыми уже следим (свёрнуто, перемещение…).
    watched: HashSet<u32>,
    /// Идёт ли таймер анимации.
    ticking: Arc<AtomicBool>,
    /// Нужно перерисовать (рисуем в [`DockUi::flush`], один раз на пачку событий).
    dirty: bool,
    tx: Sender<Msg>,
    terminal: Vec<String>,
    screen: (u32, u32),
}

impl DockUi {
    pub fn new(d: &Display, config: &Config, apps: Vec<App>, tx: Sender<Msg>) -> Result<DockUi> {
        let (sw, sh) = d.screen_size();
        let look = Look {
            icon: config.dock_icon,
            zoom: config.dock_zoom.map_or(1.0, |p| p as f32 / 100.0),
            hide: config.dock_hide,
        };
        let mut ui = DockUi {
            window: d.create_dock("dock")?,
            dock: Dock::new(look, apps, config.dock_apps.clone(), u32::from(sw)),
            hide: config.dock_hide,
            watched: HashSet::new(),
            ticking: Arc::new(AtomicBool::new(false)),
            dirty: true,
            tx,
            terminal: config.terminal.clone(),
            screen: (u32::from(sw), u32::from(sh)),
        };
        d.watch_root()?;
        ui.place(d)?;
        ui.window.show(d)?;
        ui.refresh_windows(d);
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

    pub fn set_apps(&mut self, d: &Display, apps: Vec<App>) {
        self.dock.set_apps(apps);
        self.refresh_windows(d);
        self.dirty = true;
    }

    /// Программа запущена (из Spotlight или Launchpad) — пусть значок прыгает.
    pub fn launched(&mut self, app: &App) {
        self.dock.launched(&app.id, Instant::now());
        self.start_ticking();
    }

    /// Это событие для дока?
    pub fn wants(&self, d: &Display, ev: &Event) -> bool {
        let own = self.window.id();
        match *ev {
            Event::Button { window, .. }
            | Event::Motion { window, .. }
            | Event::Enter { window, .. }
            | Event::Leave { window }
            | Event::Expose { window } => window == own,
            Event::Property { window, .. } | Event::Configure { window } => {
                window == d.root() || self.watched.contains(&window)
            }
            _ => false,
        }
    }

    /// Обработать событие. Рисует потом [`DockUi::flush`].
    pub fn event(&mut self, d: &Display, ev: Event) -> Result<()> {
        let now = Instant::now();
        if std::env::var_os("SHELL_DEBUG_DOCK").is_some() {
            eprintln!("док: {ev:?}");
        }
        match ev {
            Event::Motion { x, .. } | Event::Enter { x, .. } => {
                self.dock.set_pointer(Some(f32::from(x)));
                self.update_hide(d);
            }
            Event::Leave { .. } => {
                self.dock.set_pointer(None);
                self.update_hide(d);
            }
            Event::Button { x, y, button, .. } => {
                let Some(i) = self.dock.item_at(f32::from(x), f32::from(y)) else {
                    return Ok(());
                };
                match self.dock.click(i, button, now) {
                    Action::Nothing => {}
                    Action::Launch(app) => {
                        if let Err(e) = app.launch(&self.terminal) {
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
                }
            }
            Event::Expose { .. } => {}
            Event::Configure { window } if window == d.root() => {
                let (sw, sh) = d.screen_size();
                self.screen = (u32::from(sw), u32::from(sh));
                self.dock.set_screen_width(u32::from(sw));
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
            Event::Property { what, .. } => match what {
                Property::ClientList | Property::State | Property::Class | Property::Title => {
                    self.refresh_windows(d)
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

    /// Кадр анимации. Сам кадр рисует [`DockUi::flush`]; если анимация
    /// закончилась, это будет последний кадр — в конечном положении.
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.dock.settle(now);
        if !self.dock.animating(now) {
            self.ticking.store(false, Ordering::SeqCst);
        }
        self.dirty = true;
    }

    /// Перерисовать, если что-то изменилось.
    pub fn flush(&mut self, d: &Display, fonts: &mut Fonts, icons: &mut Icons) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        self.redraw(d, fonts, icons)
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
    fn refresh_windows(&mut self, d: &Display) {
        // Своё окно дока тоже в списке (оно управляемое) — пропускаем: подписка
        // на изменения заменила бы его собственный набор событий, и док
        // перестал бы получать движение указателя.
        let own = self.window.id();
        let list: Vec<u32> = d.client_list().into_iter().filter(|&w| w != own).collect();
        self.watched.retain(|w| list.contains(w));
        let mut infos = Vec::new();
        for &w in &list {
            if self.watched.insert(w) {
                let _ = d.watch_window(w);
            }
            if let Some(info) = d.window_info(w) {
                let hint = info
                    .pid
                    .and_then(shell_apps::process::launched_from)
                    .map(|p| p.to_string_lossy().into_owned());
                infos.push((info, hint));
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
        let bar = self.dock.bar_rect(self.screen.1);
        let active = self.dock.active().and_then(|w| {
            let info = self.dock.window(w)?;
            if info.minimized {
                return None;
            }
            let (x, y, ww, hh) = d.frame_geometry(w)?;
            Some((Rect { x, y, w: ww, h: hh }, info.maximized))
        });
        let hide = should_hide(self.hide, bar, active, self.dock.pointer().is_some());
        let now = Instant::now();
        self.dock.slide_to(hide, now);
        if self.dock.animating(now) {
            self.start_ticking();
        }
    }

    /// Окно дока у нижнего края; при «никогда не прятать» — резерв места,
    /// чтобы развёрнутые окна заканчивались над доком.
    fn place(&self, d: &Display) -> Result<()> {
        let (sw, sh) = self.screen;
        let h = self.dock.window_height();
        self.window
            .set_geometry(d, 0, sh as i32 - h as i32, sw, h)?;
        let bar = self.dock.bar_rect(sh);
        if self.hide == DockHide::Never {
            let reserve = (sh as i32 - bar.y) as u32;
            self.window
                .set_strut(d, reserve, bar.x as u32, (bar.x + bar.w as i32) as u32)?;
        } else {
            self.window.set_strut(d, 0, 0, 0)?;
        }
        Ok(())
    }

    fn redraw(&mut self, d: &Display, fonts: &mut Fonts, icons: &mut Icons) -> Result<()> {
        let now = Instant::now();
        let frame = self.dock.render(fonts, icons, now);
        self.window.present(d, &frame)?;
        self.window
            .set_input(d, &self.dock.input_rects(self.dock.hidden_at(now)))?;
        Ok(())
    }
}
