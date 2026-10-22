//! Уведомления на экране: служба `org.freedesktop.Notifications` (вместо
//! xfce4-notifyd), баннеры справа сверху под строкой меню, щелчки и время
//! исчезновения. История — для центра уведомлений.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use shell_apps::App;
use shell_dbus::notify::{self, NotifyEvent, NotifyServer};
use shell_icons::Icons;
use shell_notify::{Banners, CloseReason, Hit, Notification, WIDTH};
use shell_text::Fonts;
use shell_x11::{Display, Event, Popup};
use tiny_skia::{IntSize, Pixmap};

use crate::ui::{Msg, Result};

/// Сколько уведомлений помнить в истории.
const HISTORY: usize = 50;
/// Отступ баннеров от правого края и от строки меню.
const MARGIN: u32 = 10;

pub struct NotifyUi {
    server: NotifyServer,
    banners: Banners,
    popup: Popup,
    shown: bool,
    dirty: bool,
    /// Прошедшие уведомления, новые — первыми.
    pub history: Vec<Notification>,
    apps: Vec<App>,
    dnd: bool,
    top: u32,
    tx: Sender<Msg>,
}

impl NotifyUi {
    pub fn new(
        d: &Display,
        apps: Vec<App>,
        top: u32,
        dnd: bool,
        tx: Sender<Msg>,
    ) -> Result<NotifyUi> {
        let events = tx.clone();
        let server = notify::start(move |e| {
            let _ = events.send(Msg::Notify(e));
        })?;
        Ok(NotifyUi {
            server,
            banners: Banners::default(),
            popup: d.create_popup("notifications")?,
            shown: false,
            dirty: false,
            history: Vec::new(),
            apps,
            dnd,
            top,
            tx,
        })
    }

    pub fn set_apps(&mut self, apps: Vec<App>) {
        self.apps = apps;
    }

    /// «Не беспокоить»: новые уведомления — сразу в историю, без баннеров.
    pub fn set_dnd(&mut self, on: bool) {
        self.dnd = on;
    }

    pub fn wants(&self, ev: &Event) -> bool {
        match *ev {
            Event::Button { window, .. }
            | Event::Motion { window, .. }
            | Event::Enter { window, .. }
            | Event::Leave { window }
            | Event::Expose { window } => window == self.popup.id(),
            _ => false,
        }
    }

    pub fn notify_event(&mut self, icons: &mut Icons, e: NotifyEvent) {
        match e {
            NotifyEvent::Show(n) => {
                if self.dnd && n.urgency < 2 {
                    self.remember(n);
                    return;
                }
                let icon = self.icon(&n, icons);
                for gone in self.banners.show(n, icon, Instant::now()) {
                    self.remember(gone);
                }
                self.dirty = true;
                self.wake();
            }
            NotifyEvent::Close(id) => {
                if let Some(n) = self.banners.remove(id) {
                    self.remember(n);
                    self.dirty = true;
                }
                self.server.closed(id, CloseReason::Closed);
            }
        }
    }

    pub fn event(&mut self, fonts: &mut Fonts, ev: Event) {
        match ev {
            Event::Motion { x, y, .. } | Event::Enter { x, y, .. } => {
                if self.banners.motion(fonts, f32::from(x), f32::from(y)) {
                    self.dirty = true;
                }
            }
            Event::Leave { .. } => {
                if self.banners.leave() {
                    self.dirty = true;
                }
                // Пока смотрели, время могло выйти.
                self.wake();
            }
            Event::Button { x, y, button, .. } => {
                let Some(hit) = self.banners.hit(fonts, f32::from(x), f32::from(y)) else {
                    return;
                };
                let id = match hit {
                    // Правая кнопка — просто убрать, как на Mac.
                    Hit::Body(id) | Hit::Action(id, _) | Hit::Close(id) if button == 3 => id,
                    Hit::Body(id) => {
                        if let Some(b) = self.banners.items.iter().find(|b| b.n.id == id)
                            && b.n.actions.iter().any(|(k, _)| k == "default")
                        {
                            self.server.action(id, "default");
                        }
                        id
                    }
                    Hit::Action(id, key) => {
                        self.server.action(id, &key);
                        id
                    }
                    Hit::Close(id) => id,
                };
                if let Some(n) = self.banners.remove(id) {
                    self.remember(n);
                }
                self.server.closed(id, CloseReason::Dismissed);
                self.dirty = true;
            }
            Event::Expose { .. } => self.dirty = true,
            _ => {}
        }
    }

