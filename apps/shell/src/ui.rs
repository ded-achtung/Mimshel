//! Окна оболочки на экране — Spotlight и Launchpad: служба (`shell daemon`)
//! или разовый запуск.
//!
//! Служба заранее загружает шрифты, иконки и список программ и ждёт команды
//! через сокет `$XDG_RUNTIME_DIR/shell.sock`; поэтому окна открываются сразу.
//! Без службы `shell spotlight` / `shell launchpad` делают всё сами —
//! медленнее, но работают. Открыто не больше одного окна: открытие одного
//! закрывает другое.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::Instant;

use shell_apps::{App, Settings, Watcher};
use shell_core::config::Config;
use shell_icons::Icons;
use shell_launcher::{Action, Grid, Launchpad, Spotlight, backdrop};
use shell_text::Fonts;
use shell_x11::{Display, Event, Key, KeyPress, Popup};

use crate::dock_ui::DockUi;
use crate::menubar_ui::MenubarUi;
use tiny_skia::Pixmap;

pub(crate) type Result<T> = shell_x11::Result<T>;

/// Иконки программ в списке Spotlight — такого размера (см. его оформление).
const ROW_ICON: u32 = 26;
/// Spotlight стоит на этой доле высоты экрана сверху, как в macOS.
const TOP: f32 = 0.22;
/// Сколько ждать перерисовки композитора после того, как окно спрятано.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(60);

/// Какое окно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Spotlight,
    Launchpad,
}

impl Panel {
    /// Имя команды — и в командной строке, и в сокете.
    pub fn name(self) -> &'static str {
        match self {
            Panel::Spotlight => "spotlight",
            Panel::Launchpad => "launchpad",
        }
    }

    pub fn parse(s: &str) -> Option<Panel> {
        [Panel::Spotlight, Panel::Launchpad]
            .into_iter()
            .find(|p| p.name() == s)
    }
}

pub(crate) enum Msg {
    X(Event),
    /// Открыть или закрыть окно.
    Toggle(Panel),
    /// Список программ изменился (все ярлыки, вместе с `NoDisplay`).
    Apps(Vec<App>),
    /// Кадр анимации дока.
    Tick,
    /// Корзина стала полной (`true`) или пустой.
    Trash(bool),
    /// Часы в строке меню (новая минута).
    Clock(String),
    /// Звук, батарея, «Не беспокоить» изменились.
    Sound(Option<shell_menubar::status::Sound>),
    Battery(Option<shell_menubar::status::Battery>),
    Dnd(bool),
    /// Значки трея или меню значка.
    Tray(shell_dbus::TrayUpdate),
}

/// Настройки: файл из `SHELL_CONFIG` (для проверок) или конфиг macos-look.
pub fn load_config() -> Config {
    match config_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(text) => Config::from_macos_look(&text),
        None => Config::default(),
    }
}

/// Файл настроек: из `SHELL_CONFIG` или `~/.config/macos-look/config.ini`.
pub fn config_path() -> Option<PathBuf> {
    std::env::var_os("SHELL_CONFIG")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/macos-look/config.ini"))
        })
}

/// Только ярлыки, видимые в меню, — для Spotlight и Launchpad.
fn visible(apps: &[App]) -> Vec<App> {
    apps.iter().filter(|a| !a.no_display).cloned().collect()
}

/// Где служба ждёт команды.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("shell.sock"),
        None => std::env::temp_dir().join(format!("shell-{}.sock", std::process::id())),
    }
}

/// Служба уже запущена (кто-то слушает её сокет)?
pub fn daemon_running() -> bool {
    UnixStream::connect(socket_path()).is_ok()
}

/// Просит запущенную службу открыть окно. `false` — службы нет.
pub fn ask_daemon(panel: Panel) -> bool {
    UnixStream::connect(socket_path())
        .and_then(|mut s| s.write_all(format!("{}\n", panel.name()).as_bytes()))
        .is_ok()
}

fn grid(config: &Config) -> Grid {
    Grid {
        columns: config.launchpad_columns,
        rows: config.launchpad_rows,
        icon: config.launchpad_icon,
    }
}

