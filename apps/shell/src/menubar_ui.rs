//! Строка меню на экране: окно-полоса во всю ширину у верхнего края (с
//! резервом места), меню яблока, название активной программы, раскладка
//! (щелчок — следующая) и часы.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Duration;

use shell_apps::App;
use shell_core::config::Config;
use shell_core::keyboard::layout_label;
use shell_icons::Icons;
use shell_menubar::{AppleItem, Bar, Part, apple_menu};
use shell_text::Fonts;
use shell_widgets::{Menu, MenuEntry, MenuResult};
use shell_x11::{Display, DockWindow, Event, Popup, Property};

use crate::ui::{Msg, Result};

pub struct MenubarUi {
    window: DockWindow,
    bar: Bar,
    /// Все ярлыки — узнавать программу по окну.
    apps: Vec<App>,
    /// Раскладки по порядку групп (`["us", "ru"]`) и нынешняя.
    layouts: Vec<String>,
    group: u8,
    /// Меню яблока (пункт — номер в `apple`), его окно; `menu_dirty` — перерисовать.
    apple: Vec<AppleItem>,
    menu: Option<Menu<usize>>,
    menu_popup: Popup,
    menu_dirty: bool,
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
            menu: None,
            menu_popup: d.create_popup("menubar-menu")?,
            menu_dirty: false,
            dirty: true,
        };
        d.watch_root()?;
        d.watch_keyboard()?;
        ui.read_layouts(d);
        ui.bar.app_name = ui.app_name(d);
        ui.bar.clock = clock(&config.clock_format).0;
        watch_clock(config.clock_format.clone(), tx);
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

    /// Это событие для строки меню?
    pub fn wants(&self, d: &Display, ev: &Event) -> bool {
        match *ev {
            Event::Button { window, .. }
            | Event::Motion { window, .. }
            | Event::Expose { window } => {
                window == self.window.id() || window == self.menu_popup.id()
            }
            Event::Key(_) => self.menu.is_some(),
            Event::Property { window, what } => {
                window == d.root() && what == Property::ActiveWindow
            }
            Event::Configure { window } => window == d.root(),
            Event::KeyboardGroup(_) | Event::KeyboardLayouts => true,
            _ => false,
        }
    }

    pub fn event(&mut self, d: &Display, fonts: &mut Fonts, ev: Event) -> Result<()> {
        let popup = self.menu_popup.id();
        match ev {
            // Меню открыто: его мышь и клавиатура.
            Event::Motion { window, x, y } if window == popup => {
                if let Some(menu) = &mut self.menu
                    && menu.motion(f32::from(x), f32::from(y))
                {
                    self.menu_dirty = true;
                }
            }
            Event::Button { window, x, y, .. } if window == popup => {
                let result = match &self.menu {
                    Some(menu) => menu.click(f32::from(x), f32::from(y)),
                    None => MenuResult::Nothing,
                };
                self.menu_result(d, result)?;
            }
            Event::Key(k) => {
                let result = match &mut self.menu {
                    Some(menu) => menu.key(k.key),
                    None => MenuResult::Nothing,
                };
                self.menu_result(d, result)?;
            }
            Event::Expose { window } if window == popup => self.menu_dirty = true,
            Event::Button { x, button: 1, .. } => {
                let part = self.bar.part_at(fonts, f32::from(x));
                if std::env::var_os("SHELL_DEBUG_DOCK").is_some() {
                    eprintln!(
                        "строка меню: щелчок по {part:?}, раскладки {:?}, нынешняя {}",
                        self.layouts, self.group
                    );
                }
                // Щелчок при открытом меню (по яблоку снова или мимо) — закрыть.
                if self.menu.is_some() {
                    self.close_menu(d)?;
                    return Ok(());
                }
                match part {
                    Some((Part::Apple, px)) => self.open_apple(d, fonts, px)?,
                    Some((Part::Layout, _)) => self.next_layout(d)?,
                    _ => {}
                }
            }
            Event::Button { .. } => {
                if self.menu.is_some() {
                    self.close_menu(d)?;
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

    fn menu_result(&mut self, d: &Display, result: MenuResult<usize>) -> Result<()> {
        match result {
            MenuResult::Nothing => {}
            MenuResult::Redraw => self.menu_dirty = true,
            MenuResult::Close => self.close_menu(d)?,
            MenuResult::Run(i) => {
                self.close_menu(d)?;
                if let Some(item) = self.apple.get(i) {
                    run(&item.command);
                }
            }
        }
        Ok(())
    }

    fn open_apple(&mut self, d: &Display, fonts: &mut Fonts, x: f32) -> Result<()> {
        // Пункты перечитываем: macos-look мог их поменять.
        self.apple = apple_menu(&apple_menu_file());
        if self.apple.is_empty() {
            return Ok(());
        }
        let entries = self
            .apple
            .iter()
            .enumerate()
            .map(|(i, item)| MenuEntry {
                label: item.label.clone(),
                command: i,
                checked: false,
                separator_before: item.separator_before,
            })
            .collect();
        let mut menu = Menu::new("apple".into(), entries);
        let (w, h) = menu.measure(fonts);
        // Под строкой меню, вровень с левым краем подсветки яблока.
        self.menu_popup
            .show(d, x as i16, self.bar.height as i16, w as u16, h as u16)?;
        self.menu = Some(menu);
        self.menu_dirty = true;
        self.bar.open = Some(Part::Apple);
        self.dirty = true;
        Ok(())
    }

    /// Закрыть меню (если открыто): например, перед открытием Spotlight.
    pub fn close_menu(&mut self, d: &Display) -> Result<()> {
        if self.menu.take().is_some() {
            self.menu_popup.hide(d)?;
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
        if self.menu_dirty {
            self.menu_dirty = false;
            if let Some(menu) = &self.menu {
                self.menu_popup.present(d, &menu.render(fonts))?;
            }
        }
        Ok(())
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
