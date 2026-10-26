//! Строка меню на экране: окно-полоса во всю ширину у верхнего края (с
//! резервом места), меню яблока, название активной программы, раскладка
//! (щелчок — следующая), громкость (окошко с ползунком, колёсико), батарея,
//! колокольчик («Не беспокоить»), трей (StatusNotifierItem), часы и
//! глобальное меню активной программы («Файл», «Правка»…).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, SyncSender};
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
    ControlPanel, ControlState, NetState, NetworkAction, NetworkPanel, Part, SoundAction,
    SoundPanel, apple_menu,
};
use mimshel_text::Fonts;
use mimshel_widgets::{Menu, MenuEntry, MenuResult};
use mimshel_x11::Key;
use mimshel_x11::{Display, DockWindow, Event, Popup, Property};

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

pub struct MenubarUi {
    window: DockWindow,
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
    /// Названия месяцев и дней на языке системы (читаются один раз).
    calendar_names: Option<mimshel_menubar::Names>,
    popup: Popup,
    popup_x: i16,
    popup_dirty: bool,
    dirty: bool,
    /// Служба — для просьб, которые выполняет не строка (центр уведомлений).
    tx: Sender<Msg>,
    language: mimshel_core::i18n::Language,
}

impl MenubarUi {
    pub fn new(d: &Display, config: &Config, apps: Vec<App>, tx: Sender<Msg>) -> Result<MenubarUi> {
        let (sw, _) = d.screen_size();
        let mut ui = MenubarUi {
            window: d.create_dock("menubar")?,
            bar: Bar::new(u32::from(sw), config.menubar_height, &config.menu_icon),
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
            calendar_names: None,
            popup: d.create_popup("menubar-menu")?,
            popup_x: 0,
            popup_dirty: false,
            dirty: true,
            tx: tx.clone(),
            language: config.language,
        };
        d.watch_root()?;
        d.watch_keyboard()?;
        ui.read_layouts(d);
        ui.bar.app_name = ui.app_name(d);
        ui.bar.clock = clock(&config.clock_format).0;
        ui.bar.control = true;
        watch_clock(config.clock_format.clone(), tx.clone());
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

    pub fn set_clock(&mut self, text: String) {
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
                window == d.root() && what == Property::ActiveWindow
            }
            Event::Configure { window } => window == d.root(),
            Event::KeyboardGroup(_) | Event::KeyboardLayouts => true,
            _ => false,
        }
    }

    pub fn event(&mut self, d: &Display, fonts: &mut Fonts, ev: Event) -> Result<()> {
        let popup = self.popup.id();
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
            Event::Button { window, x, y, .. } if window == popup => {
                let (x, y) = (f32::from(x), f32::from(y));
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
            Event::Button { x, button, .. } => {
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
                match part {
                    // Колокольчик — центр уведомлений (его рисует служба
                    // уведомлений; повторный щелчок закрывает его там же).
                    Some((Part::Bell, _)) => {
                        let _ = self.tx.send(Msg::ToggleCenter);
                    }
                    Some((p, _)) if Some(p) == was => {}
                    Some((Part::Clock, px)) => self.open_calendar(d, px)?,
                    Some((Part::Apple, px)) => self.open_apple(d, fonts, px)?,
                    Some((Part::Battery, px)) => self.open_battery(d, fonts, px)?,
                    Some((Part::Volume, px)) => self.open_sound(d, px)?,
                    Some((Part::Control, px)) => self.open_control(d, px)?,
                    // Значок Bluetooth (blueman) — своё окошко устройств,
                    // как из Пункта управления; правая кнопка — меню blueman.
                    Some((Part::Tray(i), px))
                        if self.bt_state.is_some()
                            && self.tray_items.get(i).is_some_and(is_bluetooth_tray) =>
                    {
                        self.open_bluetooth(d, Part::Tray(i), px)?
                    }
                    // Значок сети (nm-applet) — своё окошко сети.
                    Some((Part::Tray(i), px))
                        if self.tray_items.get(i).is_some_and(is_network_tray) =>
                    {
                        self.open_network(d, Part::Tray(i), px)?
                    }
                    Some((Part::Tray(i), px)) => self.tray_click(i, px, 1)?,
                    Some((Part::Menu(i), px)) => {
                        // Читаем меню заново: доступность пунктов могла поменяться.
                        self.app_menu_wait = Some((i, px));
                        self.bar.open = Some(Part::Menu(i));
                        self.dirty = true;
                        self.load_app_menu(d, Some(i));
                    }
                    Some((Part::App, px)) => self.open_app(d, fonts, px)?,
                    Some((Part::Layout, _)) if was.is_none() => self.next_layout(d)?,
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
        self.popup
            .show(d, x as i16, self.bar.height as i16, w as u16, h as u16)?;
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

    fn open_apple(&mut self, d: &Display, fonts: &mut Fonts, x: f32) -> Result<()> {
        // Пункты перечитываем: `mimshel apply launchers` мог их поменять.
        self.apple = apple_menu(&apple_menu_file());
        let entries = self
            .apple
            .iter()
            .enumerate()
            .map(|(i, item)| entry(&item.label, Cmd::Apple(i), false, item.separator_before))
            .collect();
        self.open_menu(d, fonts, Part::Apple, "apple".into(), x, entries)
    }

    fn open_battery(&mut self, d: &Display, fonts: &mut Fonts, x: f32) -> Result<()> {
        let Some(b) = &self.battery else {
            return Ok(());
        };
        let entries = vec![
            entry(
                &format!("Аккумулятор: {} %", b.percent),
                Cmd::Info,
                false,
                false,
            ),
            entry(&b.state_text(), Cmd::Info, false, false),
            entry(
                "Настройки энергосбережения…",
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
        let names = self
            .calendar_names
            .get_or_insert_with(calendar_names)
            .clone();
        let (today, text) = today();
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
        let panel = ControlPanel::new(ControlState {
            network: self.network.clone(),
            bluetooth: self.bluetooth_on(),
            dnd: self.dnd,
            display: self.backlight.as_ref().map(Backlight::percent),
            sound: self.sound.clone(),
        });
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
        let panel = NetworkPanel::new(state);
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
        let panel = BluetoothPanel::new(state);
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
        let panel = SoundPanel::new(sound);
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
        let text = |name: &str| d.window_text(w, name);
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
            entry("Свернуть", Cmd::Minimize, false, false),
            entry("Закрыть окно", Cmd::CloseWindow, false, false),
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
                .map_or_else(|| "Проводник".to_string(), |a| a.name.clone())
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

    fn place(&mut self, d: &Display) -> Result<()> {
        let (sw, _) = d.screen_size();
        self.bar.width = u32::from(sw);
        let h = self.bar.height;
        self.window.set_geometry(d, 0, 0, self.bar.width, h)?;
        self.window.set_input(d, &[(0, 0, self.bar.width, h)])?;
        self.window.set_strut_top(d, h, 0, self.bar.width)?;
        self.dirty = true;
        Ok(())
    }

    /// Перерисовать, если что-то изменилось.
    pub fn flush(&mut self, d: &Display, fonts: &mut Fonts, icons: &mut Icons) -> Result<()> {
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
                self.popup.set_geometry(
                    d,
                    self.popup_x,
                    self.bar.height as i16,
                    w as u16,
                    h as u16,
                )?;
                self.popup.present(d, &panel.render(fonts))?;
            } else if let Some(panel) = &self.control_panel {
                let (w, h) = panel.size();
                self.popup.set_geometry(
                    d,
                    self.popup_x,
                    self.bar.height as i16,
                    w as u16,
                    h as u16,
                )?;
                self.popup.present(d, &panel.render(fonts, icons))?;
            } else if let Some(panel) = &self.calendar {
                self.popup.present(d, &panel.render(fonts))?;
            } else if let Some(panel) = &self.network_panel {
                let (w, h) = panel.size();
                self.popup.set_geometry(
                    d,
                    self.popup_x,
                    self.bar.height as i16,
                    w as u16,
                    h as u16,
                )?;
                self.popup.present(d, &panel.render(fonts))?;
            } else if let Some(panel) = &self.bluetooth_panel {
                // Список растёт по мере поиска — окно под размер.
                let (w, h) = panel.size();
                self.popup.set_geometry(
                    d,
                    self.popup_x,
                    self.bar.height as i16,
                    w as u16,
                    h as u16,
                )?;
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

/// Часы по формату `date` (на языке сеанса) и сколько секунд прошло в
/// нынешней минуте.
/// Вывод `date` с языком сеанса (`input` — строки для `-f -`).
fn date(args: &[&str], input: &str) -> String {
    use std::io::Write;
    let child = Command::new("date")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return String::new();
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    child
        .wait_with_output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default()
}

/// С большой буквы («октябрь» → «Октябрь»).
fn capitalize(s: &str) -> String {
    let mut c = s.trim().chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Названия месяцев и дней недели на языке системы; не вышло — русские.
fn calendar_names() -> mimshel_menubar::Names {
    let mut names = mimshel_menubar::Names::default();
    let months: Vec<String> = date(
        &["-f", "-", "+%OB"],
        &(1..=12)
            .map(|m| format!("2026-{m:02}-01\n"))
            .collect::<String>(),
    )
    .lines()
    .map(capitalize)
    .collect();
    if let Ok(m) = <[String; 12]>::try_from(months) {
        names.months = m;
    }
    // 7.12.2026 — понедельник.
    let days: Vec<String> = date(
        &["-f", "-", "+%a"],
        &(7..=13)
            .map(|d| format!("2026-12-{d:02}\n"))
            .collect::<String>(),
    )
    .lines()
    .map(capitalize)
    .collect();
    if let Ok(d) = <[String; 7]>::try_from(days) {
        names.weekdays = d;
    }
    // locale: 1 — неделя с воскресенья, 2 — с понедельника.
    if let Ok(out) = Command::new("locale").arg("first_weekday").output() {
        names.monday_first = String::from_utf8_lossy(&out.stdout).trim() != "1";
    }
    names
}

/// Сегодня: дата и она же словами («Пятница, 23 октября»).
fn today() -> (mimshel_menubar::Date, String) {
    let out = date(&["+%Y %m %d|%A, %-d %B"], "");
    let (nums, text) = out.trim().split_once('|').unwrap_or(("", ""));
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

fn clock(format: &str) -> (String, u64) {
    let out = Command::new("date")
        .arg(format!("+%S|{format}"))
        .stdin(Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let (secs, text) = out.trim_end().split_once('|').unwrap_or(("0", ""));
    (text.to_string(), secs.parse().unwrap_or(0))
}

/// Обновлять часы в начале каждой минуты.
fn watch_clock(format: String, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        // Один запуск `date` в минуту: предыдущий ответ сообщает, сколько
        // секунд прошло в текущей минуте, — по ним и спим до границы.
        let (_, mut secs) = clock(&format);
        loop {
            std::thread::sleep(Duration::from_secs(60 - secs.min(59)) + Duration::from_millis(50));
            let (text, s) = clock(&format);
            secs = s;
            if tx.send(Msg::Clock(text)).is_err() {
                return;
            }
        }
    });
}

/// Значок Bluetooth от blueman в трее (`Id = blueman`, путь `/org/blueman/sni`).
fn is_bluetooth_tray(item: &TrayItem) -> bool {
    item.key.ends_with("/org/blueman/sni") || item.title == "blueman"
}

/// Значок сети от nm-applet в трее (`--indicator`: путь `…/nm_applet`).
fn is_network_tray(item: &TrayItem) -> bool {
    item.key.ends_with("/nm_applet")
}