/// Запускает окна. `once` — открыть это окно один раз и выйти после закрытия;
/// `None` — работать службой и ждать команд.
pub fn run(once: Option<Panel>) -> Result<()> {
    let start = Instant::now();
    let daemon = once.is_none();
    let config = load_config();
    let (tx, rx) = mpsc::channel();

    if daemon {
        listen(tx.clone())?;
    }
    let display = Arc::new(Display::connect()?);
    {
        let display = display.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = display.next_event() {
                if tx.send(Msg::X(ev)).is_err() {
                    break;
                }
            }
        });
    }

    let settings = Settings::from_env();
    let all_apps = shell_apps::load_all(&settings);
    let apps = visible(&all_apps);
    let mut icons = Icons::new(&config.icon_theme);
    if daemon {
        watch_apps(settings, tx.clone());
        // Иконки всех программ — заранее, чтобы первое открытие было мгновенным.
        for name in apps.iter().filter_map(|a| a.icon.as_deref()) {
            icons.get(name, ROW_ICON);
            icons.get(name, config.launchpad_icon);
        }
    }
    // Своя строка меню — только у службы и только если выбрана вместо панели
    // Xfce (`SHELL_MENUBAR=1` — включить для проверки, не трогая настройки).
    let menubar = if daemon && (config.menubar || std::env::var_os("SHELL_MENUBAR").is_some()) {
        Some(MenubarUi::new(
            &display,
            &config,
            all_apps.clone(),
            tx.clone(),
        )?)
    } else {
        None
    };
    // Свой док — только у службы и только если он выбран вместо Plank.
    let dock = if daemon && config.dock {
        let dock = DockUi::new(&display, &config, all_apps, tx.clone())?;
        dock.preload(&mut icons);
        Some(dock)
    } else {
        None
    };
    let (sw, sh) = display.screen_size();
    let mut ui = Ui {
        display: &display,
        fonts: Fonts::new(&config.font),
        icons,
        spot_popup: display.create_popup("spotlight")?,
        pad_popup: display.create_popup("launchpad")?,
        spotlight: Spotlight::new(apps.clone()),
        launchpad: Launchpad::new(apps, (u32::from(sw), u32::from(sh)), grid(&config)),
        background: None,
        frame: None,
        shown: None,
        terminal: config.terminal,
        dock,
        menubar,
    };
    ui.flush()?;
    match once {
        None => eprintln!("Служба готова за {:?}", start.elapsed()),
        Some(panel) => {
            ui.toggle(panel)?;
            eprintln!("Окно открыто за {:?}", start.elapsed());
        }
    }

    while let Ok(msg) = rx.recv() {
        ui.message(msg)?;
        // Всё, что накопилось, пока шёл прошлый кадр, — разом, а док
        // рисуем один раз по итогу. Иначе при быстром движении мыши кадры
        // отстают от событий и док ещё секунды «доигрывает» старые движения.
        while let Ok(msg) = rx.try_recv() {
            ui.message(msg)?;
        }
        ui.flush()?;
        if !daemon && ui.shown.is_none() {
            break;
        }
    }
    Ok(())
}

struct Ui<'a> {
    display: &'a Display,
    fonts: Fonts,
    icons: Icons,
    spot_popup: Popup,
    pad_popup: Popup,
    spotlight: Spotlight,
    launchpad: Launchpad,
    /// Размытый снимок стола под Launchpad — делается при каждом открытии.
    background: Option<Pixmap>,
    /// Последняя нарисованная картинка — чтобы быстро повторить её по Expose.
    frame: Option<Pixmap>,
    shown: Option<Panel>,
    terminal: Vec<String>,
    dock: Option<DockUi>,
    menubar: Option<MenubarUi>,
}

