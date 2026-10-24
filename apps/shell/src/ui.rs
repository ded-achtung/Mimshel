//! Окна оболочки на экране — «Поиск» и «Программы»: служба (`shell daemon`)
//! или разовый запуск.
//!
//! Служба заранее загружает шрифты, иконки и список программ и ждёт команды
//! через сокет `$XDG_RUNTIME_DIR/shell.sock`; поэтому окна открываются сразу.
//! Без службы `shell search` / `shell apps` делают всё сами —
//! медленнее, но работают. Открыто не больше одного окна: открытие одного
//! закрывает другое.

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::Instant;

use shell_apps::{App, Settings, Watcher};
use shell_core::config::Config;
use shell_icons::Icons;
use shell_launcher::{Action, AppsPanel, Grid, SearchPanel, backdrop};
use shell_render::Canvas;
use shell_text::Fonts;
use shell_x11::{Display, Event, Key, KeyPress, Popup};

use crate::dock_ui::DockUi;
use crate::menubar_ui::MenubarUi;
use crate::notify_ui::NotifyUi;
use tiny_skia::Pixmap;

pub(crate) type Result<T> = shell_x11::Result<T>;

/// Иконки программ в списке «Поиска» — такого размера (см. его оформление).
const ROW_ICON: u32 = 26;
/// «Поиск» стоит на этой доле высоты экрана сверху, как в macOS.
const TOP: f32 = 0.22;
/// Сколько ждать перерисовки композитора после того, как окно спрятано.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(60);

/// Какое окно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// Окно «Поиск».
    Search,
    /// Окно «Программы».
    Apps,
}

impl Panel {
    /// Имя команды — и в командной строке, и в сокете.
    pub fn name(self) -> &'static str {
        match self {
            Panel::Search => "search",
            Panel::Apps => "apps",
        }
    }

    /// По имени команды. Прежние имена (`spotlight`, `launchpad`) тоже: ими
    /// ещё вызывают окна сочетания клавиш, поставленные до переименования.
    pub fn parse(s: &str) -> Option<Panel> {
        match s {
            "search" | "spotlight" => Some(Panel::Search),
            "apps" | "launchpad" => Some(Panel::Apps),
            _ => None,
        }
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
    Control(shell_menubar::Network, Option<bool>),
    Battery(Option<shell_menubar::status::Battery>),
    Dnd(bool),
    /// Значки трея или меню значка.
    Tray(shell_dbus::TrayUpdate),
    /// Глобальное меню активного окна.
    AppMenu(shell_dbus::AppMenuUpdate),
    /// Уведомление пришло или программа его закрыла.
    Notify(shell_dbus::notify::NotifyEvent),
    /// Щёлкнули по колокольчику — открыть или закрыть центр уведомлений.
    ToggleCenter,
}

/// Настройки из своего файла (`SHELL_CONFIG` — другой файл, для проверок).
/// Файла ещё нет — он создаётся из значений по умолчанию.
pub fn load_config() -> Config {
    match config_path() {
        Some(path) => config_store(path).load(),
        None => Config::default(),
    }
}

pub fn config_store(path: PathBuf) -> crate::config_store::Store {
    let good = if std::env::var_os("SHELL_CONFIG").is_some() {
        None
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
            .map(|p| p.join("shell/config.good.toml"))
    };
    crate::config_store::Store { path, good }
}

/// Файл настроек: из `SHELL_CONFIG` или `~/.config/shell/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    std::env::var_os("SHELL_CONFIG")
        .map(PathBuf::from)
        .or_else(|| {
            let base = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
            Some(base.join("shell/config.toml"))
        })
}

/// Только ярлыки, видимые в меню, — для «Поиска» и «Программ».
fn visible(apps: &[App]) -> Vec<App> {
    apps.iter().filter(|a| !a.no_display).cloned().collect()
}

/// Служба уже запущена (кто-то слушает её сокет)?
pub fn daemon_running() -> bool {
    crate::ipc::socket_path()
        .and_then(UnixStream::connect)
        .is_ok()
}

