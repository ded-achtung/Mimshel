//! Рабочий стол (вместо xfdesktop): окно под всеми окнами с обоями на
//! каждом мониторе и меню по правому щелчку — сменить обои, открыть папку
//! «Рабочий стол», терминал, настройки экрана и системы.
//!
//! Обои рисуются в отдельном потоке (свои обои под новый размер — секунды) и
//! приходят сообщением [`Msg::Wallpaper`]; пока их нет, стол ровного цвета.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

use shell_core::config::Config;
use shell_icons::Fit;
use shell_text::Fonts;
use shell_widgets::{Menu, MenuEntry, MenuResult};
use shell_x11::{DesktopWindow, Display, Event, Popup};
use tiny_skia::{Pixmap, PixmapPaint, Transform};

use crate::ui::{Msg, Result};

/// Пункты меню стола.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DesktopCommand {
    /// Свои обои по имени (`space`, `waves`, `aurora`).
    Wallpaper(&'static str),
    /// Выбрать картинку для обоев.
    PickWallpaper,
    OpenDesktop,
    Terminal,
    DisplaySettings,
    Settings,
}

/// Свои обои: (имя в настройках, подпись).
const WALLPAPERS: &[(&str, &str)] = &[
    ("space", "Космос"),
    ("waves", "Волны"),
    ("aurora", "Сияние"),
];

pub struct DesktopUi {
    window: DesktopWindow,
    menu_popup: Popup,
    menu: Option<Menu<DesktopCommand>>,
    menu_dirty: bool,
    wallpaper: String,
    terminal: Vec<String>,
    tx: Sender<Msg>,
}

impl DesktopUi {
    pub fn new(d: &Display, config: &Config, tx: Sender<Msg>) -> Result<DesktopUi> {
        let window = d.create_desktop()?;
        window.show(d)?;
        let ui = DesktopUi {
            window,
            menu_popup: d.create_popup("desktop-menu")?,
            menu: None,
            menu_dirty: false,
            wallpaper: config.wallpaper.clone(),
            terminal: config.terminal.clone(),
            tx,
        };
        ui.load_wallpaper(d);
        Ok(ui)
    }