impl Ui<'_> {
    fn toggle(&mut self, panel: Panel) -> Result<()> {
        if let Some(dock) = &mut self.dock {
            dock.close_popups(self.display)?;
        }
        if let Some(menubar) = &mut self.menubar {
            menubar.close_popup(self.display)?;
        }
        let was = self.shown;
        if was.is_some() {
            self.hide()?;
        }
        if was == Some(panel) {
            return Ok(());
        }
        if was.is_some() && panel == Panel::Launchpad {
            // Переход из Spotlight: композитор должен успеть убрать его с экрана,
            // иначе он попадёт в снимок под размытый фон.
            self.display.sync()?;
            std::thread::sleep(SETTLE);
        }
        match panel {
            Panel::Spotlight => self.show_spotlight(),
            Panel::Launchpad => self.show_launchpad(),
        }
    }

    fn show_spotlight(&mut self) -> Result<()> {
        self.spotlight.reset();
        let frame = self.spotlight.render(&mut self.fonts, &mut self.icons);
        let (x, y) = self.spotlight_position(frame.width());
        let (w, h) = (frame.width() as u16, frame.height() as u16);
        self.spot_popup.show(self.display, x, y, w, h)?;
        self.spot_popup.present(self.display, &frame)?;
        self.frame = Some(frame);
        self.shown = Some(Panel::Spotlight);
        Ok(())
    }

    fn show_launchpad(&mut self) -> Result<()> {
        let start = Instant::now();
        let (w, h) = self.display.screen_size();
        self.launchpad.set_screen(u32::from(w), u32::from(h));
        self.launchpad.reset();
        // Снимок — до показа окна, иначе в него попадёт сам Launchpad.
        let shot = match self.display.capture_screen() {
            Ok(shot) => Some(shot),
            Err(e) => {
                eprintln!("Снимок экрана не получился, фон будет ровным: {e}");
                None
            }
        };
        let shot_time = start.elapsed();
        let background = backdrop(shot.as_ref(), self.launchpad.size());
        let blur_time = start.elapsed();
        let frame = self
            .launchpad
            .render(&background, &mut self.fonts, &mut self.icons);
        let render_time = start.elapsed();
        self.pad_popup.show(self.display, 0, 0, w, h)?;
        self.pad_popup.present(self.display, &frame)?;
        eprintln!(
            "Launchpad: снимок {shot_time:?}, размытие {:?}, рисование {:?}, вывод {:?}",
            blur_time - shot_time,
            render_time - blur_time,
            start.elapsed() - render_time
        );
        self.background = Some(background);
        self.frame = Some(frame);
        self.shown = Some(Panel::Launchpad);
        Ok(())
    }

    fn hide(&mut self) -> Result<()> {
        match self.shown {
            Some(Panel::Spotlight) => self.spot_popup.hide(self.display)?,
            Some(Panel::Launchpad) => self.pad_popup.hide(self.display)?,
            None => {}
        }
        self.shown = None;
        self.frame = None;
        // Снимок стола занимает ~15 МБ — незачем держать его закрытым.
        self.background = None;
        Ok(())
    }

    /// Левый верхний угол Spotlight: по центру, на 22 % высоты экрана.
    fn spotlight_position(&self, width: u32) -> (i16, i16) {
        let (sw, sh) = self.display.screen_size();
        let x = (i32::from(sw) - width as i32) / 2;
        let y = (f32::from(sh) * TOP) as i32;
        (x as i16, y as i16)
    }

    fn redraw(&mut self) -> Result<()> {
        match self.shown {
            Some(Panel::Spotlight) => {
                let frame = self.spotlight.render(&mut self.fonts, &mut self.icons);
                let old = self.frame.as_ref().map(|f| (f.width(), f.height()));
                if old != Some((frame.width(), frame.height())) {
                    let (x, y) = self.spotlight_position(frame.width());
                    let (w, h) = (frame.width() as u16, frame.height() as u16);
                    self.spot_popup.set_geometry(self.display, x, y, w, h)?;
                }
                self.spot_popup.present(self.display, &frame)?;
                self.frame = Some(frame);
            }
            Some(Panel::Launchpad) => {
                let Some(background) = &self.background else {
                    return Ok(());
                };
                let frame = self
                    .launchpad
                    .render(background, &mut self.fonts, &mut self.icons);
                self.pad_popup.present(self.display, &frame)?;
                self.frame = Some(frame);
            }
            None => {}
        }
        Ok(())
    }

    /// Перерисовать строку меню и док, если в них что-то изменилось.
    fn flush(&mut self) -> Result<()> {
        if let Some(menubar) = &mut self.menubar {
            menubar.flush(self.display, &mut self.fonts, &mut self.icons)?;
        }
        if let Some(dock) = &mut self.dock {
            dock.flush(self.display, &mut self.fonts, &mut self.icons)?;
        }
        Ok(())
    }

    fn set_apps(&mut self, all: Vec<App>) -> Result<()> {
        let apps = visible(&all);
        self.spotlight.set_apps(apps.clone());
        self.launchpad.set_apps(apps);
        if let Some(menubar) = &mut self.menubar {
            menubar.set_apps(self.display, all.clone());
        }
        if let Some(dock) = &mut self.dock {
            dock.set_apps(self.display, all);
        }
        if self.shown.is_some() {
            self.redraw()?;
        }
        Ok(())
    }

    fn message(&mut self, msg: Msg) -> Result<()> {
        match msg {
            Msg::Toggle(panel) => self.toggle(panel),
            Msg::Apps(apps) => self.set_apps(apps),
            Msg::X(ev) => {
                if std::env::var_os("SHELL_DEBUG_DOCK").is_some() {
                    eprintln!("X: {ev:?}");
                }
                self.event(ev)
            }
            Msg::Tick => {
                if let Some(dock) = &mut self.dock {
                    dock.tick();
                }
                Ok(())
            }
            Msg::Trash(full) => {
                if let Some(dock) = &mut self.dock {
                    dock.set_trash(self.display, full)?;
                }
                Ok(())
            }
            Msg::Clock(text) => {
                if let Some(menubar) = &mut self.menubar {
                    menubar.set_clock(text);
                }
                Ok(())
            }
            Msg::Sound(sound) => {
                if let Some(menubar) = &mut self.menubar {
                    menubar.set_sound(sound);
                }
                Ok(())
            }
            Msg::Battery(battery) => {
                if let Some(menubar) = &mut self.menubar {
                    menubar.set_battery(battery);
                }
                Ok(())
            }
            Msg::Dnd(on) => {
                if let Some(menubar) = &mut self.menubar {
                    menubar.set_dnd(on);
                }
                Ok(())
            }
            Msg::Tray(update) => match &mut self.menubar {
                Some(menubar) => {
                    menubar.tray_update(self.display, &mut self.fonts, &mut self.icons, update)
                }
                None => Ok(()),
            },
        }
    }

    fn event(&mut self, ev: Event) -> Result<()> {
        // Одно событие может быть нужно и строке меню, и доку (например,
        // сменилось активное окно).
        let mut handled = false;
        if let Some(menubar) = &mut self.menubar
            && menubar.wants(self.display, &ev)
        {
            menubar.event(self.display, &mut self.fonts, ev)?;
            handled = true;
        }
        if let Some(dock) = &mut self.dock
            && dock.wants(self.display, &ev)
        {
            dock.event(self.display, &mut self.fonts, ev)?;
            handled = true;
        }
        if handled {
            return Ok(());
        }
        let Some(panel) = self.shown else {
            return Ok(());
        };
        let action = match (panel, ev) {
            (Panel::Spotlight, Event::Key(k)) => self.spotlight.key(k),
            (Panel::Launchpad, Event::Key(k)) => self.launchpad.key(k),
            (Panel::Spotlight, Event::Button { x, y, button, .. }) => {
                self.spotlight.click(i32::from(x), i32::from(y), button)
            }
            (Panel::Launchpad, Event::Button { x, y, button, .. }) => {
                self.launchpad.click(i32::from(x), i32::from(y), button)
            }
            (Panel::Spotlight, Event::Motion { x, y, .. }) => {
                self.spotlight.motion(i32::from(x), i32::from(y))
            }
            (_, Event::Expose { .. }) => {
                if let Some(frame) = &self.frame {
                    let popup = match panel {
                        Panel::Spotlight => &self.spot_popup,
                        Panel::Launchpad => &self.pad_popup,
                    };
                    popup.present(self.display, frame)?;
                }
                Action::Nothing
            }
            _ => Action::Nothing,
        };
        // Из Spotlight Super+A сразу открывает Launchpad и наоборот.
        if let Event::Key(k) = ev
            && k.logo
            && let Some(other) = switch_to(panel, k)
        {
            return self.toggle(other);
        }
        match action {
            Action::Nothing => {}
            Action::Redraw => self.redraw()?,
            Action::Close => self.hide()?,
            Action::Launch(app) => {
                self.hide()?;
                match app.launch(&self.terminal) {
                    Ok(_) => {
                        if let Some(dock) = &mut self.dock {
                            dock.launched(&app);
                        }
                    }
                    Err(e) => eprintln!("Не удалось запустить «{}»: {e}", app.name),
                }
            }
        }
        Ok(())
    }
}