/// Просит запущенную службу открыть окно. `false` — службы нет.
pub fn ask_daemon(panel: Panel) -> bool {
    crate::ipc::socket_path()
        .and_then(UnixStream::connect)
        .and_then(|mut s| s.write_all(format!("{}\n", panel.name()).as_bytes()))
        .is_ok()
}

fn grid(config: &Config) -> Grid {
    Grid {
        columns: config.apps_columns,
        rows: config.apps_rows,
        icon: config.apps_icon,
    }
}

/// Настройки чтения ярлыков: папки из окружения, скрытие и названия — из конфига.
pub fn app_settings(config: &Config) -> Settings {
    Settings {
        hide: config.hide_apps.clone(),
        rename: config.rename_apps.clone(),
        ..Settings::from_env()
    }
}

/// Масштаб для крупных экранов — до создания окон: из настроек, при «auto» —
/// по DPI системы. `SHELL_SCALE` — задать для проверки, не трогая настройки.
fn set_scale(config: &Config) {
    let forced = std::env::var("SHELL_SCALE")
        .ok()
        .and_then(|v| v.replace(',', ".").parse::<f32>().ok());
    let k = forced
        .or(config.scale)
        .unwrap_or_else(|| shell_x11::xft_dpi().map_or(1.0, shell_x11::scale_for_dpi));
    shell_render::set_scale(k);
    if shell_render::scale() != 1.0 {
        eprintln!("Масштаб: {}", shell_render::scale());
    }
}

