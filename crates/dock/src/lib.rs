//! Док, как в macOS: закреплённые программы, затем запущенные; точка под
//! запущенной; увеличение под мышью; подпись над значком; прыжок при запуске;
//! «умное» скрытие. Здесь нет X11: состояние, решения и рисование в картинку.

mod drag;
mod draw;
mod hide;
mod items;
mod layout;
mod menu;
mod preview;

pub use draw::{POOF, poof_image};
pub use hide::{Rect, should_hide};
pub use items::{Item, TRASH, match_window};
pub use layout::Slot;
pub use menu::{Menu, MenuCommand, MenuEntry, MenuResult};
pub use preview::{Preview, PreviewCard, THUMB_H, THUMB_W, thumbnail};

use std::time::{Duration, Instant};

use mimshel_apps::App;
use mimshel_core::config::{DockHide, dgpu_wanted, dock_entry, dock_entry_matches};
use mimshel_core::windows::WindowInfo;
use tiny_skia::Pixmap;

/// Сколько длится выезд/уезд дока (как HideTime в теме Plank).
pub const SLIDE: Duration = Duration::from_millis(200);
/// Один прыжок значка при запуске и сколько прыгать, пока не появится окно.
pub const BOUNCE: Duration = Duration::from_millis(600);
pub const BOUNCE_LIMIT: Duration = Duration::from_millis(3000);

/// Что сделать после клика.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Nothing,
    Launch(Box<App>),
    Activate(u32),
    Minimize(Vec<u32>),
    /// Закрыть окна («Завершить»).
    Close(Vec<u32>),
    /// Закреплённые значки поменялись — сохранить новый список в настройки.
    Pinned(Vec<String>),
    /// Переключили «на видеокарте NVIDIA» у программы (ярлык, просит ли он сам).
    ToggleDgpu {
        app_id: String,
        prefers: bool,
    },
    OpenTrash,
    /// Очистить корзину — только с подтверждением.
    EmptyTrash,
}

/// Настройки внешнего вида.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub icon: u32,
    /// Увеличение под мышью, во сколько раз (1.0 — нет).
    pub zoom: f32,
    pub hide: DockHide,
}

pub struct Dock {
    pub(crate) look: Look,
    apps: Vec<App>,
    pinned: Vec<String>,
    /// Есть ли видеокарта NVIDIA для запуска программ и списки `[apps] dgpu`,
    /// `no_dgpu` — для пункта меню «Запускать на видеокарте NVIDIA».
    dgpu: Option<(Vec<String>, Vec<String>)>,
    /// Окна программ (снизу вверх) и номер программы, к которой окно относится.
    windows: Vec<(WindowInfo, Option<usize>)>,
    /// Иконки окон без ярлыка.
    window_icons: Vec<(u32, Pixmap)>,
    active: Option<u32>,
    pub(crate) items: Vec<Item>,
    pub(crate) screen_w: f32,
    /// Где указатель (x в окне дока), пока он над доком.
    pub(crate) pointer: Option<f32>,
    /// Насколько спрятан: 0 — виден, 1 — совсем спрятан; и куда едет.
    hidden: f32,
    slide: Option<(f32, f32, Instant)>,
    /// Прыжки значков после запуска: ключ значка и начало.
    bounces: Vec<(String, Instant)>,
    /// Над доком открыто меню или миниатюры — подпись над значком не нужна.
    popup_open: bool,
    /// Перетаскиваемый значок.
    drag: Option<drag::Drag>,
    /// Корзина в конце дока: полна ли она (`None` — без корзины).
    trash: Option<bool>,
}

impl Dock {
    pub fn new(look: Look, apps: Vec<App>, pinned: Vec<String>, screen_w: u32) -> Dock {
        let mut d = Dock {
            look,
            apps,
            pinned,
            dgpu: None,
            windows: Vec::new(),
            window_icons: Vec::new(),
            active: None,
            items: Vec::new(),
            screen_w: screen_w as f32,
            pointer: None,
            hidden: 0.0,
            slide: None,
            bounces: Vec::new(),
            popup_open: false,
            drag: None,
            trash: None,
        };
        d.rebuild();
        d
    }

