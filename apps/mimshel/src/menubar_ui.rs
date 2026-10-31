//! Строка меню на экране: окно-полоса во всю ширину у верхнего края (с
//! резервом места), меню яблока, название активной программы, раскладка
//! (щелчок — следующая), громкость (окошко с ползунком, колёсико), батарея,
//! колокольчик («Не беспокоить»), трей (StatusNotifierItem), часы и
//! глобальное меню активной программы («Файл», «Правка»…).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mimshel_apps::App;
use mimshel_core::config::Config;
use mimshel_core::keyboard::layout_label;
use mimshel_dbus::backlight::Brightness;
use mimshel_dbus::bluetooth::{Bluetooth, BtCmd, BtState};
use mimshel_dbus::{
    AppMenu, AppMenuCmd, AppMenuUpdate, GtkPaths, MenuItem, Source, Tray, TrayCmd, TrayItem,
    TrayUpdate,
};
use mimshel_icons::Icons;
use mimshel_menubar::status::{Backlight, Battery, Sound};
use mimshel_menubar::{
    AppleItem, Bar, BluetoothAction, BluetoothPanel, CalendarAction, CalendarPanel, ControlAction,
    ControlPanel, ControlState, DRAG_SIZE, HiddenLook, NetState, NetworkAction, NetworkPanel, Part,
    SoundAction, SoundPanel, apple_menu, drag_image,
};
use mimshel_text::Fonts;
use mimshel_widgets::{Menu, MenuEntry, MenuResult};

use crate::platform::{Display, Event, Key, Panel, Popup, Property};
use crate::status::{self, Audio, ControlRequest, NetCmd};
use crate::ui::{Msg, Result};

/// Что делает пункт меню строки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmd {
    /// Пункт меню яблока (номер в `apple`).
    Apple(usize),
    /// Строка-сведения (заряд батареи) — ничего не делает.
    Info,
    PowerSettings,
    ToggleDnd,
    NotifySettings,
    /// Пункт меню значка трея (id пункта в DBusMenu).
    Tray(i32),
    /// Пункт глобального меню программы.
    Global(i32),
    /// Меню названия программы: свернуть или закрыть активное окно.
    Minimize,
    CloseWindow,
}

/// Шаг громкости колёсиком мыши над значком.
const VOLUME_STEP: u32 = 5;
/// Столько после своего изменения громкости не верим присланной: она
/// отстаёт и вернула бы старое значение между двумя щелчками колёсика.
const VOLUME_SETTLE: Duration = Duration::from_millis(700);

/// A menu bar icon held with the mouse: dragged down out of the bar it
/// moves into the Control Center, dragged from there up onto the bar it
/// comes back. Without moving it is a plain click, done on release (so a
/// drag does not also open an app or switch the layout).
struct IconDrag {
    /// The icon's name in `[menubar] hidden` (`layout`, `tray:ID`…).
    id: String,
    from: DragFrom,
    /// Where the button went down, screen points.
    start: (i32, i32),
    /// Moved far enough to be a drag; the picture that follows the pointer.
    active: Option<HiddenLook>,
    /// Where the pointer is now, screen points.
    at: (i32, i32),
}

#[derive(Clone, Copy)]
enum DragFrom {
    /// From the bar: the part, its left edge, and the part whose popup was
    /// open before the press (pressing it again only closes that popup).
    Bar {
        part: Part,
        px: f32,
        was: Option<Part>,
    },
    /// From the Control Center's hidden row (index in `control_hidden`).
    Control(usize),
}

/// How far the pointer must move before a press becomes a drag, points.
const DRAG_START: i32 = 6;

pub struct MenubarUi {
    window: Panel,
    bar: Bar,
    /// Все ярлыки — узнавать программу по окну.
    apps: Vec<App>,
    /// Раскладки по порядку групп (`["us", "ru"]`) и нынешняя.
    layouts: Vec<String>,
    group: u8,
    apple: Vec<AppleItem>,
    sound: Option<Sound>,
    battery: Option<Battery>,
    /// Screen backlight, re-read whenever the Control Center opens (the
    /// brightness keys change it behind our back).
    backlight: Option<Backlight>,
    brightness: Brightness,
    dnd: bool,
    audio: Sender<Audio>,
    control: SyncSender<ControlRequest>,
    network: mimshel_menubar::Network,
    bluetooth: Option<bool>,
    /// Когда громкость меняли отсюда.
    volume_changed: Option<Instant>,
    /// Трей (`None` — его держит кто-то другой, например панель Xfce),
    /// показанные значки и чьё меню ждём (ключ значка и где открыть).
    tray: Option<Tray>,
    tray_items: Vec<TrayItem>,
    tray_menu_wait: Option<(String, f32)>,
    /// Глобальное меню: служба чтения, меню какого окна показаны, сами меню и
    /// какое из них ждём, чтобы открыть (номер и левый край).
    appmenu: Option<AppMenu>,
    app_window: Option<u32>,
    app_menus: Vec<(String, Vec<MenuItem>)>,
    app_menu_wait: Option<(usize, f32)>,
    /// Открытое меню (яблока, батареи, колокольчика) или окошко громкости —
    /// в одном всплывающем окне; `popup_dirty` — перерисовать его.
    menu: Option<Menu<Cmd>>,
    sound_panel: Option<SoundPanel>,
    /// Пункт управления (в том же всплывающем окне).
    control_panel: Option<ControlPanel>,
    /// Календарь по щелчку на часах.
    calendar: Option<CalendarPanel>,
    /// Устройства Bluetooth (из Пункта управления), связь с BlueZ и последнее
    /// известное состояние.
    bluetooth_panel: Option<BluetoothPanel>,
    bt: Option<Bluetooth>,
    bt_state: Option<BtState>,
    /// Окошко сети, связь с NetworkManager и последнее известное состояние.
    network_panel: Option<NetworkPanel>,
    net: Sender<NetCmd>,
    net_state: Option<NetState>,
    /// Whether the system's week starts on Monday (read once, off the
    /// main thread: it takes a run of `locale`).
    monday_first: Arc<OnceLock<bool>>,
    /// `[menubar] clock_format` and the language the clock thread writes
    /// the names in (changed by `set_language`).
    clock_format: String,
    clock_language: Arc<Mutex<mimshel_core::i18n::Language>>,
    /// Today, refreshed with the clock every minute.
    today: Today,
    popup: Popup,
    popup_x: i16,
    /// `[menubar] hidden`: icons moved into the Control Center.
    hidden_ids: Vec<String>,
    /// The parts shown in the Control Center's hidden row, in its order.
    control_hidden: Vec<Part>,
    icon_drag: Option<IconDrag>,
    /// The picture under the pointer while an icon is dragged.
    drag_popup: Popup,
    drag_dirty: bool,
    /// The primary monitor (`Display::primary_area`), in points: the bar
    /// lies along its top edge, popups open under the bar on it.
    area: (i32, i32, u32, u32),
    popup_dirty: bool,
    dirty: bool,
    /// Служба — для просьб, которые выполняет не строка (центр уведомлений).
    tx: Sender<Msg>,
    language: mimshel_core::i18n::Language,
}

impl MenubarUi {
    /// Hot reload (v2): switch language without restarting the daemon.
    /// The bar is redrawn on the next flush; an already-open popup keeps
    /// its old wording until it is closed and reopened (its menu was
    /// built at open time).
    pub fn set_language(&mut self, lang: mimshel_core::i18n::Language) {
        self.language = lang;
        self.dirty = true;
        // The clock now, not at the next minute.
        *self
            .clock_language
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = lang;
        let (text, _, today) = clock(&self.clock_format, lang);
        self.set_clock(text, today);
        self.popup_dirty = true;
        if let Some(p) = &mut self.control_panel {
            p.set_language(lang);
        }
        if let Some(p) = &mut self.network_panel {
            p.set_language(lang);
        }
        if let Some(p) = &mut self.bluetooth_panel {
            p.set_language(lang);
        }
        if let Some(p) = &mut self.sound_panel {
            p.set_language(lang);
        }
    }