    /// Нарисовать обои под нынешние мониторы (в отдельном потоке).
    fn load_wallpaper(&self, d: &Display) {
        let (sw, sh) = d.screen_pixels();
        let value = self.wallpaper.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            if let Some(image) = compose(&value, u32::from(sw), u32::from(sh)) {
                let _ = tx.send(Msg::Wallpaper(Box::new(image)));
            }
        });
    }

    /// Обои готовы.
    pub fn set_wallpaper(&self, d: &Display, image: &Pixmap) -> Result<()> {
        self.window.set_wallpaper(d, image)
    }

    pub fn wants(&self, d: &Display, ev: &Event) -> bool {
        let own = |w: u32| w == self.window.id() || w == self.menu_popup.id();
        match *ev {
            Event::Button { window, .. }
            | Event::Release { window, .. }
            | Event::Motion { window, .. }
            | Event::Expose { window } => own(window) || self.menu.is_some(),
            Event::Key(_) => self.menu.is_some(),
            // Сменился размер экрана (корневое окно) — стол во весь экран заново.
            Event::Configure { window } => window == d.root(),
            _ => false,
        }
    }

    pub fn event(&mut self, d: &Display, fonts: &mut Fonts, ev: Event) -> Result<()> {
        if let Event::Configure { window } = ev
            && window == d.root()
        {
            self.window.show(d)?;
            self.load_wallpaper(d);
            return Ok(());
        }
        if self.menu.is_some() {
            return self.menu_event(d, ev);
        }
        if let Event::Button {
            window,
            x,
            y,
            button: 3,
        } = ev
            && window == self.window.id()
        {
            self.open_menu(d, fonts, x, y)?;
        }
        Ok(())
    }

    fn open_menu(&mut self, d: &Display, fonts: &mut Fonts, x: i16, y: i16) -> Result<()> {
        let entry = |label: &str, command, separator_before, checked, indent| MenuEntry {
            label: label.to_string(),
            command,
            checked,
            separator_before,
            enabled: true,
            indent,
        };
        let mut entries = vec![MenuEntry {
            enabled: false,
            ..entry("Обои", DesktopCommand::PickWallpaper, false, false, false)
        }];
        for &(name, label) in WALLPAPERS {
            let current = self.wallpaper == name;
            entries.push(entry(
                label,
                DesktopCommand::Wallpaper(name),
                false,
                current,
                true,
            ));
        }
        let own_picture = !WALLPAPERS.iter().any(|(n, _)| *n == self.wallpaper);
        entries.push(entry(
            "Картинка…",
            DesktopCommand::PickWallpaper,
            false,
            own_picture,
            true,
        ));
        entries.push(entry(
            "Открыть «Рабочий стол»",
            DesktopCommand::OpenDesktop,
            true,
            false,
            false,
        ));
        entries.push(entry(
            "Терминал",
            DesktopCommand::Terminal,
            false,
            false,
            false,
        ));
        entries.push(entry(
            "Настройки экрана…",
            DesktopCommand::DisplaySettings,
            true,
            false,
            false,
        ));
        entries.push(entry(
            "Системные настройки…",
            DesktopCommand::Settings,
            false,
            false,
            false,
        ));
        let mut menu = Menu::new("desktop".into(), entries);
        let (w, h) = menu.measure(fonts);
        // Меню под указателем; у края экрана — сдвинуть, чтобы влезло.
        let (sw, sh) = d.screen_size();
        let mx = i32::from(x).min(i32::from(sw) - w as i32 - 4).max(0);
        let my = i32::from(y).min(i32::from(sh) - h as i32 - 4).max(0);
        self.menu_popup
            .show(d, mx as i16, my as i16, w as u16, h as u16)?;
        self.menu = Some(menu);
        self.menu_dirty = true;
        Ok(())
    }

    fn menu_event(&mut self, d: &Display, ev: Event) -> Result<()> {
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
            // Щелчок мимо меню — закрыть.
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
                self.run(command);
            }
        }
        Ok(())
    }

    fn close_menu(&mut self, d: &Display) -> Result<()> {
        if self.menu.take().is_some() {
            self.menu_popup.hide(d)?;
        }
        Ok(())
    }

    /// Закрыть меню (например, перед открытием «Поиска»).
    pub fn close_popups(&mut self, d: &Display) -> Result<()> {
        self.close_menu(d)
    }

    pub fn flush(&mut self, d: &Display, fonts: &mut Fonts) -> Result<()> {
        if self.menu_dirty {
            self.menu_dirty = false;
            if let Some(menu) = &self.menu {
                self.menu_popup.present(d, &menu.render(fonts))?;
            }
        }
        Ok(())
    }

    fn run(&self, command: DesktopCommand) {
        let shell = crate::guard::own_exe().display().to_string();
        let desktop = desktop_dir();
        match command {
            // `shell set` запишет настройку, нарисует обои и перезапустит
            // службу — новый стол уже с ними.
            DesktopCommand::Wallpaper(name) => {
                spawn(&[&shell, "set", "look.wallpaper", name], None)
            }
            DesktopCommand::PickWallpaper => {
                let script = format!(
                    "f=$(zenity --file-selection --title='Картинка для обоев' \
                     --file-filter='Картинки | *.png *.jpg *.jpeg *.svg *.webp') \
                     && exec '{shell}' set look.wallpaper \"$f\""
                );
                spawn(&["sh", "-c", &script], None);
            }
            DesktopCommand::OpenDesktop => {
                let _ = std::fs::create_dir_all(&desktop);
                spawn(&["thunar", &desktop.display().to_string()], None);
            }
            DesktopCommand::Terminal => {
                if let Some(program) = self.terminal.first() {
                    spawn(&[program], Some(&desktop));
                }
            }
            DesktopCommand::DisplaySettings => spawn(&["xfce4-display-settings"], None),
            DesktopCommand::Settings => spawn(&["xfce4-settings-manager"], None),
        }
    }
}

/// Обои во весь экран `sw`×`sh` пикселей: на каждом мониторе — свои под его
/// размер (свои обои рисуются под размер один раз и хранятся). `None` —
/// нарисовать не вышло (останется ровный цвет).
fn compose(value: &str, sw: u32, sh: u32) -> Option<Pixmap> {
    let mut monitors = shell_setup::parts::monitor_rects();
    if monitors.is_empty() {
        monitors.push((0, 0, sw, sh));
    }
    let mut out = Pixmap::new(sw.max(1), sh.max(1))?;
    out.fill(tiny_skia::Color::from_rgba8(0x1e, 0x1b, 0x3a, 0xff));
    for (x, y, w, h) in monitors {
        let Some(file) = shell_setup::parts::wallpaper_file(value, i64::from(w), i64::from(h))
        else {
            eprintln!("Обои «{value}» не нарисованы и не найдены");
            continue;
        };
        let Some(image) = shell_icons::render_file(&file, w, h, Fit::Cover) else {
            eprintln!("Обои {} не прочитаны", file.display());
            continue;
        };
        out.draw_pixmap(
            x,
            y,
            image.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    Some(out)
}

/// Папка «Рабочий стол» (по `xdg-user-dir DESKTOP`, иначе `~/Desktop`).
fn desktop_dir() -> PathBuf {
    Command::new("xdg-user-dir")
        .arg("DESKTOP")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Desktop")
        })
}

/// Запустить программу отдельно от службы (в своей группе процессов).
fn spawn(args: &[&str], dir: Option<&PathBuf>) {
    let Some((program, rest)) = args.split_first() else {
        return;
    };
    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    match cmd.spawn() {
        // Дожидаемся в отдельном потоке, чтобы не копились «зомби».
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => eprintln!("Не запустить {program}: {e}"),
    }
}
