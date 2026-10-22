//! Строка меню на экране: окно-полоса во всю ширину у верхнего края (с
//! резервом места), меню яблока, название активной программы, раскладка
//! (щелчок — следующая), громкость (окошко с ползунком, колёсико), батарея,
//! колокольчик («Не беспокоить») и часы.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use shell_apps::App;
use shell_core::config::Config;
use shell_core::keyboard::layout_label;
use shell_icons::Icons;
use shell_menubar::status::{Battery, Sound};
use shell_menubar::{AppleItem, Bar, Part, SoundAction, SoundPanel, apple_menu};
use shell_text::Fonts;
use shell_widgets::{Menu, MenuEntry, MenuResult};
use shell_x11::{Display, DockWindow, Event, Popup, Property};

use crate::status::{self, Audio};
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
    dnd: bool,
    audio: Sender<Audio>,
    /// Когда громкость меняли отсюда.
    volume_changed: Option<Instant>,
    /// Открытое меню (яблока, батареи, колокольчика) или окошко громкости —
    /// в одном всплывающем окне; `popup_dirty` — перерисовать его.
    menu: Option<Menu<Cmd>>,
    sound_panel: Option<SoundPanel>,
    popup: Popup,
    popup_dirty: bool,
    dirty: bool,
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
            dnd: false,
            audio: status::audio_worker(),
            volume_changed: None,
            menu: None,
            sound_panel: None,
            popup: d.create_popup("menubar-menu")?,
            popup_dirty: false,
            dirty: true,
        };
        d.watch_root()?;
        d.watch_keyboard()?;
        ui.read_layouts(d);
        ui.bar.app_name = ui.app_name(d);
        ui.bar.clock = clock(&config.clock_format).0;
        ui.set_sound(status::read_sound());
        ui.set_battery(status::read_battery());
        ui.set_dnd(status::read_dnd());
        watch_clock(config.clock_format.clone(), tx.clone());
        status::watch_sound(tx.clone());
        status::watch_battery(tx.clone());
        status::watch_dnd(tx);
        ui.place(d)?;
        ui.window.show(d)?;
        Ok(ui)
    }

    pub fn set_apps(&mut self, d: &Display, apps: Vec<App>) {
        self.apps = apps;
        self.bar.app_name = self.app_name(d);
        self.dirty = true;
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
        }
        self.bar.volume = sound.as_ref().map(|s| s.icon().to_string());
        // Пока тащат ползунок, он главнее: ответ звуковой системы отстаёт.
        if let (Some(panel), Some(s)) = (&mut self.sound_panel, &sound)
            && !panel.dragging()
        {
            panel.sound = s.clone();
            self.popup_dirty = true;
        }
        self.sound = sound;
        self.dirty = true;
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
        self.dirty = true;
    }

    fn popup_open(&self) -> bool {
        self.menu.is_some() || self.sound_panel.is_some()
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
            }
            Event::Button { window, x, y, .. } if window == popup => {
                let (x, y) = (f32::from(x), f32::from(y));
                if let Some(menu) = &self.menu {
                    let result = menu.click(x, y);
                    self.menu_result(d, result)?;
                } else if let Some(panel) = &mut self.sound_panel {
                    let action = panel.press(x, y);
                    self.sound_action(d, action)?;
                }
            }
            Event::Release { window, .. } if window == popup => {
                if let Some(panel) = &mut self.sound_panel {
                    panel.release();
                }
            }
            Event::Key(k) => {
                if let Some(menu) = &mut self.menu {
                    let result = menu.key(k.key);
                    self.menu_result(d, result)?;
                } else if let Some(panel) = &mut self.sound_panel {
                    let action = panel.key(k.key);
                    self.sound_action(d, action)?;
                }
            }
            Event::Expose { window } if window == popup => self.popup_dirty = true,
            Event::Button { x, button, .. } => {
                let part = self.bar.part_at(fonts, f32::from(x));
                if std::env::var_os("SHELL_DEBUG_DOCK").is_some() {
                    eprintln!(
                        "строка меню: кнопка {button} по {part:?}, раскладки {:?}, нынешняя {}",
                        self.layouts, self.group
                    );
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
                    return Ok(());
                }
                // Щелчок при открытом меню: по той же части — закрыть, по
                // другой — открыть её меню, как на Mac.
                let was = self.bar.open;
                if self.popup_open() {
                    self.close_popup(d)?;
                }
                match part {
                    Some((p, _)) if Some(p) == was => {}
                    Some((Part::Apple, px)) => self.open_apple(d, fonts, px)?,
                    Some((Part::Battery, px)) => self.open_battery(d, fonts, px)?,
                    Some((Part::Bell, px)) => self.open_bell(d, fonts, px)?,
                    Some((Part::Volume, px)) => self.open_sound(d, px)?,
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
                }
            }
        }
        Ok(())
    }

    /// Показать всплывающее окно под частью строки (левый край — `x`).
    fn show_popup(&mut self, d: &Display, part: Part, x: f32, (w, h): (u32, u32)) -> Result<()> {
        let x = x.min(self.bar.width as f32 - w as f32 - 4.0).max(4.0);
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
        x: f32,
        entries: Vec<MenuEntry<Cmd>>,
    ) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut menu = Menu::new(format!("{part:?}"), entries);
        let size = menu.measure(fonts);
        self.menu = Some(menu);
        self.show_popup(d, part, x, size)
    }

    fn open_apple(&mut self, d: &Display, fonts: &mut Fonts, x: f32) -> Result<()> {
        // Пункты перечитываем: macos-look мог их поменять.
        self.apple = apple_menu(&apple_menu_file());
        let entries = self
            .apple
            .iter()
            .enumerate()
            .map(|(i, item)| entry(&item.label, Cmd::Apple(i), false, item.separator_before))
            .collect();
        self.open_menu(d, fonts, Part::Apple, x, entries)
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
        self.open_menu(d, fonts, Part::Battery, x, entries)
    }

    fn open_bell(&mut self, d: &Display, fonts: &mut Fonts, x: f32) -> Result<()> {
        let entries = vec![
            entry("Не беспокоить", Cmd::ToggleDnd, self.dnd, false),
            entry("Настройки уведомлений…", Cmd::NotifySettings, false, true),
        ];
        self.open_menu(d, fonts, Part::Bell, x, entries)
    }

    fn open_sound(&mut self, d: &Display, x: f32) -> Result<()> {
        // Свежее состояние: устройства могли появиться или пропасть.
        let Some(sound) = status::read_sound().or_else(|| self.sound.clone()) else {
            return Ok(());
        };
        let panel = SoundPanel::new(sound);
        let size = panel.size();
        self.sound_panel = Some(panel);
        self.show_popup(d, Part::Volume, x, size)
    }

    /// Закрыть меню или окошко громкости (если открыто): например, перед
    /// открытием Spotlight.
    pub fn close_popup(&mut self, d: &Display) -> Result<()> {
        if self.menu.take().is_some() | self.sound_panel.take().is_some() {
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
        if info.class.eq_ignore_ascii_case("xfdesktop") {
            return desktop();
        }
        let hint = info
            .pid
            .and_then(shell_apps::process::launched_from)
            .map(|p| p.to_string_lossy().into_owned());
        match shell_dock::match_window(&info, hint.as_deref(), &self.apps) {
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
                self.popup.present(d, &panel.render(fonts))?;
            }
        }
        Ok(())
    }
}

fn entry(label: &str, command: Cmd, checked: bool, separator_before: bool) -> MenuEntry<Cmd> {
    MenuEntry {
        label: label.to_string(),
        command,
        checked,
        separator_before,
    }
}

/// Файл меню яблока, который пишет macos-look.
fn apple_menu_file() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_default();
    data.join("macos-look/apple.menu")
}

/// Запустить команду пункта меню отдельно от службы.
fn run(command: &[String]) {
    let Some((program, args)) = command.split_first() else {
        return;
    };
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    match cmd.spawn() {
        // Дожидаемся в отдельном потоке, чтобы не копились «зомби».
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => eprintln!("Не удалось выполнить «{}»: {e}", command.join(" ")),
    }
}

/// Часы по формату `date` (на языке сеанса) и сколько секунд прошло в
/// нынешней минуте.
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
        loop {
            let (_, secs) = clock(&format);
            std::thread::sleep(Duration::from_secs(60 - secs.min(59)) + Duration::from_millis(50));
            if tx.send(Msg::Clock(clock(&format).0)).is_err() {
                return;
            }
        }
    });
}