    pub fn set_apps(&mut self, apps: Vec<App>) {
        self.apps = apps;
        let infos: Vec<_> = self.windows.drain(..).map(|(w, _)| (w, None)).collect();
        self.set_windows(infos);
    }

    pub fn apps(&self) -> &[App] {
        &self.apps
    }

    pub fn set_screen_width(&mut self, w: u32) {
        self.screen_w = w as f32;
    }

    /// Новый список окон (снизу вверх) с подсказкой — ярлыком, из которого
    /// запущен процесс окна (если известен).
    pub fn set_windows(&mut self, windows: Vec<(WindowInfo, Option<String>)>) {
        self.windows = windows
            .into_iter()
            .map(|(w, hint)| {
                let app = match_window(&w, hint.as_deref(), &self.apps);
                (w, app)
            })
            .collect();
        let alive: Vec<u32> = self.windows.iter().map(|(w, _)| w.id).collect();
        self.window_icons.retain(|(id, _)| alive.contains(id));
        self.rebuild();
    }

    /// Окна без ярлыка, для которых нужна иконка самого окна.
    pub fn windows_without_icon(&self) -> Vec<u32> {
        self.windows
            .iter()
            .filter(|(w, app)| {
                app.is_none() && !self.window_icons.iter().any(|(id, _)| *id == w.id)
            })
            .map(|(w, _)| w.id)
            .collect()
    }

    pub fn set_window_icon(&mut self, window: u32, icon: Pixmap) {
        self.window_icons.push((window, icon));
    }

    pub(crate) fn window_icon(&self, window: u32) -> Option<&Pixmap> {
        self.window_icons
            .iter()
            .find(|(id, _)| *id == window)
            .map(|(_, p)| p)
    }

    pub fn set_active(&mut self, w: Option<u32>) {
        self.active = w;
    }

    pub fn active(&self) -> Option<u32> {
        self.active
    }

    /// Окно по номеру (для решения о скрытии).
    pub fn window(&self, id: u32) -> Option<&WindowInfo> {
        self.windows.iter().map(|(w, _)| w).find(|w| w.id == id)
    }

    fn rebuild(&mut self) {
        let dragged = self
            .drag
            .and_then(|d| self.items.get(d.index))
            .map(|i| i.key.clone());
        self.items = items::build(&self.apps, &self.pinned, &self.windows, self.trash);
        // Значки перестроились посреди перетаскивания — найти свой заново.
        if let Some(key) = dragged {
            match self.items.iter().position(|i| i.key == key) {
                Some(index) => {
                    if let Some(d) = &mut self.drag {
                        d.index = index;
                    }
                }
                None => self.drag = None,
            }
        }
        // Окно появилось — прыгать незачем.
        let items = &self.items;
        self.bounces
            .retain(|(key, _)| items.iter().any(|i| &i.key == key && i.windows.is_empty()));
    }

    /// Показать корзину в конце дока; `full` — есть ли в ней что-то.
    pub fn set_trash(&mut self, full: bool) {
        if self.trash != Some(full) {
            self.trash = Some(full);
            self.rebuild();
        }
    }

    /// Клик по значку `index` кнопкой `button`.
    pub fn click(&mut self, index: usize, button: u8, now: Instant) -> Action {
        let Some(item) = self.items.get(index) else {
            return Action::Nothing;
        };
        if item.key == TRASH {
            return match button {
                1 | 2 => Action::OpenTrash,
                _ => Action::Nothing,
            };
        }
        let launch = |item: &Item| match &item.app {
            Some(app) => Action::Launch(Box::new(app.clone())),
            None => Action::Nothing,
        };
        let action = match button {
            // Средняя кнопка — ещё одна копия программы.
            2 => launch(item),
            1 if item.windows.is_empty() => launch(item),
            1 => {
                let ours = |w: &u32| Some(*w) == self.active;
                if !item.windows.iter().any(ours) {
                    // Самое верхнее окно программы (список — снизу вверх).
                    Action::Activate(*item.windows.last().expect("не пусто"))
                } else if item.windows.len() > 1 {
                    // Несколько окон — следующее по кругу: самое нижнее из них.
                    Action::Activate(item.windows[0])
                } else {
                    // Как в Plank: клик по активной программе сворачивает её.
                    Action::Minimize(item.windows.clone())
                }
            }
            _ => Action::Nothing,
        };
        if let Action::Launch(_) = action {
            let key = item.key.clone();
            self.launched(&key, now);
        }
        action
    }