    /// Время вышло — убрать баннеры.
    pub fn tick(&mut self) {
        let gone = self.banners.expire(Instant::now());
        if gone.is_empty() {
            return;
        }
        for n in gone {
            self.server.closed(n.id, CloseReason::Expired);
            self.remember(n);
        }
        self.dirty = true;
        self.wake();
    }

    /// Разбудить службу, когда исчезнет ближайший баннер.
    fn wake(&self) {
        let Some(at) = self.banners.next_expiry() else {
            return;
        };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(
                at.saturating_duration_since(Instant::now()) + Duration::from_millis(20),
            );
            let _ = tx.send(Msg::Tick);
        });
    }

    fn remember(&mut self, n: Notification) {
        if n.transient {
            return;
        }
        self.history.retain(|h| h.id != n.id);
        self.history.insert(0, n);
        self.history.truncate(HISTORY);
    }

    /// Картинка уведомления, иначе иконка: присланная, ярлыка программы,
    /// программы с таким названием; иначе — «информация».
    fn icon(&self, n: &Notification, icons: &mut Icons) -> Option<Pixmap> {
        if let Some(img) = &n.image {
            let mut rgba = Vec::with_capacity(img.rgba.len());
            for px in img.rgba.as_chunks::<4>().0 {
                let [r, g, b, a] = *px;
                let m = |c: u8| (u16::from(c) * u16::from(a) / 255) as u8;
                rgba.extend_from_slice(&[m(r), m(g), m(b), a]);
            }
            if let Some(pm) =
                IntSize::from_wh(img.width, img.height).and_then(|s| Pixmap::from_vec(rgba, s))
            {
                return Some(pm);
            }
        }
        let by_app = |pred: &dyn Fn(&App) -> bool| {
            self.apps
                .iter()
                .find(|a| pred(a))
                .and_then(|a| a.icon.clone())
        };
        let candidates = [
            Some(n.app_icon.trim_start_matches("file://").to_string()).filter(|s| !s.is_empty()),
            n.desktop_entry.as_ref().and_then(|e| {
                let id = format!("{}.desktop", e.trim_end_matches(".desktop"));
                by_app(&|a: &App| a.id == id)
            }),
            by_app(&|a: &App| {
                a.name.eq_ignore_ascii_case(&n.app_name)
                    || a.name_untranslated.eq_ignore_ascii_case(&n.app_name)
            }),
            Some("dialog-information".to_string()),
        ];
        candidates
            .into_iter()
            .flatten()
            .find_map(|name| icons.get(&name, 64).cloned())
    }

    /// Показать, сдвинуть или спрятать окно баннеров.
    pub fn flush(&mut self, d: &Display, fonts: &mut Fonts) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        if self.banners.items.is_empty() {
            if self.shown {
                self.popup.hide_passive(d)?;
                self.shown = false;
            }
            return Ok(());
        }
        let (w, h) = self.banners.size(fonts);
        let (sw, _) = d.screen_size();
        let x = i32::from(sw) - WIDTH as i32 - MARGIN as i32;
        self.popup
            .show_passive(d, x as i16, (self.top + MARGIN) as i16, w as u16, h as u16)?;
        self.shown = true;
        self.popup.present(d, &self.banners.render(fonts))?;
        Ok(())
    }
}