    pub fn new(d: &Display, config: &Config, apps: Vec<App>, tx: Sender<Msg>) -> Result<MenubarUi> {
        let (_, _, area_w, _) = d.primary_area();
        let mut ui = MenubarUi {
            window: d.create_panel("menubar")?,
            bar: Bar::new(area_w, config.menubar_height, &config.menu_icon),
            apps,
            layouts: Vec::new(),
            group: 0,
            apple: apple_menu(&apple_menu_file()),
            sound: None,
            battery: None,
            backlight: None,
            brightness: Brightness::start(),
            dnd: false,
            audio: status::audio_worker(tx.clone()),
            control: status::control_worker(tx.clone()),
            network: mimshel_menubar::Network::default(),
            bluetooth: None,
            volume_changed: None,
            tray: None,
            tray_items: Vec::new(),
            tray_menu_wait: None,
            appmenu: None,
            app_window: None,
            app_menus: Vec::new(),
            app_menu_wait: None,
            menu: None,
            sound_panel: None,
            control_panel: None,
            calendar: None,
            bluetooth_panel: None,
            bt: None,
            bt_state: None,
            network_panel: None,
            net: status::network_worker(tx.clone()),
            net_state: None,
            monday_first: Arc::new(OnceLock::new()),
            clock_format: config.clock_format.clone(),
            clock_language: Arc::new(Mutex::new(config.language)),
            today: (
                mimshel_menubar::Date {
                    year: 1970,
                    month: 1,
                    day: 1,
                },
                String::new(),
            ),
            popup: d.create_popup("menubar-menu")?,
            popup_x: 0,
            hidden_ids: config.menubar_hidden.clone(),
            control_hidden: Vec::new(),
            icon_drag: None,
            drag_popup: d.create_popup("menubar-drag")?,
            drag_dirty: false,
            area: d.primary_area(),
            popup_dirty: false,
            dirty: true,
            tx: tx.clone(),
            language: config.language,
        };
        d.watch_desktop()?;
        d.watch_keyboard()?;
        ui.read_layouts(d);
        ui.bar.app_name = ui.app_name(d);
        let (text, _, today) = clock(&config.clock_format, config.language);
        ui.bar.clock = text;
        ui.today = today;
        {
            let monday_first = Arc::clone(&ui.monday_first);
            std::thread::spawn(move || {
                let _ = monday_first.set(system_monday_first());
            });
        }
        ui.bar.control = true;
        ui.refresh_hidden();
        watch_clock(
            config.clock_format.clone(),
            Arc::clone(&ui.clock_language),
            tx.clone(),
        );
        status::watch_sound(tx.clone());
        status::watch_battery(tx.clone());
        status::watch_dnd(tx.clone());
        status::watch_network(tx.clone());
        let tray_tx = tx.clone();
        let bt_tx = tx.clone();
        let menu_tx = tx;
        ui.bt = match mimshel_dbus::bluetooth::start(move |s| {
            let _ = bt_tx.send(Msg::Bluetooth(s));
        }) {
            Ok(bt) => Some(bt),
            Err(e) => {
                eprintln!("Bluetooth не подключён (нет системной шины?): {e}");
                None
            }
        };
        ui.appmenu = match mimshel_dbus::appmenu::start(move |u| {
            let _ = menu_tx.send(Msg::AppMenu(u));
        }) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("Глобальное меню не запущено: {e}");
                None
            }
        };
        ui.load_app_menu(d, None);
        ui.tray = match mimshel_dbus::tray::start(move |u| {
            let _ = tray_tx.send(Msg::Tray(u));
        }) {
            Ok(tray) => Some(tray),
            Err(e) => {
                eprintln!("Трей не запущен (его держит другая программа?): {e}");
                None
            }
        };
        ui.place(d)?;
        ui.window.show(d)?;
        Ok(ui)
    }

    pub fn set_apps(&mut self, d: &Display, apps: Vec<App>) {
        self.apps = apps;
        self.bar.app_name = self.app_name(d);
        self.dirty = true;
    }

    /// Подсветить колокольчик, пока открыт центр уведомлений.
    pub fn set_center_open(&mut self, open: bool) {
        let want = open.then_some(Part::Bell);
        let other = self.bar.open.is_some_and(|p| p != Part::Bell);
        if !other && self.bar.open != want {
            self.bar.open = want;
            self.dirty = true;
        }
    }

    /// Новое состояние сети — в открытое окошко сети. Кто сейчас подключается
    /// и последнюю ошибку знает окошко, а не NetworkManager.
    pub fn set_net_state(&mut self, state: NetState) {
        if let Some(panel) = &mut self.network_panel {
            let (busy, error) = (panel.state.busy.take(), panel.state.error.take());
            // Пустой список посреди поиска не показываем — ждём свежий.
            let networks = if state.networks.is_empty() && state.scanning {
                std::mem::take(&mut panel.state.networks)
            } else {
                state.networks.clone()
            };
            panel.state = NetState {
                busy,
                error,
                networks,
                ..state.clone()
            };
            self.popup_dirty = true;
        }
        self.net_state = Some(state);
    }

    /// Подключение к сети `ssid` закончилось.
    pub fn net_done(&mut self, ssid: &str, error: Option<String>) {
        if let Some(panel) = &mut self.network_panel {
            if panel.state.busy.as_deref() == Some(ssid) {
                panel.state.busy = None;
            }
            panel.state.error = error;
            self.popup_dirty = true;
        }
    }

    /// Новое состояние Bluetooth — в открытое окошко устройств.
    pub fn set_bluetooth_state(&mut self, state: Option<BtState>) {
        if let Some(panel) = &mut self.bluetooth_panel {
            match &state {
                Some(s) => panel.state = s.clone(),
                // Служба bluetooth остановилась — показывать нечего.
                None => panel.state.powered = false,
            }
            self.popup_dirty = true;
        }
        self.bt_state = state;
        let on = self.bluetooth_on();
        if let Some(panel) = &mut self.control_panel {
            panel.state.bluetooth = on;
            self.popup_dirty = true;
        }
    }

    pub fn set_clock(&mut self, text: String, today: Today) {
        self.today = today;
        if self.bar.clock != text {
            self.bar.clock = text;
            self.dirty = true;
        }
    }

    pub fn set_sound(&mut self, mut sound: Option<Sound>) {
        if let (Some(new), Some(old)) = (&mut sound, &self.sound)
            && self
                .volume_changed
                .is_some_and(|t| t.elapsed() < VOLUME_SETTLE)
        {
            new.volume = old.volume;
            new.muted = old.muted;
            // Громкости программ — как стоят ползунки в окошке.
            if let Some(panel) = &self.sound_panel {
                for app in &mut new.apps {
                    if let Some(shown) = panel.sound.apps.iter().find(|a| a.name == app.name) {
                        app.volume = shown.volume;
                        app.muted = shown.muted;
                    }
                }
            }
        }
        self.bar.volume = sound.as_ref().map(|s| s.icon().to_string());
        // Пока тащат ползунок, он главнее: ответ звуковой системы отстаёт.
        if let (Some(panel), Some(s)) = (&mut self.sound_panel, &sound)
            && !panel.dragging()
        {
            panel.sound = s.clone();
            self.popup_dirty = true;
        }
        if let Some(panel) = &mut self.control_panel
            && !panel.dragging()
        {
            panel.state.sound = sound.clone();
            self.popup_dirty = true;
        }
        self.sound = sound;
        self.dirty = true;
    }

    pub fn set_control(&mut self, network: mimshel_menubar::Network, bluetooth: Option<bool>) {
        self.network = network;
        self.bluetooth = bluetooth;
        let bluetooth = self.bluetooth_on();
        if let Some(panel) = &mut self.control_panel {
            panel.state.network = self.network.clone();
            panel.state.bluetooth = bluetooth;
            self.popup_dirty = true;
        }
    }

    /// Включён ли Bluetooth: у службы bluetooth (адаптер включают и выключают
    /// там), без неё — по блокировке радио (rfkill). `None` — адаптера нет.
    fn bluetooth_on(&self) -> Option<bool> {
        match &self.bt_state {
            Some(s) => Some(s.powered),
            None => self.bluetooth,
        }
    }

    pub fn set_battery(&mut self, battery: Option<Battery>) {
        self.bar.battery = battery
            .as_ref()
            .map(|b| (b.icon.clone(), format!("{}%", b.percent)));
        self.battery = battery;
        self.dirty = true;
    }

    pub fn set_dnd(&mut self, on: bool) {
        self.dnd = on;
        let icon = if on {
            "notification-disabled-symbolic"
        } else {
            "notification-symbolic"
        };
        self.bar.bell = Some(icon.into());
        if let Some(panel) = &mut self.control_panel {
            panel.state.dnd = on;
            self.popup_dirty = true;
        }
        self.dirty = true;
    }

    pub fn popup_open(&self) -> bool {
        self.menu.is_some()
            || self.sound_panel.is_some()
            || self.control_panel.is_some()
            || self.calendar.is_some()
            || self.bluetooth_panel.is_some()
            || self.network_panel.is_some()
    }

    /// Окно строки меню (сама полоса или её всплывающее окно).
    pub fn owns(&self, window: u32) -> bool {
        window == self.window.id() || window == self.popup.id()
    }

    /// Это событие для строки меню?
    pub fn wants(&self, d: &Display, ev: &Event) -> bool {
        match *ev {
            Event::Button { window, .. }
            | Event::Release { window, .. }
            | Event::Motion { window, .. }
            | Event::Expose { window } => window == self.window.id() || window == self.popup.id(),
            Event::Key(_) => self.popup_open(),
            Event::Property { window, what } => {
                d.is_screen(window) && what == Property::ActiveWindow
            }
            Event::Configure { window } => d.is_screen(window),
            Event::KeyboardGroup(_) | Event::KeyboardLayouts => true,
            _ => false,
        }
    }

    pub fn event(&mut self, d: &Display, fonts: &mut Fonts, ev: Event) -> Result<()> {
        let popup = self.popup.id();
        if self.icon_drag.is_some() {
            match ev {
                Event::Motion { window, x, y } => {
                    let at = self.to_screen(window, x, y);
                    return self.icon_drag_motion(d, at);
                }
                Event::Release {
                    window,
                    x,
                    y,
                    button: 1,
                } => {
                    let at = self.to_screen(window, x, y);
                    return self.icon_drag_end(d, fonts, at);
                }
                _ => {}
            }
        }
        match ev {
            // Открытое меню или окошко громкости: их мышь и клавиатура.
            Event::Motion { window, x, y } if window == popup => {
                let (x, y) = (f32::from(x), f32::from(y));
                if let Some(menu) = &mut self.menu
                    && menu.motion(x, y)
                {
                    self.popup_dirty = true;
                }
                if let Some(panel) = &mut self.sound_panel {
                    let action = panel.motion(x, y);
                    self.sound_action(d, action)?;
                }
                if let Some(panel) = &mut self.control_panel {
                    let action = panel.motion(x, y);
                    self.control_action(d, action)?;
                }
                if let Some(panel) = &mut self.calendar {
                    let action = panel.motion(x, y);
                    self.calendar_action(d, action)?;
                }
                if let Some(panel) = &mut self.bluetooth_panel {
                    let action = panel.motion(x, y);
                    self.bluetooth_action(d, action)?;
                }
                if let Some(panel) = &mut self.network_panel {
                    let action = panel.motion(x, y);
                    self.network_action(d, action)?;
                }
            }
            Event::Button {
                window,
                x: ex,
                y: ey,
                button,
            } if window == popup => {
                let (x, y) = (f32::from(ex), f32::from(ey));
                // A hidden icon in the Control Center: a click or a drag back
                // to the bar, decided on release.
                if button == 1
                    && let Some(i) = self.control_panel.as_ref().and_then(|p| p.hidden_at(x, y))
                    && let Some(id) = self.control_hidden.get(i).and_then(|&p| self.icon_id(p))
                {
                    let start = self.to_screen(window, ex, ey);
                    self.icon_drag = Some(IconDrag {
                        id,
                        from: DragFrom::Control(i),
                        start,
                        active: None,
                        at: start,
                    });
                    return Ok(());
                }
                if let Some(menu) = &self.menu {
                    let result = menu.click(x, y);
                    self.menu_result(d, result)?;
                } else if let Some(panel) = &mut self.sound_panel {
                    let action = panel.press(x, y);
                    self.sound_action(d, action)?;
                } else if let Some(panel) = &mut self.control_panel {
                    let action = panel.press(x, y);
                    self.control_action(d, action)?;
                } else if let Some(panel) = &mut self.calendar {
                    let action = panel.press(x, y);
                    self.calendar_action(d, action)?;
                } else if let Some(panel) = &mut self.bluetooth_panel {
                    let action = panel.press(x, y);
                    self.bluetooth_action(d, action)?;
                } else if let Some(panel) = &mut self.network_panel {
                    let action = panel.press(x, y);
                    self.network_action(d, action)?;
                }
            }
            Event::Release { window, .. } if window == popup => {
                if let Some(panel) = &mut self.sound_panel {
                    panel.release();
                }
                if let Some(panel) = &mut self.control_panel {
                    panel.release();
                }
            }
            Event::Key(k) => {
                // ← / → — соседнее меню программы.
                if let Some(Part::Menu(i)) = self.bar.open
                    && matches!(k.key, Key::Left | Key::Right)
                    && !self.app_menus.is_empty()
                {
                    let n = self.app_menus.len();
                    let j = if k.key == Key::Right {
                        (i + 1) % n
                    } else {
                        (i + n - 1) % n
                    };
                    if let Some((_, px)) = self
                        .bar
                        .parts(fonts)
                        .into_iter()
                        .find(|p| p.0 == Part::Menu(j))
                        .map(|p| (p.0, p.1))
                    {
                        self.close_popup(d)?;
                        self.open_global(d, fonts, j, px)?;
                    }
                    return Ok(());
                }
                if let Some(menu) = &mut self.menu {
                    let result = menu.key(k.key);
                    self.menu_result(d, result)?;
                } else if let Some(panel) = &mut self.sound_panel {
                    let action = panel.key(k.key);
                    self.sound_action(d, action)?;
                } else if let Some(panel) = &mut self.control_panel {
                    let action = panel.key(k.key);
                    self.control_action(d, action)?;
                } else if let Some(panel) = &mut self.calendar {
                    let action = panel.key(k.key);
                    self.calendar_action(d, action)?;
                } else if let Some(panel) = &mut self.bluetooth_panel {
                    let action = panel.key(k.key);
                    self.bluetooth_action(d, action)?;
                } else if let Some(panel) = &mut self.network_panel {
                    let action = panel.key(k.key);
                    self.network_action(d, action)?;
                }
            }
            Event::Expose { window } if window == popup => self.popup_dirty = true,
            Event::Button {
                window,
                x,
                y,
                button,
            } => {
                let part = self.bar.part_at(fonts, f32::from(x));
                if mimshel_core::names::flag("DEBUG_DOCK") {
                    eprintln!(
                        "строка меню: кнопка {button} по {part:?}, раскладки {:?}, нынешняя {}",
                        self.layouts, self.group
                    );
                }
                // Значок трея: правая кнопка — меню, средняя — второе
                // действие, колёсико — прокрутка.
                if let Some((Part::Tray(i), px)) = part
                    && button != 1
                {
                    if self.popup_open() {
                        self.close_popup(d)?;
                    }
                    return self.tray_click(i, px, button);
                }
                // Колёсико и средняя кнопка над динамиком — громкость и «без звука».
                if let Some((Part::Volume, _)) = part
                    && let Some(sound) = &self.sound
                {
                    let v = sound.volume;
                    match button {
                        4 => self.change_volume(d, (v + VOLUME_STEP).min(100))?,
                        5 => self.change_volume(d, v.saturating_sub(VOLUME_STEP))?,
                        2 => {
                            let _ = self.audio.send(Audio::ToggleMute);
                        }
                        _ => {}
                    }
                }
                if button != 1 {
                    if self.popup_open() && !matches!(button, 4 | 5) {
                        self.close_popup(d)?;
                    }
                    // Правая кнопка по колокольчику — «Не беспокоить» и настройки.
                    if button == 3
                        && let Some((Part::Bell, px)) = part
                    {
                        self.open_bell(d, fonts, px)?;
                    }
                    return Ok(());
                }
                // Щелчок при открытом меню: по той же части — закрыть, по
                // другой — открыть её меню, как на Mac.
                let was = self.bar.open;
                if self.popup_open() {
                    self.close_popup(d)?;
                }
                // An icon that can move to the Control Center: wait for the
                // release — a click, or a drag out of the bar.
                if let Some((p, px)) = part
                    && Bar::movable(p)
                    && let Some(id) = self.icon_id(p)
                {
                    let start = self.to_screen(window, x, y);
                    self.icon_drag = Some(IconDrag {
                        id,
                        from: DragFrom::Bar { part: p, px, was },
                        start,
                        active: None,
                        at: start,
                    });
                    return Ok(());
                }
                match part {
                    // Icons that can move (bell, battery, volume, tray,
                    // layout) are clicked on release; this is the rare case
                    // of one without a name to remember it by.
                    Some((p, px)) if Bar::movable(p) => self.click_part(d, fonts, p, px, was)?,
                    Some((p, _)) if Some(p) == was => {}
                    Some((Part::Clock, px)) => self.open_calendar(d, px)?,
                    Some((Part::Apple, px)) => self.open_apple(d, fonts, px)?,
                    Some((Part::Control, px)) => self.open_control(d, px)?,
                    Some((Part::Menu(i), px)) => {
                        // Читаем меню заново: доступность пунктов могла поменяться.
                        self.app_menu_wait = Some((i, px));
                        self.bar.open = Some(Part::Menu(i));
                        self.dirty = true;
                        self.load_app_menu(d, Some(i));
                    }
                    Some((Part::App, px)) => self.open_app(d, fonts, px)?,
                    _ => {}
                }
            }
            Event::Expose { .. } => self.dirty = true,
            Event::Property { .. } => {
                let name = self.app_name(d);
                if name != self.bar.app_name {
                    self.bar.app_name = name;
                    self.dirty = true;
                }
                if d.active_window() != self.app_window {
                    // Перешли в другое окно — открытое окошко больше не нужно
                    // (и не должно держать клавиатуру).
                    self.close_popup(d)?;
                    self.load_app_menu(d, None);
                }
            }
            // Меню открыто, мышь ведут по строке: другое меню программы —
            // сразу оно, как на Mac.
            Event::Motion { x, .. } => {
                if let Some(Part::Menu(open)) = self.bar.open
                    && let Some((Part::Menu(j), px)) = self.bar.part_at(fonts, f32::from(x))
                    && j != open
                    && self.menu.is_some()
                {
                    self.close_popup(d)?;
                    self.open_global(d, fonts, j, px)?;
                }
            }
            Event::Configure { .. } => self.place(d)?,
            Event::KeyboardGroup(g) => {
                self.group = g;
                self.update_layout_label();
            }
            Event::KeyboardLayouts => self.read_layouts(d),
            _ => {}
        }
        Ok(())
    }

    /// A left click on a movable icon at left edge `px`; `was` — the part
    /// whose popup was open before (clicking it again only closed it).
    fn click_part(
        &mut self,
        d: &Display,
        fonts: &mut Fonts,
        part: Part,
        px: f32,
        was: Option<Part>,
    ) -> Result<()> {
        match part {
            // Колокольчик — центр уведомлений (его рисует служба
            // уведомлений; повторный щелчок закрывает его там же).
            Part::Bell => {
                let _ = self.tx.send(Msg::ToggleCenter);
            }
            p if Some(p) == was => {}
            Part::Battery => self.open_battery(d, fonts, px)?,
            Part::Volume => self.open_sound(d, px)?,
            // Значок Bluetooth (blueman) — своё окошко устройств,
            // как из Пункта управления; правая кнопка — меню blueman.
            Part::Tray(i)
                if self.bt_state.is_some()
                    && self.tray_items.get(i).is_some_and(is_bluetooth_tray) =>
            {
                self.open_bluetooth(d, Part::Tray(i), px)?
            }
            // Значок сети (nm-applet) — своё окошко сети.
            Part::Tray(i) if self.tray_items.get(i).is_some_and(is_network_tray) => {
                self.open_network(d, Part::Tray(i), px)?
            }
            Part::Tray(i) => self.tray_click(i, px, 1)?,
            Part::Layout if was.is_none() => self.next_layout(d)?,
            _ => {}
        }
        Ok(())
    }

    /// The settings name of a movable icon: `layout`, `volume`, `battery`,
    /// `bell`, or `tray:ID` (the app's StatusNotifierItem `Id`, its title
    /// when it has none).
    fn icon_id(&self, part: Part) -> Option<String> {
        match part {
            Part::Layout => Some("layout".into()),
            Part::Volume => Some("volume".into()),
            Part::Battery => Some("battery".into()),
            Part::Bell => Some("bell".into()),
            Part::Tray(i) => {
                let item = self.tray_items.get(i)?;
                let name = if item.id.is_empty() {
                    &item.title
                } else {
                    &item.id
                };
                (!name.is_empty()).then(|| format!("tray:{name}"))
            }
            _ => None,
        }
    }

    /// Mark the icons listed in `[menubar] hidden` as hidden in the bar.
    fn refresh_hidden(&mut self) {
        let parts = [Part::Layout, Part::Volume, Part::Battery, Part::Bell]
            .into_iter()
            .chain((0..self.tray_items.len()).map(Part::Tray));
        let hidden: Vec<Part> = parts
            .filter(|&p| {
                self.icon_id(p)
                    .is_some_and(|id| self.hidden_ids.contains(&id))
            })
            .collect();
        if hidden != self.bar.hidden {
            self.bar.hidden = hidden;
            self.dirty = true;
        }
    }

    /// `[menubar] hidden` changed in the file (hot reload).
    pub fn set_hidden_ids(&mut self, ids: Vec<String>) {
        self.hidden_ids = ids;
        self.refresh_hidden();
    }

    /// What a movable icon looks like now; `None` — it is not there (no
    /// battery, a single layout…).
    fn look_of(&self, part: Part) -> Option<HiddenLook> {
        match part {
            Part::Tray(i) => self.bar.tray.get(i).cloned().map(HiddenLook::Picture),
            Part::Layout => self.bar.layout.clone().map(HiddenLook::Text),
            Part::Volume => self.bar.volume.clone().map(HiddenLook::Icon),
            Part::Battery => self
                .bar
                .battery
                .as_ref()
                .map(|(icon, _)| HiddenLook::Icon(icon.clone())),
            Part::Bell => self.bar.bell.clone().map(HiddenLook::Icon),
            _ => None,
        }
    }

    /// Hidden icons that are there now, in bar order (tray, then the rest).
    fn hidden_present(&self) -> Vec<(Part, HiddenLook)> {
        let mut parts = self.bar.hidden.clone();
        let order = |p: &Part| match p {
            Part::Tray(i) => *i,
            Part::Layout => 1000,
            Part::Volume => 1001,
            Part::Battery => 1002,
            _ => 1003,
        };
        parts.sort_by_key(order);
        parts
            .into_iter()
            .filter_map(|p| Some((p, self.look_of(p)?)))
            .collect()
    }

    /// An event point of our bar or popup window, in screen points.
    fn to_screen(&self, window: u32, x: i16, y: i16) -> (i32, i32) {
        let (ax, ay, _, _) = self.area;
        let (ox, oy) = if window == self.popup.id() {
            let (px, py) = self.popup_origin();
            (i32::from(px), i32::from(py))
        } else {
            (ax, ay)
        };
        (ox + i32::from(x), oy + i32::from(y))
    }

    /// The pointer moved with an icon held: past [`DRAG_START`] it becomes
    /// a drag; the picture follows the pointer.
    fn icon_drag_motion(&mut self, d: &Display, at: (i32, i32)) -> Result<()> {
        let Some(drag) = &self.icon_drag else {
            return Ok(());
        };
        let (dx, dy) = (at.0 - drag.start.0, at.1 - drag.start.1);
        if drag.active.is_none() {
            if dx.abs().max(dy.abs()) < DRAG_START {
                return Ok(());
            }
            let part = match drag.from {
                DragFrom::Bar { part, .. } => Some(part),
                DragFrom::Control(i) => self.control_hidden.get(i).copied(),
            };
            let Some(look) = part.and_then(|p| self.look_of(p)) else {
                self.icon_drag = None;
                return Ok(());
            };
            // Out of the Control Center: it closes, the drag goes on. The
            // pointer is held on the bar window either way, so the release
            // comes back to us wherever it happens.
            self.close_popup(d)?;
            d.grab_pointer_for_drag(self.window.id())?;
            if let Some(drag) = &mut self.icon_drag {
                drag.active = Some(look);
            }
        }
        if let Some(drag) = &mut self.icon_drag {
            drag.at = at;
        }
        self.drag_dirty = true;
        Ok(())
    }

    /// The button went up: drop the dragged icon, or click it.
    fn icon_drag_end(&mut self, d: &Display, fonts: &mut Fonts, at: (i32, i32)) -> Result<()> {
        let Some(drag) = self.icon_drag.take() else {
            return Ok(());
        };
        if drag.active.is_some() {
            d.ungrab_pointer()?;
            self.drag_popup.hide_passive(d)?;
            let (ax, ay, aw, _) = self.area;
            let bar_bottom = ay + self.bar.height as i32;
            let over_bar = at.1 < bar_bottom + 4 && at.0 >= ax && at.0 < ax + aw as i32;
            // Dropped onto the Control Center icon itself counts too: that is
            // where people aim first.
            let on_control = over_bar
                && matches!(
                    self.bar.part_at(fonts, (at.0 - ax) as f32),
                    Some((Part::Control, _))
                );
            match drag.from {
                // Anywhere below the bar, or onto the Control Center icon.
                // Live test: a drop 10 px under the bar was ignored with a
                // stricter margin, and the user took it for a bug.
                DragFrom::Bar { .. } if at.1 >= bar_bottom || on_control => {
                    self.move_icon(&drag.id, true);
                }
                DragFrom::Control(_) if over_bar => self.move_icon(&drag.id, false),
                _ => {}
            }
            return Ok(());
        }
        match drag.from {
            DragFrom::Bar { part, px, was } => self.click_part(d, fonts, part, px, was),
            // A click on a hidden icon: what a click on it in the bar does,
            // opened where the Control Center was.
            DragFrom::Control(i) => {
                let Some(part) = self.control_hidden.get(i).copied() else {
                    return Ok(());
                };
                let x = f32::from(self.popup_x);
                self.close_popup(d)?;
                self.click_part(d, fonts, part, x, None)
            }
        }
    }

    /// Move icon `id` into the Control Center (`hide`) or back to the bar,
    /// and remember it in `[menubar] hidden`.
    fn move_icon(&mut self, id: &str, hide: bool) {
        self.hidden_ids.retain(|h| h != id);
        if hide {
            self.hidden_ids.push(id.to_string());
        }
        self.refresh_hidden();
        if let Some(path) = crate::ui::config_path()
            && let Err(e) = save_hidden(&path, &self.hidden_ids)
        {
            eprintln!(
                "Не удалось сохранить скрытые значки в {}: {e}",
                path.display()
            );
        }
    }

    fn change_volume(&mut self, _d: &Display, v: u32) -> Result<()> {
        let _ = self.audio.send(Audio::Volume(v));
        self.volume_changed = Some(Instant::now());
        if let Some(sound) = &mut self.sound {
            sound.volume = v;
            sound.muted = false;
            self.bar.volume = Some(sound.icon().to_string());
            self.dirty = true;
        }
        Ok(())
    }

    fn sound_action(&mut self, d: &Display, action: SoundAction) -> Result<()> {
        match action {
            SoundAction::Nothing => {}
            SoundAction::Redraw => self.popup_dirty = true,
            SoundAction::Close => self.close_popup(d)?,
            SoundAction::SetVolume(v) => {
                self.change_volume(d, v)?;
                self.popup_dirty = true;
            }
            SoundAction::SetAppVolume(i, v) => {
                if let Some(panel) = &self.sound_panel
                    && let Some(app) = panel.sound.apps.get(i)
                {
                    let _ = self.audio.send(Audio::AppVolume(app.inputs.clone(), v));
                    self.volume_changed = Some(Instant::now());
                    self.popup_dirty = true;
                }
            }
            SoundAction::SetSink(i) => {
                if let Some(panel) = &mut self.sound_panel
                    && let Some(sink) = panel.sound.sinks.get(i)
                {
                    let _ = self.audio.send(Audio::Sink(sink.name.clone()));
                    panel.sound.default = sink.name.clone();
                    self.popup_dirty = true;
                }
            }
            SoundAction::Settings => {
                self.close_popup(d)?;
                run(&["pavucontrol".to_string()]);
            }
        }
        Ok(())
    }

    fn menu_result(&mut self, d: &Display, result: MenuResult<Cmd>) -> Result<()> {
        match result {
            MenuResult::Nothing => {}
            MenuResult::Redraw => self.popup_dirty = true,
            MenuResult::Close => self.close_popup(d)?,
            MenuResult::Run(cmd) => {
                let key = self
                    .menu
                    .as_ref()
                    .map(|m| m.key.clone())
                    .unwrap_or_default();
                self.close_popup(d)?;
                match cmd {
                    Cmd::Apple(i) => {
                        if let Some(item) = self.apple.get(i) {
                            run(&item.command);
                        }
                    }
                    Cmd::Info => {}
                    Cmd::PowerSettings => run(&["xfce4-power-manager-settings".to_string()]),
                    Cmd::ToggleDnd => {
                        let on = !self.dnd;
                        self.set_dnd(on);
                        std::thread::spawn(move || status::set_dnd(on));
                    }
                    Cmd::NotifySettings => run(&["xfce4-notifyd-config".to_string()]),
                    Cmd::Tray(id) => {
                        if let Some(tray) = &self.tray {
                            tray.send(TrayCmd::MenuClicked(key, id));
                        }
                    }
                    Cmd::Global(id) => {
                        if let Some(appmenu) = &self.appmenu {
                            appmenu.send(AppMenuCmd::Activate(id));
                        }
                    }
                    Cmd::Minimize => {
                        if let Some(w) = d.active_window() {
                            d.minimize(w)?;
                        }
                    }
                    Cmd::CloseWindow => {
                        if let Some(w) = d.active_window() {
                            d.close(w)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Показать всплывающее окно под частью строки (левый край — `x`).
    fn show_popup(&mut self, d: &Display, part: Part, x: f32, (w, h): (u32, u32)) -> Result<()> {
        let x = x.min(self.bar.width as f32 - w as f32 - 4.0).max(4.0);
        self.popup_x = x as i16;
        let (px, py) = self.popup_origin();
        self.popup.show(d, px, py, w as u16, h as u16)?;
        self.popup_dirty = true;
        self.bar.open = Some(part);
        self.dirty = true;
        Ok(())
    }

    fn open_menu(
        &mut self,
        d: &Display,
        fonts: &mut Fonts,
        part: Part,
        key: String,
        x: f32,
        entries: Vec<MenuEntry<Cmd>>,
    ) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut menu = Menu::new(key, entries);
        let size = menu.measure(fonts);
        self.menu = Some(menu);
        self.show_popup(d, part, x, size)
    }

    fn apple_label(lang: mimshel_core::i18n::Language, item: &AppleItem) -> String {
        let action = item
            .command
            .iter()
            .position(|c| c == "action")
            .and_then(|i| item.command.get(i + 1))
            .map(|s| s.as_str());
        let key = match action {
            Some("about") => "apple.about",
            Some("settings") => "apple.settings",
            Some("forcequit") => "apple.forcequit",
            Some("sleep") => "apple.sleep",
            Some("restart") => "apple.restart",
            Some("shutdown") => "apple.shutdown",
            Some("lock") => "apple.lock",
            Some("logout") => "apple.logout",
            _ => return item.label.clone(),
        };
        mimshel_core::i18n::t(lang, key).to_string()
    }

    fn open_apple(&mut self, d: &Display, fonts: &mut Fonts, x: f32) -> Result<()> {
        // Пункты перечитываем: `mimshel apply launchers` мог их поменять.
        self.apple = apple_menu(&apple_menu_file());
        let entries = self
            .apple
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let label = Self::apple_label(self.language, item);
                entry(&label, Cmd::Apple(i), false, item.separator_before)
            })
            .collect();
        self.open_menu(d, fonts, Part::Apple, "apple".into(), x, entries)
    }

    fn open_battery(&mut self, d: &Display, fonts: &mut Fonts, x: f32) -> Result<()> {
        let Some(b) = &self.battery else {
            return Ok(());
        };
        let entries = vec![
            entry(
                &format!(
                    "{}: {} %",
                    mimshel_core::i18n::t(self.language, "battery.title"),
                    b.percent
                ),
                Cmd::Info,
                false,
                false,
            ),
            entry(&b.state_text_lang(self.language), Cmd::Info, false, false),
            entry(
                mimshel_core::i18n::t(self.language, "battery.power_settings"),
                Cmd::PowerSettings,
                false,
                true,
            ),
        ];
        self.open_menu(d, fonts, Part::Battery, "battery".into(), x, entries)
    }

    fn open_bell(&mut self, d: &Display, fonts: &mut Fonts, x: f32) -> Result<()> {
        let entries = vec![
            entry(
                mimshel_core::i18n::t(self.language, "notify.do_not_disturb"),
                Cmd::ToggleDnd,
                self.dnd,
                false,
            ),
            entry(
                mimshel_core::i18n::t(self.language, "notify.settings"),
                Cmd::NotifySettings,
                false,
                true,
            ),
        ];
        self.open_menu(d, fonts, Part::Bell, "bell".into(), x, entries)
    }

    fn open_calendar(&mut self, d: &Display, x: f32) -> Result<()> {
        // Opened in the first moments after start: read it now.
        let monday_first = *self.monday_first.get_or_init(system_monday_first);
        let names = calendar_names(self.language, monday_first);
        let (today, text) = self.today.clone();
        let panel = CalendarPanel::new(today, text, names);
        let size = panel.size();
        self.calendar = Some(panel);
        self.show_popup(d, Part::Clock, x, size)
    }

    fn calendar_action(&mut self, d: &Display, action: CalendarAction) -> Result<()> {
        match action {
            CalendarAction::Nothing => {}
            CalendarAction::Redraw => self.popup_dirty = true,
            CalendarAction::Close => self.close_popup(d)?,
        }
        Ok(())
    }

    fn open_control(&mut self, d: &Display, x: f32) -> Result<()> {
        let _ = self.control.try_send(ControlRequest::Refresh);
        let _ = self.audio.send(Audio::Refresh);
        self.backlight = status::read_backlight();
        let mut panel = ControlPanel::new(ControlState {
            network: self.network.clone(),
            bluetooth: self.bluetooth_on(),
            dnd: self.dnd,
            display: self.backlight.as_ref().map(Backlight::percent),
            sound: self.sound.clone(),
        });
        panel.set_language(self.language);
        let (parts, looks) = self.hidden_present().into_iter().unzip();
        self.control_hidden = parts;
        panel.hidden = looks;
        let size = panel.size();
        self.control_panel = Some(panel);
        self.show_popup(d, Part::Control, x, size)
    }

    fn control_action(&mut self, d: &Display, action: ControlAction) -> Result<()> {
        match action {
            ControlAction::Nothing => {}
            ControlAction::Redraw => self.popup_dirty = true,
            ControlAction::Close => self.close_popup(d)?,
            ControlAction::SetWifi(on) => {
                let _ = self.control.try_send(ControlRequest::Wifi(on));
                self.popup_dirty = true;
            }
            ControlAction::SetBluetooth(on) => {
                match &self.bt {
                    Some(bt) => bt.send(BtCmd::Power(on)),
                    None => {
                        let _ = self.control.try_send(ControlRequest::Bluetooth(on));
                    }
                }
                self.popup_dirty = true;
            }
            ControlAction::SetDnd(on) => {
                self.set_dnd(on);
                std::thread::spawn(move || status::set_dnd(on));
            }
            ControlAction::SetVolume(v) => {
                self.change_volume(d, v)?;
                self.popup_dirty = true;
            }
            ControlAction::SetBrightness(percent) => {
                if let Some(b) = &mut self.backlight {
                    b.value = b.value_for(percent);
                    self.brightness.set(&b.device, b.value);
                }
                self.popup_dirty = true;
            }
            ControlAction::OpenNetwork => {
                let x = f32::from(self.popup_x);
                self.close_popup(d)?;
                self.open_network(d, Part::Control, x)?;
            }
            ControlAction::OpenBluetooth => {
                // Окошко устройств — на месте Пункта управления.
                let x = f32::from(self.popup_x);
                self.close_popup(d)?;
                self.open_bluetooth(d, Part::Control, x)?;
            }
            ControlAction::NetworkSettings => {
                self.close_popup(d)?;
                run(&["nm-connection-editor".to_string()]);
            }
        }
        Ok(())
    }

    /// Окошко сети под частью строки `part` (левый край — `x`). Сразу — что
    /// известно, NetworkManager тем временем ищет сети заново.
    fn open_network(&mut self, d: &Display, part: Part, x: f32) -> Result<()> {
        let mut state = self.net_state.clone().unwrap_or_default();
        state.scanning = true;
        let _ = self.net.send(NetCmd::Scan);
        let mut panel = NetworkPanel::new(state);
        panel.set_language(self.language);
        let size = panel.size();
        self.network_panel = Some(panel);
        self.show_popup(d, part, x, size)
    }

    fn network_action(&mut self, d: &Display, action: NetworkAction) -> Result<()> {
        match action {
            NetworkAction::Nothing => {}
            NetworkAction::Redraw => self.popup_dirty = true,
            NetworkAction::Close => self.close_popup(d)?,
            NetworkAction::Wifi(on) => {
                let _ = self.net.send(NetCmd::Wifi(on));
                self.popup_dirty = true;
            }
            NetworkAction::Connect(ssid, known) => {
                let _ = self.net.send(NetCmd::Connect(ssid, known));
                self.popup_dirty = true;
            }
            NetworkAction::Settings => {
                self.close_popup(d)?;
                run(&["nm-connection-editor".to_string()]);
            }
        }
        Ok(())
    }

    /// Окошко устройств Bluetooth под частью строки `part` (левый край — `x`).
    fn open_bluetooth(&mut self, d: &Display, part: Part, x: f32) -> Result<()> {
        let Some(state) = self.bt_state.clone() else {
            return Ok(());
        };
        // Пока окошко открыто, ищем устройства рядом.
        if let Some(bt) = &self.bt {
            bt.send(BtCmd::Scan(true));
        }
        let mut panel = BluetoothPanel::new(state);
        panel.set_language(self.language);
        let size = panel.size();
        self.bluetooth_panel = Some(panel);
        self.show_popup(d, part, x, size)
    }

    fn bluetooth_action(&mut self, d: &Display, action: BluetoothAction) -> Result<()> {
        let send = |bt: &Option<Bluetooth>, cmd: BtCmd| {
            if let Some(bt) = bt {
                bt.send(cmd);
            }
        };
        match action {
            BluetoothAction::Nothing => {}
            BluetoothAction::Redraw => self.popup_dirty = true,
            BluetoothAction::Close => self.close_popup(d)?,
            BluetoothAction::Power(on) => {
                send(&self.bt, BtCmd::Power(on));
                if on {
                    send(&self.bt, BtCmd::Scan(true));
                }
                self.popup_dirty = true;
            }
            BluetoothAction::Connect(path) => {
                send(&self.bt, BtCmd::Connect(path));
                self.popup_dirty = true;
            }
            BluetoothAction::Disconnect(path) => {
                send(&self.bt, BtCmd::Disconnect(path));
                self.popup_dirty = true;
            }
            BluetoothAction::Settings => {
                self.close_popup(d)?;
                run(&["blueman-manager".to_string()]);
            }
        }
        Ok(())
    }

    fn open_sound(&mut self, d: &Display, x: f32) -> Result<()> {
        // Показываем последнее состояние сразу, обновляем в фоне.
        let _ = self.audio.send(Audio::Refresh);
        let Some(sound) = self.sound.clone() else {
            return Ok(());
        };
        let mut panel = SoundPanel::new(sound);
        panel.set_language(self.language);
        let size = panel.size();
        self.sound_panel = Some(panel);
        self.show_popup(d, Part::Volume, x, size)
    }

    /// Щелчок по значку трея `i` (левый край — `x`) кнопкой `button`.
    fn tray_click(&mut self, i: usize, x: f32, button: u8) -> Result<()> {
        let (Some(tray), Some(item)) = (&self.tray, self.tray_items.get(i)) else {
            return Ok(());
        };
        let key = item.key.clone();
        // Программе — точка экрана под значком (там она может показать своё окно).
        let (sx, sy) = ((x + 17.0) as i32, self.bar.height as i32);
        let wants_menu = match button {
            1 => item.item_is_menu && item.has_menu,
            3 => item.has_menu,
            _ => false,
        };
        if wants_menu {
            // Меню придёт ответом от программы (TrayUpdate::Menu).
            tray.send(TrayCmd::Menu(key.clone()));
            self.tray_menu_wait = Some((key, x));
            self.bar.open = Some(Part::Tray(i));
            self.dirty = true;
            return Ok(());
        }
        match button {
            1 => {
                // Нет у значка действия — трей пришлёт его меню.
                if item.has_menu {
                    self.tray_menu_wait = Some((key.clone(), x));
                }
                tray.send(TrayCmd::Activate(key, sx, sy));
            }
            2 => tray.send(TrayCmd::SecondaryActivate(key, sx, sy)),
            4 => tray.send(TrayCmd::Scroll(key, -1)),
            5 => tray.send(TrayCmd::Scroll(key, 1)),
            _ => {}
        }
        Ok(())
    }

    /// Прочитать меню активного окна (`open` — какое из них затем открыть).
    fn load_app_menu(&mut self, d: &Display, open: Option<usize>) {
        let active = d.active_window();
        if active != self.app_window {
            // Меню прежнего окна сразу убираем: новое придёт чуть позже.
            self.app_window = active;
            self.app_menus.clear();
            self.bar.menus.clear();
            self.dirty = true;
        }
        let (Some(appmenu), Some(w)) = (&self.appmenu, active) else {
            return;
        };
        // Рабочий стол без меню.
        if d.window_info(w).is_some_and(|i| is_desktop(&i)) {
            return;
        }
        let text = |name: &str| d.window_menu_property(w, name);
        let source = match (
            text("_GTK_UNIQUE_BUS_NAME"),
            text("_GTK_MENUBAR_OBJECT_PATH"),
        ) {
            (Some(bus), Some(menubar)) => Source::Gtk(GtkPaths {
                bus,
                menubar,
                app: text("_GTK_APPLICATION_OBJECT_PATH"),
                window: text("_GTK_WINDOW_OBJECT_PATH"),
                unity: text("_UNITY_OBJECT_PATH"),
            }),
            _ => Source::Registrar,
        };
        appmenu.send(AppMenuCmd::Load {
            window: w,
            source,
            open,
        });
    }

    /// Пришло меню окна (и, может быть, его нужно открыть).
    pub fn app_menu_update(
        &mut self,
        d: &Display,
        fonts: &mut Fonts,
        update: AppMenuUpdate,
    ) -> Result<()> {
        let AppMenuUpdate::Menu {
            window,
            menus,
            open,
        } = update;
        if Some(window) != self.app_window {
            return Ok(());
        }
        self.bar.menus = menus.iter().map(|(t, _)| t.clone()).collect();
        self.app_menus = menus;
        self.dirty = true;
        if let (Some(i), Some((wait, x))) = (open, self.app_menu_wait)
            && i == wait
        {
            self.app_menu_wait = None;
            self.bar.open = None;
            self.open_global(d, fonts, i, x)?;
        }
        Ok(())
    }

    /// Открыть меню программы `i` (из прочитанного) под его названием.
    fn open_global(&mut self, d: &Display, fonts: &mut Fonts, i: usize, x: f32) -> Result<()> {
        let Some((_, items)) = self.app_menus.get(i) else {
            return Ok(());
        };
        let mut entries = Vec::new();
        flatten(items, false, Cmd::Global, &mut entries);
        if entries.is_empty() {
            self.dirty = true;
            return Ok(());
        }
        self.open_menu(d, fonts, Part::Menu(i), "global".into(), x, entries)
    }

    /// Меню названия программы.
    fn open_app(&mut self, d: &Display, fonts: &mut Fonts, x: f32) -> Result<()> {
        if self.app_window.is_none() {
            return Ok(());
        }
        let entries = vec![
            entry(
                mimshel_core::i18n::t(self.language, "menu.minimize"),
                Cmd::Minimize,
                false,
                false,
            ),
            entry(
                mimshel_core::i18n::t(self.language, "term.close_window"),
                Cmd::CloseWindow,
                false,
                false,
            ),
        ];
        self.open_menu(d, fonts, Part::App, "app".into(), x, entries)
    }

    /// Пришли значки трея или меню значка.
    pub fn tray_update(
        &mut self,
        d: &Display,
        fonts: &mut Fonts,
        icons: &mut Icons,
        update: TrayUpdate,
    ) -> Result<()> {
        match update {
            TrayUpdate::Items(items) => {
                // «Passive» — программа просит значок не показывать.
                self.tray_items = items
                    .into_iter()
                    .filter(|i| i.status != "Passive")
                    .collect();
                self.bar.tray = self
                    .tray_items
                    .iter()
                    .map(|i| tray_icon(i, icons))
                    .collect();
                self.refresh_hidden();
                self.dirty = true;
            }
            TrayUpdate::Menu { key, items } => {
                let Some((wait, x)) = self.tray_menu_wait.take() else {
                    return Ok(());
                };
                if wait != key {
                    return Ok(());
                }
                let index = self.tray_items.iter().position(|i| i.key == key);
                let part = Part::Tray(index.unwrap_or(0));
                self.bar.open = None;
                let mut entries = Vec::new();
                flatten(&items, false, Cmd::Tray, &mut entries);
                if entries.is_empty() {
                    self.dirty = true;
                    return Ok(());
                }
                self.open_menu(d, fonts, part, key, x, entries)?;
            }
        }
        Ok(())
    }

    /// Закрыть меню или окошко громкости (если открыто): например, перед
    /// открытием «Поиска».
    pub fn close_popup(&mut self, d: &Display) -> Result<()> {
        if self.bluetooth_panel.is_some()
            && let Some(bt) = &self.bt
        {
            bt.send(BtCmd::Scan(false));
        }
        if self.menu.take().is_some()
            | self.sound_panel.take().is_some()
            | self.control_panel.take().is_some()
            | self.calendar.take().is_some()
            | self.bluetooth_panel.take().is_some()
            | self.network_panel.take().is_some()
        {
            self.popup.hide(d)?;
            self.bar.open = None;
            self.dirty = true;
        }
        Ok(())
    }

    fn next_layout(&mut self, d: &Display) -> Result<()> {
        if self.layouts.len() > 1 {
            let next = (usize::from(self.group) + 1) % self.layouts.len();
            d.set_keyboard_group(next as u8)?;
        }
        Ok(())
    }

    fn read_layouts(&mut self, d: &Display) {
        self.layouts = d.keyboard_layouts();
        self.group = d.keyboard_group();
        self.update_layout_label();
    }

    fn update_layout_label(&mut self) {
        let label = (self.layouts.len() > 1)
            .then(|| self.layouts.get(usize::from(self.group)))
            .flatten()
            .map(|code| layout_label(code));
        if label != self.bar.layout {
            self.bar.layout = label;
            self.dirty = true;
        }
    }

    /// Название активной программы. Активен рабочий стол или ничего — как на
    /// Mac, «Проводник» (Finder).
    fn app_name(&self, d: &Display) -> String {
        let desktop = || {
            self.apps
                .iter()
                .find(|a| a.id == "thunar.desktop")
                .map_or_else(
                    || mimshel_core::i18n::t(self.language, "app.file_manager").to_string(),
                    |a| a.name.clone(),
                )
        };
        let Some(info) = d.active_window().and_then(|w| d.window_info(w)) else {
            return desktop();
        };
        if is_desktop(&info) {
            return desktop();
        }
        let hint = info
            .pid
            .and_then(mimshel_apps::process::launched_from)
            .map(|p| p.to_string_lossy().into_owned());
        match mimshel_dock::match_window(&info, hint.as_deref(), &self.apps) {
            Some(i) => self.apps[i].name.clone(),
            None if !info.class.is_empty() => info.class.clone(),
            None => info.title.clone(),
        }
    }

    /// Where the popup's top-left corner goes on the screen.
    fn popup_origin(&self) -> (i16, i16) {
        let (ax, ay, _, _) = self.area;
        (
            (ax + i32::from(self.popup_x)) as i16,
            (ay + self.bar.height as i32) as i16,
        )
    }

    fn place(&mut self, d: &Display) -> Result<()> {
        self.area = d.primary_area();
        let (ax, ay, aw, _) = self.area;
        self.bar.width = aw;
        let h = self.bar.height;
        self.window.set_geometry(d, ax, ay, self.bar.width, h)?;
        self.window.set_input(d, &[(0, 0, self.bar.width, h)])?;
        // The top strut counts from the top of the whole screen and covers
        // only this monitor's columns.
        let (x0, top) = (ax.max(0) as u32, ay.max(0) as u32 + h);
        self.window
            .reserve_top(d, top, x0, x0 + self.bar.width.saturating_sub(1))?;
        self.dirty = true;
        Ok(())
    }

    /// Перерисовать, если что-то изменилось.
    pub fn flush(&mut self, d: &Display, fonts: &mut Fonts, icons: &mut Icons) -> Result<()> {
        if self.drag_dirty {
            self.drag_dirty = false;
            if let Some(drag) = &self.icon_drag
                && let Some(look) = &drag.active
            {
                let image = drag_image(look, fonts, icons);
                let half = (DRAG_SIZE / 2) as i32;
                self.drag_popup.show_passive(
                    d,
                    (drag.at.0 - half) as i16,
                    (drag.at.1 - half) as i16,
                    DRAG_SIZE as u16,
                    DRAG_SIZE as u16,
                )?;
                self.drag_popup.present(d, &image)?;
            }
        }
        if self.dirty {
            self.dirty = false;
            self.window.present(d, &self.bar.render(fonts, icons))?;
        }
        if self.popup_dirty {
            self.popup_dirty = false;
            if let Some(menu) = &self.menu {
                self.popup.present(d, &menu.render(fonts))?;
            } else if let Some(panel) = &self.sound_panel {
                let (w, h) = panel.size();
                let (px, py) = self.popup_origin();
                self.popup.set_geometry(d, px, py, w as u16, h as u16)?;
                self.popup.present(d, &panel.render(fonts))?;
            } else if let Some(panel) = &self.control_panel {
                let (w, h) = panel.size();
                let (px, py) = self.popup_origin();
                self.popup.set_geometry(d, px, py, w as u16, h as u16)?;
                self.popup.present(d, &panel.render(fonts, icons))?;
            } else if let Some(panel) = &self.calendar {
                self.popup.present(d, &panel.render(fonts))?;
            } else if let Some(panel) = &self.network_panel {
                let (w, h) = panel.size();
                let (px, py) = self.popup_origin();
                self.popup.set_geometry(d, px, py, w as u16, h as u16)?;
                self.popup.present(d, &panel.render(fonts))?;
            } else if let Some(panel) = &self.bluetooth_panel {
                // Список растёт по мере поиска — окно под размер.
                let (w, h) = panel.size();
                let (px, py) = self.popup_origin();
                self.popup.set_geometry(d, px, py, w as u16, h as u16)?;
                self.popup.present(d, &panel.render(fonts))?;
            }
        }
        Ok(())
    }
}

/// Пункты меню DBusMenu → пункты нашего меню (`cmd` — что делает пункт с
/// таким id). Подменю показываем внутри: его название — серым
/// заголовком, пункты — с отступом.
fn flatten(items: &[MenuItem], indent: bool, cmd: fn(i32) -> Cmd, out: &mut Vec<MenuEntry<Cmd>>) {
    let mut separator = false;
    for item in items.iter().filter(|i| i.visible) {
        if item.separator {
            separator = !out.is_empty();
            continue;
        }
        let submenu = !item.children.is_empty();
        out.push(MenuEntry {
            label: item.label.clone(),
            command: cmd(item.id),
            checked: item.toggle == Some(true),
            separator_before: separator,
            enabled: item.enabled && !submenu,
            indent,
        });
        separator = false;
        if submenu {
            flatten(&item.children, true, cmd, out);
        }
    }
}

/// Картинка значка трея: иконка из темы по имени или готовая картинка
/// программы (ARGB, «старший байт первым» → RGBA с умножением на прозрачность).
fn tray_icon(item: &TrayItem, icons: &mut Icons) -> tiny_skia::Pixmap {
    if !item.icon_name.is_empty()
        && let Some(icon) = icons.get(&item.icon_name, 16)
    {
        return icon.clone();
    }
    if let Some((w, h, argb)) = &item.pixmap {
        let mut rgba = Vec::with_capacity(argb.len());
        for px in argb.as_chunks::<4>().0 {
            let [a, r, g, b] = *px;
            let m = |c: u8| (u16::from(c) * u16::from(a) / 255) as u8;
            rgba.extend_from_slice(&[m(r), m(g), m(b), a]);
        }
        if let Some(size) = tiny_skia::IntSize::from_wh(*w, *h)
            && let Some(pm) = tiny_skia::Pixmap::from_vec(rgba, size)
        {
            return pm;
        }
    }
    icons
        .get("image-missing", 16)
        .cloned()
        .unwrap_or_else(|| tiny_skia::Pixmap::new(16, 16).expect("размер не нулевой"))
}

fn entry(label: &str, command: Cmd, checked: bool, separator_before: bool) -> MenuEntry<Cmd> {
    MenuEntry {
        label: label.to_string(),
        command,
        checked,
        separator_before,
        enabled: true,
        indent: false,
    }
}

/// Write `[menubar] hidden`; the rest of the file stays as it is.
fn save_hidden(path: &std::path::Path, ids: &[String]) -> std::io::Result<()> {
    crate::ui::config_store(path.to_path_buf()).edit(|text| {
        mimshel_core::config::set_menubar_hidden(text, ids).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "в файле ошибка, исправьте её — тогда значки сохранятся",
            )
        })
    })
}