/// Сочетание другого окна: Super+Пробел в Launchpad, Super+A в Spotlight.
fn switch_to(panel: Panel, k: KeyPress) -> Option<Panel> {
    match (panel, k.key) {
        (Panel::Spotlight, Key::Char('a')) => Some(Panel::Launchpad),
        (Panel::Launchpad, Key::Char(' ')) => Some(Panel::Spotlight),
        _ => None,
    }
}

/// Принимает команды через сокет в отдельном потоке.
fn listen(tx: Sender<Msg>) -> Result<()> {
    let path = socket_path();
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(&path).is_ok() {
                return Err("служба уже запущена".into());
            }
            // Файл остался от упавшей службы — убираем и занимаем заново.
            std::fs::remove_file(&path)?;
            UnixListener::bind(&path)?
        }
        Err(e) => return Err(e.into()),
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            for line in BufReader::new(stream).lines().map_while(|l| l.ok()) {
                let Some(panel) = Panel::parse(line.trim()) else {
                    eprintln!("Неизвестная команда: {}", line.trim());
                    continue;
                };
                if tx.send(Msg::Toggle(panel)).is_err() {
                    return;
                }
            }
        }
    });
    Ok(())
}

/// Следит за ярлыками и присылает новый список программ.
fn watch_apps(settings: Settings, tx: Sender<Msg>) {
    let mut watcher = match Watcher::new(&settings) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Не удалось следить за ярлыками: {e}");
            return;
        }
    };
    std::thread::spawn(move || {
        while watcher.wait().is_ok() {
            if tx.send(Msg::Apps(shell_apps::load_all(&settings))).is_err() {
                break;
            }
        }
    });
}

