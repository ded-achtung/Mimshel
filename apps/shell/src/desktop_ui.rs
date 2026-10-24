//! Рабочий стол (вместо xfdesktop): окно под всеми окнами с обоями на
//! каждом мониторе, меню по правому щелчку («Изменить обои…», «Настройки
//! экрана…») и окно «Обои» с миниатюрами (`shell_desktop::WallpaperPicker`).
//!
//! Всё долгое — в отдельных потоках, результат приходит сообщением
//! [`DesktopMsg`]: обои под мониторы (свои под новый размер — секунды),
//! миниатюры, выбранный файл. Обои меняются без перезапуска службы: настройка
//! пишется в файл, стол и фон блокировки перерисовываются.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

use shell_core::config::{Config, set_value, setting_value};
use shell_desktop::{PickerResult, WallpaperPicker, wallpaper_picker};
use shell_icons::Fit;
use shell_text::Fonts;
use shell_widgets::{Menu, MenuEntry, MenuResult};
use shell_x11::{DesktopWindow, Display, Event, Popup};
use tiny_skia::{Pixmap, PixmapPaint, Transform};

use crate::ui::{Msg, Result};

/// Пункты меню стола.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DesktopCommand {
    ChangeWallpaper,
    DisplaySettings,
}

/// Что приходит столу из его потоков.
pub enum DesktopMsg {
    /// Обои во весь экран (в пикселях) для этого значения настройки.
    Wallpaper(String, Pixmap),
    /// Миниатюра для окна «Обои».
    Thumb(String, Pixmap),
    /// Окно выбора файла закрыто: путь к картинке или `None` (отмена).
    Picked(Option<String>),
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
    picker_popup: Popup,
    picker: Option<WallpaperPicker>,
    picker_dirty: bool,
    /// Нынешние обои (значение `look.wallpaper`).
    wallpaper: String,
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
            picker_popup: d.create_popup("wallpaper-picker")?,
            picker: None,
            picker_dirty: false,
            wallpaper: config.wallpaper.clone(),
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
                let msg = DesktopMsg::Wallpaper(value, image);
                let _ = tx.send(Msg::Desktop(Box::new(msg)));
            }
        });
    }

    /// Пришло из потока.
    pub fn message(&mut self, d: &Display, msg: DesktopMsg) -> Result<()> {
        match msg {
            // Пока рисовались одни обои, могли выбрать другие — старые не ставим.
            DesktopMsg::Wallpaper(value, image) if value == self.wallpaper => {
                self.window.set_wallpaper(d, &image)
            }
            DesktopMsg::Wallpaper(..) => Ok(()),
            DesktopMsg::Thumb(value, image) => {
                if let Some(picker) = &mut self.picker {
                    picker.set_thumb(value, image);
                    self.picker_dirty = true;
                }
                Ok(())
            }
            DesktopMsg::Picked(path) => {
                if let Some(path) = path {
                    self.apply(d, &path);
                }
                self.open_picker(d)
            }
        }
    }

    pub fn wants(&self, d: &Display, ev: &Event) -> bool {
        let own = |w: u32| {
            w == self.window.id() || w == self.menu_popup.id() || w == self.picker_popup.id()
        };
        let open = self.menu.is_some() || self.picker.is_some();
        match *ev {
            Event::Button { window, .. }
            | Event::Release { window, .. }
            | Event::Motion { window, .. }
            | Event::Expose { window } => own(window) || open,
            Event::Key(_) => open,
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
        if self.picker.is_some() {
            return self.picker_event(d, ev);
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

    /// Меню по правому щелчку — только то, что относится к столу.
    fn open_menu(&mut self, d: &Display, fonts: &mut Fonts, x: i16, y: i16) -> Result<()> {
        let entry = |label: &str, command| MenuEntry {
            label: label.to_string(),
            command,
            checked: false,
            separator_before: false,
            enabled: true,
            indent: false,
        };
        let mut menu = Menu::new(
            "desktop".into(),
            vec![
                entry("Изменить обои…", DesktopCommand::ChangeWallpaper),
                entry("Настройки экрана…", DesktopCommand::DisplaySettings),
            ],
        );
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
                match command {
                    DesktopCommand::ChangeWallpaper => self.open_picker(d)?,
                    DesktopCommand::DisplaySettings => spawn(&["xfce4-display-settings"]),
                }
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

    /// Окно «Обои» посередине экрана; миниатюры рисуются в фоне.
    fn open_picker(&mut self, d: &Display) -> Result<()> {
        let picker = WallpaperPicker::new(WALLPAPERS, &self.wallpaper);
        let (w, h) = picker.size();
        let (sw, sh) = d.screen_size();
        let x = (i32::from(sw) - w as i32) / 2;
        let y = (i32::from(sh) - h as i32) / 3;
        self.picker_popup
            .show(d, x.max(0) as i16, y.max(0) as i16, w as u16, h as u16)?;
        let values = picker.values();
        self.picker = Some(picker);
        self.picker_dirty = true;
        let tx = self.tx.clone();
        let scale = d.scale();
        std::thread::spawn(move || {
            let tw = shell_render::to_px(wallpaper_picker::THUMB_W as u32, scale);
            let th = shell_render::to_px(wallpaper_picker::THUMB_H as u32, scale);
            for value in values {
                if let Some(image) = thumb(&value, tw, th) {
                    let msg = DesktopMsg::Thumb(value, image);
                    if tx.send(Msg::Desktop(Box::new(msg))).is_err() {
                        return;
                    }
                }
            }
        });
        Ok(())
    }

    fn picker_event(&mut self, d: &Display, ev: Event) -> Result<()> {
        let Some(picker) = &mut self.picker else {
            return Ok(());
        };
        let popup = self.picker_popup.id();
        let result = match ev {
            Event::Motion { window, x, y } if window == popup => {
                if picker.motion(f32::from(x), f32::from(y)) {
                    PickerResult::Redraw
                } else {
                    PickerResult::Nothing
                }
            }
            Event::Button { window, x, y, .. } if window == popup => {
                picker.click(f32::from(x), f32::from(y))
            }
            Event::Button { .. } => PickerResult::Close,
            Event::Key(k) => picker.key(k.key),
            Event::Expose { window } if window == popup => PickerResult::Redraw,
            _ => PickerResult::Nothing,
        };
        match result {
            PickerResult::Nothing => {}
            PickerResult::Redraw => self.picker_dirty = true,
            PickerResult::Close => self.close_picker(d)?,
            PickerResult::Apply(value) => {
                self.apply(d, &value);
                if let Some(picker) = &mut self.picker {
                    picker.set_current(&value);
                }
                self.picker_dirty = true;
            }
            PickerResult::PickFile => {
                // Окно выбора файла не получит мышь, пока «Обои» её держат.
                self.close_picker(d)?;
                pick_file(self.tx.clone());
            }
        }
        Ok(())
    }

    fn close_picker(&mut self, d: &Display) -> Result<()> {
        if self.picker.take().is_some() {
            self.picker_popup.hide(d)?;
        }
        Ok(())
    }

    /// Поставить обои: запомнить в настройках, перерисовать стол и фон
    /// блокировки. Служба не перезапускается.
    fn apply(&mut self, d: &Display, value: &str) {
        if let Err(e) = save_wallpaper(value) {
            eprintln!("Обои не сохранены в настройках: {e}");
        }
        self.wallpaper = value.to_string();
        self.load_wallpaper(d);
        let value = value.to_string();
        std::thread::spawn(move || {
            let (w, h) = shell_setup::lock::screen_size();
            shell_setup::lock::background(&value, w, h);
        });
    }

    /// Закрыть меню и окно «Обои» (например, перед открытием «Поиска»).
    pub fn close_popups(&mut self, d: &Display) -> Result<()> {
        self.close_menu(d)?;
        self.close_picker(d)
    }

    pub fn flush(&mut self, d: &Display, fonts: &mut Fonts) -> Result<()> {
        if self.menu_dirty {
            self.menu_dirty = false;
            if let Some(menu) = &self.menu {
                self.menu_popup.present(d, &menu.render(fonts))?;
            }
        }
        if self.picker_dirty {
            self.picker_dirty = false;
            if let Some(picker) = &self.picker {
                self.picker_popup.present(d, &picker.render(fonts))?;
            }
        }
        Ok(())
    }
}

/// Записать `look.wallpaper` в файл настроек (пояснения в файле остаются).
fn save_wallpaper(value: &str) -> std::io::Result<()> {
    let Some(path) = crate::ui::config_path() else {
        return Ok(());
    };
    let v = setting_value("look.wallpaper", value).map_err(std::io::Error::other)?;
    crate::ui::config_store(path).edit(|text| {
        set_value(text, "look", "wallpaper", v)
            .ok_or_else(|| std::io::Error::other("не удалось изменить настройки"))
    })
}

/// Миниатюра обоев `tw`×`th` пикселей: из уже нарисованных под экран обоев
/// (так она такая же, как на столе), иначе свои рисуются сразу маленькими.
fn thumb(value: &str, tw: u32, th: u32) -> Option<Pixmap> {
    let (sw, sh) = shell_setup::lock::screen_size();
    let drawn = match shell_wallpaper::Style::parse(value) {
        Some(style) => {
            let (sw, sh) = (u32::try_from(sw).ok()?, u32::try_from(sh).ok()?);
            let file = shell_setup::parts::wallpaper_path(style, sw, sh);
            if file.exists() {
                Some(file)
            } else {
                return shell_wallpaper::draw(style, tw, th);
            }
        }
        None => Some(PathBuf::from(value)),
    };
    shell_icons::render_file(&drawn?, tw, th, Fit::Cover)
}

/// Окно выбора картинки (zenity) в отдельном потоке.
fn pick_file(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        // Открыть сразу в папке «Изображения» (по xdg-user-dir).
        let pictures = Command::new("xdg-user-dir")
            .arg("PICTURES")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && std::path::Path::new(s).is_dir());
        let start = pictures.map(|p| format!("--filename={p}/"));
        let path = Command::new("zenity")
            .args(start)
            .args([
                "--file-selection",
                "--title=Картинка для обоев",
                "--file-filter=Картинки | *.png *.jpg *.jpeg *.svg *.webp *.PNG *.JPG *.JPEG",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let _ = tx.send(Msg::Desktop(Box::new(DesktopMsg::Picked(path))));
    });
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

/// Запустить программу отдельно от службы (в своей группе процессов).
fn spawn(args: &[&str]) {
    let Some((program, rest)) = args.split_first() else {
        return;
    };
    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    match cmd.spawn() {
        // Дожидаемся в отдельном потоке, чтобы не копились «зомби».
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => eprintln!("Не запустить {program}: {e}"),
    }
}