/// Файл главного меню (его пишет `mimshel apply launchers`).
fn apple_menu_file() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_default();
    mimshel_core::names::dir_in(&data).join("apple.menu")
}

/// Окно рабочего стола: свой (класс `desktop.Shell`) или xfdesktop.
fn is_desktop(info: &mimshel_core::windows::WindowInfo) -> bool {
    info.class.eq_ignore_ascii_case("xfdesktop")
        || ((info.class == mimshel_core::names::CLASS
            || info.class == mimshel_core::names::OLD_CLASS)
            && info.instance == "desktop")
}

/// Запустить команду пункта меню отдельно от службы.
fn run(command: &[String]) {
    let Some((program, args)) = command.split_first() else {
        return;
    };
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Err(e) = crate::ui::spawn_detached(cmd) {
        eprintln!("Не удалось выполнить «{}»: {e}", command.join(" "));
    }
}

/// С большой буквы («октябрь» → «Октябрь»).
fn capitalize(s: &str) -> String {
    let mut c = s.trim().chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Month and weekday names for the calendar in the interface language.
fn calendar_names(
    lang: mimshel_core::i18n::Language,
    monday_first: bool,
) -> mimshel_menubar::Names {
    use mimshel_core::dates::{Month, month, weekday};
    mimshel_menubar::Names {
        months: std::array::from_fn(|i| month(lang, i as u32 + 1, Month::Alone).to_string()),
        weekdays: std::array::from_fn(|i| weekday(lang, i as u32 + 1, false).to_string()),
        monday_first,
    }
}

/// Whether the system's week starts on Monday (`locale first_weekday`: 1 —
/// Sunday, 2 — Monday).
fn system_monday_first() -> bool {
    Command::new("locale")
        .arg("first_weekday")
        .output()
        .map_or(true, |out| {
            String::from_utf8_lossy(&out.stdout).trim() != "1"
        })
}

/// Сегодня: дата и она же словами («Пятница, 23 октября»).
fn parse_today(nums: &str, text: &str) -> Today {
    let n: Vec<u32> = nums.split(' ').filter_map(|v| v.parse().ok()).collect();
    let today = match n[..] {
        [y, m, d] => mimshel_menubar::Date {
            year: y as i32,
            month: m,
            day: d,
        },
        _ => mimshel_menubar::Date {
            year: 1970,
            month: 1,
            day: 1,
        },
    };
    (today, capitalize(text))
}

/// One `date` run: the clock text, seconds into the minute, and today for
/// the calendar (so opening it runs nothing on the main thread). Weekday
/// and month names are in `lang` (`mimshel_core::dates`).
fn clock(format: &str, lang: mimshel_core::i18n::Language) -> (String, u64, Today) {
    use mimshel_core::dates;
    let words = dates::today_format(lang);
    let out = Command::new("date")
        .arg(dates::prepare(&format!("+%S|%Y %m %d|{words}|{format}")))
        .stdin(Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    parse_clock(&dates::fill(out.trim_end(), lang))
}

/// What the calendar shows as today: the date and the same in words.
pub type Today = (mimshel_menubar::Date, String);

/// `SS|YYYY MM DD|weekday, D month|clock text` (the clock format may hold
/// `|` itself, so it goes last).
fn parse_clock(out: &str) -> (String, u64, Today) {
    let mut parts = out.splitn(4, '|');
    let secs = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let nums = parts.next().unwrap_or("");
    let words = parts.next().unwrap_or("");
    let text = parts.next().unwrap_or("").to_string();
    (text, secs, parse_today(nums, words))
}

/// Обновлять часы в начале каждой минуты.
fn watch_clock(format: String, lang: Arc<Mutex<mimshel_core::i18n::Language>>, tx: Sender<Msg>) {
    let lang = move || *lang.lock().unwrap_or_else(|p| p.into_inner());
    std::thread::spawn(move || {
        // Один запуск `date` в минуту: предыдущий ответ сообщает, сколько
        // секунд прошло в текущей минуте, — по ним и спим до границы.
        // `sleep` does not count time spent suspended, so it sleeps at most
        // CLOCK_STEP at a time and checks the wall-clock minute: after a
        // resume the clock is right within seconds, not up to a minute late.
        let (_, mut secs, _) = clock(&format, lang());
        let mut shown = wall_minute();
        loop {
            let left = Duration::from_secs(60 - secs.min(59)) + Duration::from_millis(50);
            let started = std::time::Instant::now();
            while started.elapsed() < left && wall_minute() == shown {
                std::thread::sleep(CLOCK_STEP.min(left.saturating_sub(started.elapsed())));
            }
            let (text, s, today) = clock(&format, lang());
            secs = s;
            shown = wall_minute();
            if tx.send(Msg::Clock(text, today)).is_err() {
                return;
            }
        }
    });
}

/// The longest nap of the clock thread between wall-clock checks.
const CLOCK_STEP: Duration = Duration::from_secs(5);

/// Minutes since the epoch by the wall clock (time zones shift by whole
/// minutes, so its change is the change of the shown minute).
fn wall_minute() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0)
}

/// Значок Bluetooth от blueman в трее (`Id = blueman`, путь `/org/blueman/sni`).
fn is_bluetooth_tray(item: &TrayItem) -> bool {
    item.key.ends_with("/org/blueman/sni") || item.title == "blueman"
}

/// Значок сети от nm-applet в трее (`--indicator`: путь `…/nm_applet`).
fn is_network_tray(item: &TrayItem) -> bool {
    item.key.ends_with("/nm_applet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_date_run_gives_clock_and_today() {
        let (text, secs, (today, words)) =
            parse_clock("07|2026 10 28|среда, 28 октября|Ср 28 окт | 19:05");
        assert_eq!(text, "Ср 28 окт | 19:05", "the clock format may hold |");
        assert_eq!(secs, 7);
        assert_eq!(
            today,
            mimshel_menubar::Date {
                year: 2026,
                month: 10,
                day: 28
            }
        );
        assert_eq!(words, "Среда, 28 октября");
        // `date` did not run: an empty clock, not a panic.
        let (text, secs, (today, _)) = parse_clock("");
        assert_eq!((text.as_str(), secs, today.year), ("", 0, 1970));
    }
}