/// Запускает окна. `once` — открыть это окно один раз и выйти после закрытия;
/// `None` — работать службой и ждать команд.
pub fn run(once: Option<Panel>) -> Result<()> {
    let start = Instant::now();
    // Время шагов запуска — в журнал: долгий запуск после загрузки ВМ иначе
    // не разобрать.
    let mut steps: Vec<(&str, std::time::Duration)> = Vec::new();
    let mut last = start;
    let mut step = |name: &'static str| {
        let now = Instant::now();
        steps.push((name, now - last));
        last = now;
    };
    let daemon = once.is_none();
    let config = load_config();
    set_scale(&config);
    step("настройки");
    // Шрифты читаются с диска долго (после загрузки — секунды): пусть грузятся
    // заодно со списком программ и значками.
    let fonts = {
        let family = config.font.clone();
        std::thread::spawn(move || Fonts::new(&family))
    };
    let (tx, rx) = mpsc::channel();

    if daemon {
        listen(tx.clone())?;
        // У виртуальной клавиатуры после входа одна раскладка (урок 18): одно
        // нажатие от программы — и значок раскладки пропадал. Вернуть все;
        // строка меню сама заметит смену клавиатуры.
        std::thread::spawn(|| {
            if shell_setup::parts::fix_virtual_keyboard() {
                eprintln!("Раскладки виртуальной клавиатуры восстановлены");
            }
        });
    }
    let display = Arc::new(Display::connect()?);
    step("экран");
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

    let settings = app_settings(&config);
    let all_apps = shell_apps::load_all(&settings);
    let apps = visible(&all_apps);
    step("программы");
    let mut icons = Icons::new(&config.icon_theme);
    if daemon {
        watch_apps(settings, tx.clone());
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
    step("строка меню");
    // Свои уведомления — вместо xfce4-notifyd (`SHELL_NOTIFY=1` — для проверки).
    let notify = if daemon && (config.notify || std::env::var_os("SHELL_NOTIFY").is_some()) {
        if menubar.is_none() {
            crate::status::watch_dnd(tx.clone());
        }
        Some(NotifyUi::new(
            &display,
            all_apps.clone(),
            config.menubar_height,
            true,
            tx.clone(),
        )?)
    } else {
        None
    };
    step("уведомления");
    // Свой док — только у службы и только если он выбран вместо Plank.
    let dock = if daemon && config.dock {
        let dock = DockUi::new(&display, &config, all_apps, tx.clone())?;
        dock.preload(&mut icons);
        Some(dock)
    } else {
        None
    };
    step("док");
    let preload: Vec<String> = apps.iter().filter_map(|a| a.icon.clone()).collect();
    let fonts = fonts.join().map_err(|_| "сбой при загрузке шрифтов")?;
    step("шрифты");
    let (sw, sh) = display.screen_size();
    let mut ui = Ui {
        display: &display,
        fonts,
        icons,
        search_popup: display.create_popup("search")?,
        apps_popup: display.create_popup("apps")?,
        search: SearchPanel::new(apps.clone()),
        apps_panel: AppsPanel::new(apps, (u32::from(sw), u32::from(sh)), grid(&config)),
        background: None,
        frame: None,
        shown: None,
        terminal: config.terminal,
        dock,
        menubar,
        notify,
    };
    ui.flush()?;
    step("окна");
    if daemon {
        // Иконки всех программ — заранее, чтобы первое открытие «Поиска» и
        // «Программ» было мгновенным. Уже после показа строки меню и дока:
        // им эти иконки не нужны.
        for name in &preload {
            ui.icons.get(name, ROW_ICON);
            ui.icons.get(name, config.apps_icon);
        }
        step("значки");
    }
    match once {
        None => {
            eprintln!("Служба готова за {:?}", start.elapsed());
            let line: Vec<String> = steps
                .iter()
                .map(|(name, d)| format!("{name} {:.2} с", d.as_secs_f32()))
                .collect();
            eprintln!("  по шагам: {}", line.join(", "));
        }
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
    search_popup: Popup,
    apps_popup: Popup,
    search: SearchPanel,
    apps_panel: AppsPanel,
    /// Размытый снимок стола под окном «Программы» — делается при каждом открытии.
    background: Option<Pixmap>,
    /// Последняя нарисованная картинка — чтобы быстро повторить её по Expose.
    frame: Option<Canvas>,
    shown: Option<Panel>,
    terminal: Vec<String>,
    dock: Option<DockUi>,
    menubar: Option<MenubarUi>,
    notify: Option<NotifyUi>,
}

impl Ui<'_> {
    fn toggle(&mut self, panel: Panel) -> Result<()> {
        if let Some(dock) = &mut self.dock {
            dock.close_popups(self.display)?;
        }
        if let Some(menubar) = &mut self.menubar {
            menubar.close_popup(self.display)?;
        }
        if let Some(notify) = &mut self.notify {
            notify.close_center(self.display)?;
        }
        let was = self.shown;
        if was.is_some() {
            self.hide()?;
        }
        if was == Some(panel) {
            return Ok(());
        }
        if was.is_some() && panel == Panel::Apps {
            // Переход из «Поиска»: композитор должен успеть убрать его с экрана,
            // иначе он попадёт в снимок под размытый фон.
            self.display.sync()?;
            std::thread::sleep(SETTLE);
        }
        match panel {
            Panel::Search => self.show_search(),
            Panel::Apps => self.show_apps(),
        }
    }

    fn show_search(&mut self) -> Result<()> {
        self.search.reset();
        let frame = self.search.render(&mut self.fonts, &mut self.icons);
        let (x, y) = self.search_position(frame.width());
        let (w, h) = (frame.width() as u16, frame.height() as u16);
        self.search_popup.show(self.display, x, y, w, h)?;
        self.search_popup.present(self.display, &frame)?;
        self.frame = Some(frame);
        self.shown = Some(Panel::Search);
        Ok(())
    }

    fn show_apps(&mut self) -> Result<()> {
        let start = Instant::now();
        let (w, h) = self.display.screen_size();
        self.apps_panel.set_screen(u32::from(w), u32::from(h));
        self.apps_panel.reset();
        // Снимок — до показа окна, иначе в него попадёт само окно.
        let shot = match self.display.capture_screen() {
            Ok(shot) => Some(shot),
            Err(e) => {
                eprintln!("Снимок экрана не получился, фон будет ровным: {e}");
                None
            }
        };
        let shot_time = start.elapsed();
        // Фон — в пикселях экрана, всё остальное окно считает в точках.
        let (pw, ph) = self.display.screen_pixels();
        let background = backdrop(shot.as_ref(), (u32::from(pw), u32::from(ph)));
        let blur_time = start.elapsed();
        let scale = self.display.scale();
        let frame = self
            .apps_panel
            .render(&background, scale, &mut self.fonts, &mut self.icons);
        let render_time = start.elapsed();
        self.apps_popup.show(self.display, 0, 0, w, h)?;
        self.apps_popup.present(self.display, &frame)?;
        eprintln!(
            "Программы: снимок {shot_time:?}, размытие {:?}, рисование {:?}, вывод {:?}",
            blur_time - shot_time,
            render_time - blur_time,
            start.elapsed() - render_time
        );
        self.background = Some(background);
        self.frame = Some(frame);
        self.shown = Some(Panel::Apps);
        Ok(())
    }

    fn hide(&mut self) -> Result<()> {
        match self.shown {
            Some(Panel::Search) => self.search_popup.hide(self.display)?,
            Some(Panel::Apps) => self.apps_popup.hide(self.display)?,
            None => {}
        }
        self.shown = None;
        self.frame = None;
        // Снимок стола занимает ~15 МБ — незачем держать его закрытым.
        self.background = None;
        Ok(())
    }

    /// Левый верхний угол «Поиска»: по центру, на 22 % высоты экрана.
    fn search_position(&self, width: u32) -> (i16, i16) {
        let (sw, sh) = self.display.screen_size();
        let x = (i32::from(sw) - width as i32) / 2;
        let y = (f32::from(sh) * TOP) as i32;
        (x as i16, y as i16)
    }

    fn redraw(&mut self) -> Result<()> {
        match self.shown {
            Some(Panel::Search) => {
                let frame = self.search.render(&mut self.fonts, &mut self.icons);
                let old = self.frame.as_ref().map(|f| (f.width(), f.height()));
                if old != Some((frame.width(), frame.height())) {
                    let (x, y) = self.search_position(frame.width());
                    let (w, h) = (frame.width() as u16, frame.height() as u16);
                    self.search_popup.set_geometry(self.display, x, y, w, h)?;
                }
                self.search_popup.present(self.display, &frame)?;
                self.frame = Some(frame);
            }
            Some(Panel::Apps) => {
                let Some(background) = &self.background else {
                    return Ok(());
                };
                let scale = self.display.scale();
                let frame =
                    self.apps_panel
                        .render(background, scale, &mut self.fonts, &mut self.icons);
                self.apps_popup.present(self.display, &frame)?;
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
        if let Some(notify) = &mut self.notify {
            notify.flush(self.display, &mut self.fonts)?;
            // Колокольчик в строке меню подсвечен, пока открыт центр уведомлений.
            if let Some(menubar) = &mut self.menubar {
                menubar.set_center_open(notify.center_open());
            }
        }
        if let Some(dock) = &mut self.dock {
            dock.flush(self.display, &mut self.fonts, &mut self.icons)?;
        }
        Ok(())
    }

    fn set_apps(&mut self, all: Vec<App>) -> Result<()> {
        // Новые программы могли принести свои значки.
        self.icons.refresh();
        let apps = visible(&all);
        self.search.set_apps(apps.clone());
        self.apps_panel.set_apps(apps);
        if let Some(menubar) = &mut self.menubar {
            menubar.set_apps(self.display, all.clone());
        }
        if let Some(notify) = &mut self.notify {
            notify.set_apps(all.clone());
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
                if let Some(notify) = &mut self.notify {
                    notify.tick();
                }
                Ok(())
            }
            Msg::Notify(e) => {
                if let Some(notify) = &mut self.notify {
                    notify.notify_event(&mut self.icons, e);
                }
                Ok(())
            }
            Msg::ToggleCenter => match &mut self.notify {
                Some(notify) => notify.toggle_center(self.display, &mut self.fonts),
                None => Ok(()),
            },
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
            Msg::Control(network, bluetooth) => {
                if let Some(menubar) = &mut self.menubar {
                    menubar.set_control(network, bluetooth);
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
                if let Some(notify) = &mut self.notify {
                    notify.set_dnd(on);
                }
                Ok(())
            }
            Msg::AppMenu(update) => match &mut self.menubar {
                Some(menubar) => menubar.app_menu_update(self.display, &mut self.fonts, update),
                None => Ok(()),
            },
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
        if let Some(notify) = &mut self.notify
            && notify.wants(&ev)
        {
            return notify.event(self.display, &mut self.fonts, ev);
        }
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
            (Panel::Search, Event::Key(k)) => self.search.key(k),
            (Panel::Apps, Event::Key(k)) => self.apps_panel.key(k),
            (Panel::Search, Event::Button { x, y, button, .. }) => {
                self.search.click(i32::from(x), i32::from(y), button)
            }
            (Panel::Apps, Event::Button { x, y, button, .. }) => {
                self.apps_panel.click(i32::from(x), i32::from(y), button)
            }
            (Panel::Search, Event::Motion { x, y, .. }) => {
                self.search.motion(i32::from(x), i32::from(y))
            }
            (_, Event::Expose { .. }) => {
                if let Some(frame) = &self.frame {
                    let popup = match panel {
                        Panel::Search => &self.search_popup,
                        Panel::Apps => &self.apps_popup,
                    };
                    popup.present(self.display, frame)?;
                }
                Action::Nothing
            }
            _ => Action::Nothing,
        };
        // Из «Поиска» Super+A сразу открывает «Программы» и наоборот.
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

/// Сочетание другого окна: Super+Пробел в «Программах», Super+A в «Поиске».
fn switch_to(panel: Panel, k: KeyPress) -> Option<Panel> {
    match (panel, k.key) {
        (Panel::Search, Key::Char('a')) => Some(Panel::Apps),
        (Panel::Apps, Key::Char(' ')) => Some(Panel::Search),
        _ => None,
    }
}

/// Принимает команды через сокет в отдельном потоке.
fn listen(tx: Sender<Msg>) -> Result<()> {
    let path = crate::ipc::socket_path()?;
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
    crate::ipc::start(listener, tx)?;
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

/// Рисует окно с набранным текстом в PNG. Для «Программ», если доступен
/// экран, берёт его размер и снимок для фона; иначе — 1920×1080, ровный фон.
pub fn snapshot(panel: Panel, text: &str, file: &str) -> Result<()> {
    let config = load_config();
    set_scale(&config);
    let start = Instant::now();
    let mut fonts = Fonts::new(&config.font);
    let mut icons = Icons::new(&config.icon_theme);
    let apps = shell_apps::load(&app_settings(&config));
    eprintln!("Загрузка: {:?}", start.elapsed());
    let keys = text.chars().map(|c| KeyPress::plain(Key::Char(c)));
    let start = Instant::now();
    let frame = match panel {
        Panel::Search => {
            let mut s = SearchPanel::new(apps);
            keys.for_each(|k| {
                s.key(k);
            });
            s.render(&mut fonts, &mut icons)
        }
        Panel::Apps => {
            let display = Display::connect().ok();
            let size = display.as_ref().map_or((1920, 1080), |d| {
                let (w, h) = d.screen_size();
                (u32::from(w), u32::from(h))
            });
            let shot = display.and_then(|d| d.capture_screen().ok());
            let mut l = AppsPanel::new(apps, size, grid(&config));
            keys.for_each(|k| {
                l.key(k);
            });
            let k = shell_render::scale();
            let px = |v| shell_render::to_px(v, k);
            let background = backdrop(shot.as_ref(), (px(size.0), px(size.1)));
            l.render(&background, k, &mut fonts, &mut icons)
        }
    };
    eprintln!("Рисование: {:?}", start.elapsed());
    frame.save_png(file)?;
    Ok(())
}