/// Рисует окно с набранным текстом в PNG. Для Launchpad, если доступен
/// экран, берёт его размер и снимок для фона; иначе — 1920×1080, ровный фон.
pub fn snapshot(panel: Panel, text: &str, file: &str) -> Result<()> {
    let config = load_config();
    let start = Instant::now();
    let mut fonts = Fonts::new(&config.font);
    let mut icons = Icons::new(&config.icon_theme);
    let apps = shell_apps::load(&Settings::from_env());
    eprintln!("Загрузка: {:?}", start.elapsed());
    let keys = text.chars().map(|c| KeyPress::plain(Key::Char(c)));
    let start = Instant::now();
    let frame = match panel {
        Panel::Spotlight => {
            let mut s = Spotlight::new(apps);
            keys.for_each(|k| {
                s.key(k);
            });
            s.render(&mut fonts, &mut icons)
        }
        Panel::Launchpad => {
            let display = Display::connect().ok();
            let size = display.as_ref().map_or((1920, 1080), |d| {
                let (w, h) = d.screen_size();
                (u32::from(w), u32::from(h))
            });
            let shot = display.and_then(|d| d.capture_screen().ok());
            let mut l = Launchpad::new(apps, size, grid(&config));
            keys.for_each(|k| {
                l.key(k);
            });
            let background = backdrop(shot.as_ref(), size);
            l.render(&background, &mut fonts, &mut icons)
        }
    };
    eprintln!("Рисование: {:?}", start.elapsed());
    frame.save_png(file)?;
    Ok(())
}
