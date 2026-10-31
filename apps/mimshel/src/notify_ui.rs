//! Уведомления на экране: служба `org.freedesktop.Notifications` (вместо
//! xfce4-notifyd), баннеры справа сверху под строкой меню, щелчки и время
//! исчезновения; центр уведомлений (щелчок по колокольчику) — история.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use mimshel_apps::App;
use mimshel_core::config::dock_entry_matches;
use mimshel_dbus::notify::{self, NotifyEvent, NotifyServer};
use mimshel_icons::Icons;
use mimshel_notify::{Banner, Banners, Center, CenterHit, CloseReason, Hit, Notification, WIDTH};
use mimshel_text::Fonts;
use tiny_skia::{IntSize, Pixmap};

use crate::platform::{Display, Event, Key, Popup, Result};
use crate::ui::Msg;

/// Сколько уведомлений помнить в истории.
const HISTORY: usize = 50;
/// Отступ баннеров от правого края и от строки меню.
const MARGIN: u32 = 10;
/// Столько снизу оставить под док, открывая центр уведомлений.
const DOCK_ROOM: u32 = 90;

pub struct NotifyUi {
    server: NotifyServer,
    banners: Banners,
    popup: Popup,
    shown: bool,
    dirty: bool,
    /// Прошедшие уведомления (с иконками и временем прихода), новые — первыми.
    history: Vec<Banner>,
    /// Центр уведомлений и его окно.
    center: Option<Center>,
    center_popup: Popup,
    /// Перерисовать центр; `center_stale` — история поменялась, собрать заново.
    center_dirty: bool,
    center_stale: bool,
    apps: Vec<App>,
    dnd: bool,
    top: u32,
    tx: Sender<Msg>,
    language: mimshel_core::i18n::Language,
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
            center: None,
            center_popup: d.create_popup("notification-center")?,
            center_dirty: false,
            center_stale: false,
            apps,
            dnd,
            top,
            tx,
            language: mimshel_core::i18n::Language::Auto,
        })
    }

    pub fn set_language(&mut self, lang: mimshel_core::i18n::Language) {
        self.language = lang;
        self.banners.language = lang;
        if let Some(center) = &mut self.center {
            center.language = lang;
            center.list.language = lang;
        }
    }

    pub fn set_apps(&mut self, apps: Vec<App>) {
        self.apps = apps;
    }

    /// «Не беспокоить»: новые уведомления — сразу в историю, без баннеров.
    pub fn set_dnd(&mut self, on: bool) {
        self.dnd = on;
    }

    pub fn center_open(&self) -> bool {
        self.center.is_some()
    }

    pub fn wants(&self, ev: &Event) -> bool {
        match *ev {
            Event::Button { window, .. }
            | Event::Motion { window, .. }
            | Event::Enter { window, .. }
            | Event::Leave { window }
            | Event::Expose { window } => {
                window == self.popup.id() || window == self.center_popup.id()
            }
            Event::Key(_) => self.center.is_some(),
            _ => false,
        }
    }

    pub fn notify_event(&mut self, icons: &mut Icons, e: NotifyEvent) {
        match e {
            NotifyEvent::Show(n) => {
                let icon = self.icon(&n, icons);
                if self.dnd && n.urgency < 2 {
                    self.remember(Banner {
                        n,
                        icon,
                        shown_at: Instant::now(),
                    });
                    self.refresh_center();
                    return;
                }
                for gone in self.banners.show(n, icon, Instant::now()) {
                    self.remember(gone);
                }
                self.dirty = true;
                self.wake();
            }
            NotifyEvent::Close(id) => {
                if let Some(b) = self.banners.remove(id) {
                    self.remember(b);
                    self.dirty = true;
                }
                self.server.closed(id, CloseReason::Closed);
            }
        }
        self.refresh_center();
    }

    pub fn event(&mut self, d: &Display, fonts: &mut Fonts, ev: Event) -> Result<()> {
        let center = self.center_popup.id();
        match ev {
            // Центр уведомлений.
            Event::Motion { window, x, y } | Event::Enter { window, x, y } if window == center => {
                if let Some(c) = &mut self.center
                    && c.motion(fonts, f32::from(x), f32::from(y))
                {
                    self.center_dirty = true;
                }
            }
            Event::Button { window, x, y, .. } if window == center => {
                let hit = match &self.center {
                    Some(c) => c.hit(fonts, f32::from(x), f32::from(y)),
                    None => return Ok(()),
                };
                let (w, h) = self.center.as_ref().map_or((0, 0), |c| c.size(fonts));
                let inside = x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h;
                match hit {
                    Some(CenterHit::Clear) => self.history.clear(),
                    Some(CenterHit::Remove(id)) => self.history.retain(|b| b.n.id != id),
                    Some(CenterHit::Action(id, key)) => {
                        self.server.action(id, &key);
                        self.history.retain(|b| b.n.id != id);
                    }
                    None if inside => return Ok(()),
                    // Щелчок мимо — закрыть, как на Mac.
                    None => return self.close_center(d),
                }
                self.center_stale = true;
            }
            Event::Key(k) => {
                if k.key == Key::Escape {
                    self.close_center(d)?;
                }
            }
            Event::Expose { window } if window == center => self.center_dirty = true,
            Event::Leave { window } if window == center => {}
            // Баннеры.
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
                    return Ok(());
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
                if let Some(b) = self.banners.remove(id) {
                    self.remember(b);
                }
                self.server.closed(id, CloseReason::Dismissed);
                self.dirty = true;
                self.refresh_center();
            }
            Event::Expose { .. } => self.dirty = true,
            _ => {}
        }
        Ok(())
    }

    /// Время вышло — убрать баннеры.
    pub fn tick(&mut self) {
        let gone = self.banners.expire(Instant::now());
        if gone.is_empty() {
            return;
        }
        for b in gone {
            self.server.closed(b.n.id, CloseReason::Expired);
            self.remember(b);
        }
        self.dirty = true;
        self.refresh_center();
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

    fn remember(&mut self, b: Banner) {
        if b.n.transient {
            return;
        }
        self.history.retain(|h| h.n.id != b.n.id);
        self.history.push(b);
        // По времени прихода, а не ухода с экрана: новые — первыми.
        self.history
            .sort_by(|a, b| b.shown_at.cmp(&a.shown_at).then(b.n.id.cmp(&a.n.id)));
        self.history.truncate(HISTORY);
    }

    /// Окно уведомлений (баннеры или центр).
    pub fn owns(&self, window: u32) -> bool {
        window == self.popup.id() || window == self.center_popup.id()
    }

    /// Top-right corner for banners and the centre: on the primary
    /// monitor, under the menu bar.
    fn corner(&self, d: &Display) -> (i16, i16) {
        let (ax, ay, aw, _) = d.primary_area();
        (
            (ax + aw as i32 - WIDTH as i32 - MARGIN as i32) as i16,
            (ay + (self.top + MARGIN) as i32) as i16,
        )
    }

    /// Открыть центр уведомлений или закрыть, если открыт.
    pub fn toggle_center(&mut self, d: &Display, fonts: &mut Fonts) -> Result<()> {
        if self.center.is_some() {
            return self.close_center(d);
        }
        self.rebuild_center(fonts, d);
        let Some(c) = &self.center else {
            return Ok(());
        };
        let (w, h) = c.size(fonts);
        let (x, y) = self.corner(d);
        self.center_popup.show(d, x, y, w as u16, h as u16)?;
        self.center_dirty = true;
        Ok(())
    }

    /// Закрыть центр уведомлений (если открыт).
    pub fn close_center(&mut self, d: &Display) -> Result<()> {
        if self.center.take().is_some() {
            self.center_popup.hide(d)?;
        }
        Ok(())
    }

    /// История поменялась — открытый центр собрать заново (при выводе:
    /// для раскладки нужны шрифты).
    fn refresh_center(&mut self) {
        if self.center.is_some() {
            self.center_stale = true;
        }
    }

    /// Собрать центр заново по истории (размер окна — по содержимому).
    fn rebuild_center(&mut self, fonts: &mut Fonts, d: &Display) {
        let (_, _, _, area_h) = d.primary_area();
        let max_h = area_h.saturating_sub(self.top + 2 * MARGIN + DOCK_ROOM) as f32;
        let mut center = Center::new(fonts, self.history.clone(), max_h);
        center.language = self.language;
        self.center = Some(center);
        self.center_dirty = true;
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
            n.desktop_entry
                .as_ref()
                .and_then(|e| by_app(&|a: &App| dock_entry_matches(e, &a.id))),
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

    /// Показать, сдвинуть или спрятать окна баннеров и центра.
    pub fn flush(&mut self, d: &Display, fonts: &mut Fonts) -> Result<()> {
        if self.center_stale && self.center.is_some() {
            // Карточек стало меньше или больше — собрать заново, подогнать окно.
            self.center_stale = false;
            self.rebuild_center(fonts, d);
            if let Some(c) = &self.center {
                let (w, h) = c.size(fonts);
                let (x, y) = self.corner(d);
                self.center_popup
                    .set_geometry(d, x, y, w as u16, h as u16)?;
            }
        }
        if self.center_dirty {
            self.center_dirty = false;
            if let Some(c) = &self.center {
                self.center_popup.present(d, &c.render(fonts))?;
            }
        }
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
        let (x, y) = self.corner(d);
        self.popup.show_passive(d, x, y, w as u16, h as u16)?;
        self.shown = true;
        self.popup.present(d, &self.banners.render(fonts))?;
        Ok(())
    }
}