    /// Меню значка `index` (правый клик). Пусто — меню нет.
    pub fn menu(&self, index: usize) -> Option<Menu> {
        let item = self.items.get(index)?;
        if item.key == TRASH {
            let mut entries = vec![MenuEntry {
                label: "Открыть".into(),
                command: MenuCommand::OpenTrash,
                checked: false,
                separator_before: false,
                enabled: true,
                indent: false,
            }];
            if self.trash == Some(true) {
                entries.push(MenuEntry {
                    label: "Очистить Корзину…".into(),
                    command: MenuCommand::EmptyTrash,
                    checked: false,
                    separator_before: true,
                    enabled: true,
                    indent: false,
                });
            }
            return Some(Menu::new(item.key.clone(), entries));
        }
        let running = !item.windows.is_empty();
        // Сверху — окна программы в порядке появления, как на Mac.
        let mut windows = item.windows.clone();
        windows.sort_unstable();
        let mut entries: Vec<MenuEntry> = windows
            .iter()
            .filter_map(|&w| self.window(w))
            .map(|w| MenuEntry {
                label: if w.title.is_empty() {
                    item.name.clone()
                } else {
                    w.title.clone()
                },
                command: MenuCommand::Activate(w.id),
                checked: Some(w.id) == self.active,
                separator_before: false,
                enabled: true,
                indent: false,
            })
            .collect();
        // У окна без ярлыка нечего запускать и закреплять — только завершить.
        if item.app.is_some() {
            entries.push(MenuEntry {
                label: if running {
                    "Новое окно"
                } else {
                    "Открыть"
                }
                .into(),
                command: MenuCommand::Launch,
                checked: false,
                separator_before: running,
                enabled: true,
                indent: false,
            });
            entries.push(MenuEntry {
                label: "Оставить в Dock".into(),
                command: MenuCommand::TogglePin,
                checked: item.pinned,
                separator_before: true,
                enabled: true,
                indent: false,
            });
            if let (Some((yes, no)), Some(app)) = (&self.dgpu, &item.app) {
                entries.push(MenuEntry {
                    label: "Запускать на видеокарте NVIDIA".into(),
                    command: MenuCommand::ToggleDgpu,
                    checked: dgpu_wanted(yes, no, &app.id, app.prefers_dgpu),
                    separator_before: false,
                    enabled: true,
                    indent: false,
                });
            }
        }
        if running {
            entries.push(MenuEntry {
                label: "Завершить".into(),
                command: MenuCommand::Quit,
                checked: false,
                separator_before: true,
                enabled: true,
                indent: false,
            });
        }
        (!entries.is_empty()).then(|| Menu::new(item.key.clone(), entries))
    }

    /// Выполнить пункт меню значка `key`.
    pub fn command(&mut self, key: &str, command: MenuCommand, now: Instant) -> Action {
        let Some(item) = self.items.iter().find(|i| i.key == key) else {
            return Action::Nothing;
        };
        match command {
            MenuCommand::Launch => match &item.app {
                Some(app) => {
                    let app = app.clone();
                    self.launched(key, now);
                    Action::Launch(Box::new(app))
                }
                None => Action::Nothing,
            },
            MenuCommand::Quit => Action::Close(item.windows.clone()),
            MenuCommand::ToggleDgpu => match &item.app {
                Some(app) => Action::ToggleDgpu {
                    app_id: app.id.clone(),
                    prefers: app.prefers_dgpu,
                },
                None => Action::Nothing,
            },
            MenuCommand::Activate(w) => Action::Activate(w),
            MenuCommand::OpenTrash => Action::OpenTrash,
            MenuCommand::EmptyTrash => Action::EmptyTrash,
            MenuCommand::TogglePin => {
                if item.app.is_none() {
                    return Action::Nothing;
                }
                let same = |p: &String| dock_entry_matches(p, key);
                if self.pinned.iter().any(same) {
                    self.pinned.retain(|p| !same(p));
                } else {
                    self.pinned.push(dock_entry(key).to_string());
                }
                self.rebuild();
                Action::Pinned(self.pinned.clone())
            }
        }
    }

    /// Видеокарта NVIDIA для запуска программ есть (`Some` — списки `[apps]
    /// dgpu` и `no_dgpu`) или нет (`None` — пункта в меню нет).
    pub fn set_dgpu(&mut self, lists: Option<(Vec<String>, Vec<String>)>) {
        self.dgpu = lists;
    }

    /// Над доком открыто меню или миниатюры (тогда подпись над значком не рисуется).
    pub fn set_popup_open(&mut self, open: bool) {
        self.popup_open = open;
    }

    /// Программа запущена (из дока, «Поиска» или «Программ») — её значок
    /// прыгает, пока не появится окно. Значка нет — ничего.
    pub fn launched(&mut self, app_id: &str, now: Instant) {
        if self.items.iter().any(|i| i.key == app_id) {
            self.bounces.retain(|(k, _)| k != app_id);
            self.bounces.push((app_id.to_string(), now));
        }
    }

    /// Высота прыжка значка (доля размера иконки) в момент `now`.
    pub(crate) fn bounce(&self, key: &str, now: Instant) -> f32 {
        let Some((_, start)) = self.bounces.iter().find(|(k, _)| k == key) else {
            return 0.0;
        };
        let t = now.saturating_duration_since(*start);
        if t >= BOUNCE_LIMIT {
            return 0.0;
        }
        let phase = (t.as_secs_f32() % BOUNCE.as_secs_f32()) / BOUNCE.as_secs_f32();
        (phase * std::f32::consts::PI).sin() * 0.5
    }

    /// Указатель над доком (x в окне) или ушёл (`None`).
    pub fn set_pointer(&mut self, x: Option<f32>) {
        self.pointer = x;
    }

    pub fn pointer(&self) -> Option<f32> {
        self.pointer
    }

    /// Начать выезд (`false`) или уезд (`true`) дока, если он уже не там.
    pub fn slide_to(&mut self, hidden: bool, now: Instant) {
        let target = if hidden { 1.0 } else { 0.0 };
        let current = self.hidden_at(now);
        match self.slide {
            Some((_, to, _)) if to == target => {}
            _ if self.slide.is_none() && current == target => {}
            _ => self.slide = Some((current, target, now)),
        }
    }

    /// Насколько док спрятан в момент `now` (0..=1).
    pub fn hidden_at(&self, now: Instant) -> f32 {
        match self.slide {
            None => self.hidden,
            Some((from, to, start)) => {
                let t = (now.saturating_duration_since(start).as_secs_f32() / SLIDE.as_secs_f32())
                    .min(1.0);
                // Плавно в начале и в конце.
                let t = t * t * (3.0 - 2.0 * t);
                from + (to - from) * t
            }
        }
    }

    /// Двигается ли что-нибудь (тогда нужна перерисовка по таймеру).
    pub fn animating(&self, now: Instant) -> bool {
        let sliding = self
            .slide
            .is_some_and(|(_, _, start)| now.saturating_duration_since(start) < SLIDE);
        let bouncing = self
            .bounces
            .iter()
            .any(|(_, s)| now.saturating_duration_since(*s) < BOUNCE_LIMIT);
        sliding || bouncing
    }

    /// Закончить завершившиеся движения.
    pub fn settle(&mut self, now: Instant) {
        if let Some((_, to, start)) = self.slide
            && now.saturating_duration_since(start) >= SLIDE
        {
            self.hidden = to;
            self.slide = None;
        }
        self.bounces
            .retain(|(_, s)| now.saturating_duration_since(*s) < BOUNCE_LIMIT);
    }

    /// Спрятан полностью (и не едет).
    pub fn is_hidden(&self) -> bool {
        self.slide.is_none() && self.hidden >= 1.0
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    pub fn app(id: &str, name: &str) -> App {
        App {
            id: format!("{id}.desktop"),
            path: PathBuf::from(format!("/usr/share/applications/{id}.desktop")),
            name: name.into(),
            name_untranslated: name.into(),
            generic_name: None,
            comment: None,
            keywords: vec![],
            categories: vec![],
            icon: Some(id.into()),
            command: vec![id.into()],
            working_dir: None,
            terminal: false,
            startup_wm_class: None,
            no_display: false,
            prefers_dgpu: false,
        }
    }

    pub fn window(id: u32, instance: &str, class: &str) -> WindowInfo {
        WindowInfo {
            id,
            instance: instance.into(),
            class: class.into(),
            title: format!("окно {id}"),
            pid: None,
            minimized: false,
            maximized: false,
        }
    }

    pub const LOOK: Look = Look {
        icon: 48,
        zoom: 1.4,
        hide: DockHide::Smart,
    };

    pub fn dock() -> Dock {
        let apps = vec![
            app("thunar", "Проводник"),
            app("firefox", "Firefox"),
            app("org.xfce.mousepad", "Блокнот"),
            app("xfce4-terminal", "Терминал"),
        ];
        let pinned = vec![
            "thunar".into(),
            "firefox".into(),
            "org.xfce.mousepad".into(),
        ];
        Dock::new(LOOK, apps, pinned, 1600)
    }

    fn keys(d: &Dock) -> Vec<String> {
        d.items.iter().map(|i| i.key.clone()).collect()
    }

    #[test]
    fn pinned_then_running() {
        let mut d = dock();
        assert_eq!(
            keys(&d),
            [
                "thunar.desktop",
                "firefox.desktop",
                "org.xfce.mousepad.desktop"
            ]
        );
        d.set_windows(vec![
            (window(10, "xfce4-terminal", "Xfce4-terminal"), None),
            (window(11, "org.xfce.mousepad", "Org.xfce.mousepad"), None),
            (window(12, "xeyes", "XEyes"), None),
        ]);
        assert_eq!(
            keys(&d),
            [
                "thunar.desktop",
                "firefox.desktop",
                "org.xfce.mousepad.desktop",
                "xfce4-terminal.desktop",
                "window:12"
            ]
        );
        assert_eq!(d.items[2].windows, [11]);
        assert_eq!(d.windows_without_icon(), [12]);
    }

    #[test]
    fn pin_name_with_desktop_inside() {
        // Ярлык Telegram — org.telegram.desktop.desktop: отрезать «.desktop»
        // нужно один раз, иначе значок не находится и не закрепляется.
        let now = Instant::now();
        let apps = vec![app("org.telegram.desktop", "Telegram")];
        let mut d = Dock::new(LOOK, apps, vec![], 1600);
        d.set_windows(vec![(
            window(10, "telegram-desktop", "TelegramDesktop"),
            Some("/usr/share/applications/org.telegram.desktop.desktop".into()),
        )]);
        let key = "org.telegram.desktop.desktop";
        assert_eq!(
            d.command(key, MenuCommand::TogglePin, now),
            Action::Pinned(vec!["org.telegram.desktop".into()])
        );
        d.set_windows(vec![]);
        assert_eq!(keys(&d), [key], "закреплён и без окна");
        assert!(d.items[0].pinned);
        assert_eq!(
            d.command(key, MenuCommand::TogglePin, now),
            Action::Pinned(vec![])
        );
        assert!(keys(&d).is_empty());
    }

    #[test]
    fn trash_is_last_and_stays_put() {
        let now = Instant::now();
        let mut d = dock();
        d.set_trash(false);
        d.set_windows(vec![(window(10, "xfce4-terminal", "Xfce4-terminal"), None)]);
        assert_eq!(keys(&d).last().map(String::as_str), Some(TRASH));
        let n = d.items.len() - 1;
        assert!(d.items[n].separator_before, "перед корзиной разделитель");
        assert!(d.items[n - 1].separator_before, "и перед запущенными");
        assert_eq!(
            d.items[n].app.as_ref().unwrap().icon.as_deref(),
            Some("user-trash")
        );

        assert_eq!(d.click(n, 1, now), Action::OpenTrash);
        assert!(!d.animating(now), "корзина не прыгает");
        // Пустую не очистить, полную — можно; закрепить нельзя.
        let labels = |m: Menu| m.entries.into_iter().map(|e| e.command).collect::<Vec<_>>();
        assert_eq!(labels(d.menu(n).unwrap()), [MenuCommand::OpenTrash]);
        d.set_trash(true);
        assert_eq!(
            d.items[n].app.as_ref().unwrap().icon.as_deref(),
            Some("user-trash-full")
        );
        assert_eq!(
            labels(d.menu(n).unwrap()),
            [MenuCommand::OpenTrash, MenuCommand::EmptyTrash]
        );
        // Тащить корзину нельзя.
        d.start_drag(n, 100.0, 100.0);
        assert!(!d.dragging());
    }

    #[test]
    fn pin_and_unpin_from_menu() {
        let now = Instant::now();
        let mut d = dock();
        d.set_windows(vec![
            (window(10, "xfce4-terminal", "Xfce4-terminal"), None),
            (window(12, "xeyes", "XEyes"), None),
        ]);
        d.set_active(Some(10));
        fn labels(m: &Menu) -> Vec<(&str, bool)> {
            m.entries
                .iter()
                .map(|e| (e.label.as_str(), e.checked))
                .collect()
        }

        // Запущенный, не закреплённый Терминал: закрепить.
        let m = d.menu(3).unwrap();
        assert_eq!(
            labels(&m),
            [
                ("окно 10", true),
                ("Новое окно", false),
                ("Оставить в Dock", false),
                ("Завершить", false)
            ]
        );
        assert!(m.entries[1].separator_before, "окна отделены чертой");
        assert_eq!(
            d.command(&m.key, MenuCommand::Activate(10), now),
            Action::Activate(10)
        );
        let a = d.command(&m.key, MenuCommand::TogglePin, now);
        assert_eq!(
            a,
            Action::Pinned(vec![
                "thunar".into(),
                "firefox".into(),
                "org.xfce.mousepad".into(),
                "xfce4-terminal".into()
            ])
        );
        assert!(
            d.items[3].pinned,
            "теперь среди закреплённых, без разделителя"
        );
        assert!(!d.items[3].separator_before && d.items[4].separator_before);

        // Закреплённый незапущенный Firefox: открепить — значок пропадает.
        let m = d.menu(1).unwrap();
        assert_eq!(labels(&m), [("Открыть", false), ("Оставить в Dock", true)]);
        d.command(&m.key, MenuCommand::TogglePin, now);
        assert_eq!(
            keys(&d),
            [
                "thunar.desktop",
                "org.xfce.mousepad.desktop",
                "xfce4-terminal.desktop",
                "window:12"
            ]
        );

        // Окно без ярлыка: само окно и «Завершить».
        let m = d.menu(3).unwrap();
        assert_eq!(labels(&m), [("окно 12", false), ("Завершить", false)]);
        assert_eq!(
            d.command(&m.key, MenuCommand::Quit, now),
            Action::Close(vec![12])
        );
        assert_eq!(
            d.command(&m.key, MenuCommand::TogglePin, now),
            Action::Nothing
        );
    }

    #[test]
    fn click_actions() {
        let now = Instant::now();
        let mut d = dock();
        assert!(matches!(d.click(1, 1, now), Action::Launch(a) if a.name == "Firefox"));
        assert!(d.animating(now));
        d.set_windows(vec![
            (window(20, "Navigator", "firefox"), None),
            (window(21, "Navigator", "firefox"), None),
        ]);
        // Окно появилось — прыжки кончились.
        assert!(!d.animating(now));
        assert_eq!(d.click(1, 1, now), Action::Activate(21));
        d.set_active(Some(21));
        assert_eq!(d.click(1, 1, now), Action::Activate(20));
        d.set_windows(vec![(window(20, "Navigator", "firefox"), None)]);
        d.set_active(Some(20));
        assert_eq!(d.click(1, 1, now), Action::Minimize(vec![20]));
        assert!(matches!(d.click(1, 2, now), Action::Launch(_)));
        assert_eq!(d.click(9, 1, now), Action::Nothing);
    }

    #[test]
    fn slide() {
        let t0 = Instant::now();
        let mut d = dock();
        assert_eq!(d.hidden_at(t0), 0.0);
        d.slide_to(true, t0);
        let mid = d.hidden_at(t0 + SLIDE / 2);
        assert!(mid > 0.3 && mid < 0.7, "{mid}");
        assert!(d.animating(t0 + SLIDE / 2));
        d.settle(t0 + SLIDE);
        assert!(d.is_hidden());
        // Повторная просьба спрятаться ничего не начинает.
        d.slide_to(true, t0 + SLIDE);
        assert!(!d.animating(t0 + SLIDE));
    }
}
