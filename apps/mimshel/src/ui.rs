//! Окна оболочки на экране — «Поиск» и «Программы»: служба (`mimshel daemon`)
//! или разовый запуск.
//!
//! Служба заранее загружает шрифты, иконки и список программ и ждёт команды
//! через сокет `$XDG_RUNTIME_DIR/mimshel.sock`; поэтому окна открываются сразу.
//! Без службы `mimshel search` / `mimshel apps` делают всё сами —
//! медленнее, но работают. Открыто не больше одного окна: открытие одного
//! закрывает другое.

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use mimshel_apps::{App, Settings, Watcher};
use mimshel_core::config::Config;
use mimshel_icons::Icons;
use mimshel_launcher::{Action, AppsPanel, Grid, SearchPanel, backdrop};
use mimshel_render::Canvas;
use mimshel_text::Fonts;
use mimshel_x11::{Display, Event, Key, KeyPress, Popup};

use crate::desktop_ui::DesktopUi;
use crate::dock_ui::DockUi;
use crate::folder_ui::FolderUi;
use crate::menubar_ui::MenubarUi;
use crate::notify_ui::NotifyUi;
use tiny_skia::Pixmap;

pub(crate) type Result<T> = mimshel_x11::Result<T>;

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
    Clock(String, crate::menubar_ui::Today),
    /// Звук, батарея, «Не беспокоить» изменились.
    Sound(Option<mimshel_menubar::status::Sound>),
    Control(mimshel_menubar::Network, Option<bool>),
    Battery(Option<mimshel_menubar::status::Battery>),
    Dnd(bool),
    /// Значки трея или меню значка.
    Tray(mimshel_dbus::TrayUpdate),
    /// Сеть для окошка сети (Wi-Fi, провод, сети рядом).
    Net(mimshel_menubar::NetState),
    /// Подключение к сети Wi-Fi закончилось: имя и ошибка, если была.
    NetDone(String, Option<String>),
    /// Bluetooth: адаптер и устройства (`None` — адаптера или службы нет).
    Bluetooth(Option<mimshel_dbus::bluetooth::BtState>),
    /// Глобальное меню активного окна.
    AppMenu(mimshel_dbus::AppMenuUpdate),
    /// Уведомление пришло или программа его закрыла.
    Notify(mimshel_dbus::notify::NotifyEvent),
    /// Щёлкнули по колокольчику — открыть или закрыть центр уведомлений.
    ToggleCenter,
    /// Полный набор шрифтов загружен (вместо быстрого, с одним семейством).
    Fonts(Box<Fonts>),
    /// Рабочему столу — из его потоков (обои, миниатюры, выбранный файл).
    Desktop(Box<crate::desktop_ui::DesktopMsg>),
    /// Окнам папок — из их потоков (список файлов).
    Folder(Box<crate::folder_ui::FolderMsg>),
    /// Открыть папку в окне (команда `mimshel folder`, двойной щелчок на столе).
    OpenFolder(PathBuf),
    /// Hot reload (v2): re-read the config file and redraw the
    /// daemon UI in the new language / wallpaper, without exiting.
    /// Sent by the `reload` socket command and by the config watcher.
    Reload,
    /// Показать «Об этом компьютере» (команда `about` через сокет).
    ShowAbout,
    /// Сведения для «Об этом компьютере» собраны.
    About(Box<mimshel_setup::actions::About>),
    /// Перенос брошенных файлов закончен — сообщить источнику (XDND).
    DropFinished {
        ticket: u64,
        success: bool,
    },
}

impl Msg {
    /// Что за сообщение — для журнала замеров.
    fn name(&self) -> String {
        match self {
            Msg::X(ev) => format!("X {ev:?}"),
            Msg::Toggle(_) => "окно".into(),
            Msg::Apps(_) => "программы".into(),
            Msg::Tick => "кадр".into(),
            Msg::Trash(_) => "корзина".into(),
            Msg::Clock(..) => "часы".into(),
            Msg::Sound(_) | Msg::Control(..) | Msg::Battery(_) | Msg::Dnd(_) => "состояние".into(),
            Msg::Tray(_) => "трей".into(),
            Msg::Bluetooth(_) => "bluetooth".into(),
            Msg::Net(_) | Msg::NetDone(..) => "сеть".into(),
            Msg::AppMenu(_) => "меню программы".into(),
            Msg::Notify(_) | Msg::ToggleCenter => "уведомления".into(),
            Msg::Fonts(_) => "шрифты".into(),
            Msg::Desktop(_) => "стол".into(),
            Msg::Folder(_) => "окно папки".into(),
            Msg::OpenFolder(_) => "открыть папку".into(),
            Msg::Reload => "перезагрузка настроек".into(),
            Msg::ShowAbout | Msg::About(_) => "об этом компьютере".into(),
            Msg::DropFinished { .. } => "перенос".into(),
        }
    }
}

/// Настройки из своего файла (`MIMSHEL_CONFIG` — другой файл, для проверок).
/// Файла ещё нет — он создаётся из значений по умолчанию.
pub fn load_config() -> Config {
    match config_path() {
        Some(path) => config_store(path).load(),
        None => Config::default(),
    }
}

/// The settings as the file has them now, for a quick look on the main
/// thread (launching an app: is it set to the discrete GPU?). No lock and
/// no writes: the file is replaced atomically, so a read sees a whole
/// version. A file that does not pass the check goes the full way.
fn quick_config() -> Config {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .filter(|text| mimshel_core::config::toml_error(text).is_none())
        .map(|text| Config::from_toml(&text))
        .unwrap_or_else(load_config)
}

/// The terminal command from settings, ready to run: `mimshel …` (our own
/// terminal) becomes the full path of this program — the session's PATH
/// may not include `~/.local/bin`.
pub fn terminal_command(config: &Config) -> Vec<String> {
    let mut cmd = config.terminal.clone();
    if cmd.first().map(String::as_str) == Some(mimshel_core::names::APP) {
        cmd[0] = crate::guard::own_exe().display().to_string();
    }
    cmd
}

/// Run a command detached from us: stdio to null, its own process group, and
/// a thread waits for it to exit — otherwise finished children would stay
/// zombies in a long-running process (the daemon, a terminal window).
pub fn spawn_detached(mut cmd: Command) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Запустить программу: на видеокарте NVIDIA, если так выбрано в настройках
/// или просит ярлык. Настройки читаются заново — галочку в доке могли
/// переключить только что.
pub fn launch_app(app: &App, terminal: &[String]) -> std::io::Result<u32> {
    let dgpu = quick_config().on_dgpu(&app.id, app.prefers_dgpu);
    app.launch(terminal, dgpu)
}

pub fn config_store(path: PathBuf) -> crate::config_store::Store {
    let good = if mimshel_core::names::flag("CONFIG") {
        None
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
            .map(|p| mimshel_core::names::dir_in(&p).join("config.good.toml"))
    };
    crate::config_store::Store { path, good }
}

/// Файл настроек: из `MIMSHEL_CONFIG` или `~/.config/mimshel/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    mimshel_core::names::env("CONFIG")
        .map(PathBuf::from)
        .or_else(|| {
            let base = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
            Some(mimshel_core::names::dir_in(&base).join("config.toml"))
        })
}

/// Только ярлыки, видимые в меню, — для «Поиска» и «Программ».
fn visible(apps: &[App]) -> Vec<App> {
    apps.iter().filter(|a| !a.no_display).cloned().collect()
}

/// Просит службу открыть папку в окне. `false` — службы нет.
pub fn ask_daemon_folder(path: &std::path::Path) -> bool {
    crate::ipc::socket_path()
        .and_then(UnixStream::connect)
        .and_then(|mut s| s.write_all(format!("folder {}\n", path.display()).as_bytes()))
        .is_ok()
}

/// Просит службу перечитать настройки и перерисоваться (v2 hot reload).
/// `false` — службы нет.
pub fn ask_daemon_reload() -> bool {
    crate::ipc::socket_path()
        .and_then(UnixStream::connect)
        .and_then(|mut s| s.write_all(b"reload\n"))
        .is_ok()
}

/// Просит службу показать «Об этом компьютере». `false` — службы нет.
pub fn ask_daemon_about() -> bool {
    crate::ipc::socket_path()
        .and_then(UnixStream::connect)
        .and_then(|mut s| s.write_all(b"about\n"))
        .is_ok()
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
/// по DPI системы. `MIMSHEL_SCALE` — задать для проверки, не трогая настройки.
pub(crate) fn set_scale(config: &Config) {
    let forced = mimshel_core::names::env("SCALE")
        .and_then(|v| v.into_string().ok())
        .and_then(|v| v.replace(',', ".").parse::<f32>().ok());
    let k = forced
        .or(config.scale)
        .unwrap_or_else(|| mimshel_x11::xft_dpi().map_or(1.0, mimshel_x11::scale_for_dpi));
    mimshel_render::set_scale(k);
    if mimshel_render::scale() != 1.0 {
        eprintln!("Масштаб: {}", mimshel_render::scale());
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
    // Шрифты читаются с диска долго (после загрузки системы — секунды): пусть
    // грузятся заодно со списком программ и значками. Службе для первого показа
    // хватает основного семейства, полный набор она подгрузит после.
    let fonts = {
        let family = config.font.clone();
        std::thread::spawn(move || {
            daemon
                .then(|| Fonts::quick(&family))
                .flatten()
                .unwrap_or_else(|| Fonts::new(&family))
        })
    };
    let (tx, rx) = mpsc::channel();

    if daemon {
        listen(tx.clone())?;
        // Hot reload watcher (v2): if the config file is edited by hand
        // (not via `mimshel set`, which also sends `reload` over the
        // socket), the daemon picks it up within ~2 s without exiting.
        // Reload itself only reads the file, so it cannot retrigger.
        {
            let watch_tx = tx.clone();
            let watch_path = config_path();
            std::thread::spawn(move || {
                let mut last = watch_path
                    .as_ref()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .and_then(|m| m.modified().ok());
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    let now = watch_path
                        .as_ref()
                        .and_then(|p| std::fs::metadata(p).ok())
                        .and_then(|m| m.modified().ok());
                    if now != last {
                        last = now;
                        if watch_tx.send(Msg::Reload).is_err() {
                            break;
                        }
                    }
                }
            });
        }
        // У виртуальной клавиатуры после входа одна раскладка (урок 18): одно
        // нажатие от программы — и значок раскладки пропадал. Вернуть все;
        // строка меню сама заметит смену клавиатуры.
        std::thread::spawn(|| {
            if mimshel_setup::parts::fix_virtual_keyboard() {
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

    // Рабочий стол — первым: обои рисуются в фоне, пока грузится остальное.
    let desktop = if daemon && config.desktop {
        Some(DesktopUi::new(&display, &config, tx.clone())?)
    } else {
        None
    };
    step("рабочий стол");
    let folders = if daemon && config.folders {
        crate::folder_ui::automount();
        Some(FolderUi::new(&display, &config, tx.clone())?)
    } else {
        None
    };

    let settings = Arc::new(Mutex::new(app_settings(&config)));
    let all_apps = mimshel_apps::load_all(&settings.lock().unwrap_or_else(|p| p.into_inner()));
    let apps = visible(&all_apps);
    step("программы");
    let mut icons = Icons::new(&config.icon_theme);
    if daemon {
        watch_apps(Arc::clone(&settings), tx.clone());
    }
    // Своя строка меню — только у службы и только если выбрана вместо панели
    // Xfce (`MIMSHEL_MENUBAR=1` — включить для проверки, не трогая настройки).
    let menubar = if daemon && (config.menubar || mimshel_core::names::flag("MENUBAR")) {
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
    // Свои уведомления — вместо xfce4-notifyd (`MIMSHEL_NOTIFY=1` — для проверки).
    let mut notify = if daemon && (config.notify || mimshel_core::names::flag("NOTIFY")) {
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
    if let Some(n) = &mut notify {
        n.set_language(config.language);
    }
    step("уведомления");
    if daemon {
        // Up after a crash — tell the user through our own banners, or
        // through notify-send when notifications belong to Xfce (once;
        // last-crash is removed when read).
        crate::crash::announce_restart(notify.as_mut(), &mut icons, config.language);
    }
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
        terminal: terminal_command(&config),
        dock,
        desktop,
        folders,
        menubar,
        notify,
        about: crate::about_ui::AboutUi::new(config.language),
        tx: tx.clone(),
        app_settings: settings,
    };
    ui.search.set_language(config.language);
    ui.apps_panel.set_language(config.language);
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
        if let Some(folders) = &ui.folders {
            folders.preload(&mut ui.icons);
        }
        step("значки");
        // Полный набор шрифтов (значки ●, ✓, другие языки в уведомлениях) —
        // в фоне; готов — заменит быстрый.
        let family = config.font.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Fonts(Box::new(Fonts::new(&family))));
        });
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

    let timing = mimshel_core::names::flag("TIMING");
    // Медленный шаг основного потока — в журнал (с SHELL_TIMING): пока он
    // идёт, всё остальное ждёт.
    let slow = |what: &str, t: Instant| {
        if timing && t.elapsed() > std::time::Duration::from_millis(30) {
            eprintln!("Медленно: {what} — {:?}", t.elapsed());
        }
    };
    while let Ok(msg) = rx.recv() {
        let t = Instant::now();
        let what = if timing { msg.name() } else { String::new() };
        ui.message(msg)?;
        slow(&what, t);
        // Всё, что накопилось, пока шёл прошлый кадр, — разом, а док
        // рисуем один раз по итогу. Иначе при быстром движении мыши кадры
        // отстают от событий и док ещё секунды «доигрывает» старые движения.
        while let Ok(msg) = rx.try_recv() {
            let t = Instant::now();
            let what = if timing { msg.name() } else { String::new() };
            ui.message(msg)?;
            slow(&what, t);
        }
        let t = Instant::now();
        ui.flush()?;
        slow("перерисовка", t);
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
    desktop: Option<DesktopUi>,
    folders: Option<FolderUi>,
    menubar: Option<MenubarUi>,
    notify: Option<NotifyUi>,
    about: crate::about_ui::AboutUi,
    /// Для потоков, которые служба запускает сама (сведения «Об этом компьютере»).
    tx: Sender<Msg>,
    /// How app shortcuts are read (`[apps] hide` and `rename`), shared with
    /// the thread that re-reads them when they change (`watch_apps`).
    app_settings: Arc<Mutex<Settings>>,
}

impl Ui<'_> {
    fn toggle(&mut self, panel: Panel) -> Result<()> {
        if let Some(dock) = &mut self.dock {
            dock.close_popups(self.display)?;
        }
        if let Some(desktop) = &mut self.desktop {
            desktop.close_popups(self.display)?;
        }
        if let Some(folders) = &mut self.folders {
            folders.close_popups(self.display)?;
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
    /// Centred on the primary monitor.
    fn search_position(&self, width: u32) -> (i16, i16) {
        let (ax, ay, aw, ah) = self.display.primary_area();
        let x = ax + (aw as i32 - width as i32) / 2;
        let y = ay + (ah as f32 * TOP) as i32;
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
        let timing = mimshel_core::names::flag("TIMING");
        let t = Instant::now();
        self.about
            .flush(self.display, &mut self.fonts, &mut self.icons)?;
        // Окна папок — первыми: их ждут (только что открыли папку).
        if let Some(folders) = &mut self.folders {
            folders.flush(self.display, &mut self.fonts, &mut self.icons)?;
        }
        let folders_time = t.elapsed();
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
        let bar_time = t.elapsed();
        if let Some(dock) = &mut self.dock {
            dock.flush(self.display, &mut self.fonts, &mut self.icons)?;
        }
        let dock_time = t.elapsed();
        if let Some(desktop) = &mut self.desktop {
            desktop.flush(self.display, &mut self.fonts, &mut self.icons)?;
        }
        if timing && t.elapsed() > std::time::Duration::from_millis(30) {
            eprintln!(
                "  перерисовка: окна папок {folders_time:?}, строка меню и уведомления {:?}, \
                 док {:?}, стол {:?}",
                bar_time - folders_time,
                dock_time - bar_time,
                t.elapsed() - dock_time
            );
        }
        Ok(())
    }

    /// Hot reload (v2): re-read config, switch language (and wallpaper)
    /// in every daemon component, and mark them dirty. The main loop
    /// calls `flush()` right after this message, so the bar, dock,
    /// desktop, folder windows and next-opened popups redraw in the
    /// new language without the daemon exiting (PID unchanged).
    /// Already-open terminal windows and already-open popups keep
    /// their old wording until reopened (see docs/i18n.md).
    fn reload(&mut self) -> Result<()> {
        let config = load_config();
        let lang = config.language;
        eprintln!("Reloaded language: {}", lang.name());
        self.search.set_language(lang);
        self.apps_panel.set_language(lang);
        if let Some(dock) = &mut self.dock {
            dock.set_language(lang);
        }
        if let Some(menubar) = &mut self.menubar {
            menubar.set_language(lang);
            menubar.set_hidden_ids(config.menubar_hidden.clone());
        }
        if let Some(notify) = &mut self.notify {
            notify.set_language(lang);
        }
        if let Some(desktop) = &mut self.desktop {
            desktop.set_language(self.display, lang, &config.wallpaper);
        }
        if let Some(folders) = &mut self.folders {
            folders.set_language(self.display, lang)?;
        }
        self.about.set_language(lang);
        self.terminal = terminal_command(&config);
        // The program names we give (`[apps] rename`: «Проводник», "File
        // Manager") follow the language; the list is read again off the main
        // thread and comes back as `Msg::Apps`.
        let new = app_settings(&config);
        let changed = {
            let mut current = self.app_settings.lock().unwrap_or_else(|p| p.into_inner());
            let changed = current.rename != new.rename || current.hide != new.hide;
            if changed {
                *current = new.clone();
            }
            changed
        };
        if changed {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Apps(mimshel_apps::load_all(&new)));
            });
        }
        if self.shown.is_some() {
            self.redraw()?;
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
            Msg::Reload => self.reload(),
            Msg::Toggle(panel) => self.toggle(panel),
            Msg::Apps(apps) => self.set_apps(apps),
            Msg::Fonts(fonts) => {
                self.fonts = *fonts;
                // Новый набор шрифтов ещё не рисовал букв окна папки —
                // нарисовать его один раз невидимо, чтобы первое окно
                // открылось без задержки.
                if let Some(folders) = &self.folders {
                    folders.warm_up(&mut self.fonts, &mut self.icons);
                }
                Ok(())
            }
            Msg::Desktop(msg) => match &mut self.desktop {
                Some(desktop) => desktop.message(self.display, *msg),
                None => Ok(()),
            },
            // Список файлов пришёл — нарисовать окно сразу, не дожидаясь,
            // пока док и строка меню разберут появление нового окна.
            Msg::Folder(msg) => match &mut self.folders {
                Some(folders) => {
                    folders.message(*msg);
                    folders.flush(self.display, &mut self.fonts, &mut self.icons)
                }
                None => Ok(()),
            },
            Msg::ShowAbout => {
                crate::about_ui::AboutUi::request(self.tx.clone());
                Ok(())
            }
            Msg::About(info) => self.about.show(self.display, *info),
            Msg::OpenFolder(path) => match &mut self.folders {
                Some(folders) => folders.open(self.display, path),
                // Свой Проводник выключен — прежний (Thunar).
                None => {
                    crate::folder_ui::open_elsewhere(&path);
                    Ok(())
                }
            },
            Msg::DropFinished { ticket, success } => {
                self.display.finish_drop(ticket, success);
                Ok(())
            }
            Msg::X(ev) => {
                if mimshel_core::names::flag("DEBUG_DOCK") {
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
            Msg::Clock(text, today) => {
                if let Some(menubar) = &mut self.menubar {
                    menubar.set_clock(text, today);
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
            Msg::Net(state) => {
                if let Some(menubar) = &mut self.menubar {
                    menubar.set_net_state(state);
                }
                Ok(())
            }
            Msg::NetDone(ssid, error) => {
                if let Some(menubar) = &mut self.menubar {
                    menubar.net_done(&ssid, error);
                }
                Ok(())
            }
            Msg::Bluetooth(state) => {
                if let Some(menubar) = &mut self.menubar {
                    menubar.set_bluetooth_state(state);
                }
                Ok(())
            }
        }
    }

    fn event(&mut self, ev: Event) -> Result<()> {
        // Открыто окно «Поиск» или «Программы» — клавиатура его (оно её и
        // захватило). Иначе нажатия забирали стол или окно папки, если они
        // были активны, и в поиске ничего не набиралось.
        let to_panel = matches!(ev, Event::Key(_)) && self.shown.is_some();
        // Щелчок по своему же окну (стол, док, окно папки, уведомление)
        // приходит прямо ему, мимо захвата мыши всплывающим окном. Открытое
        // окошко строки меню и центр уведомлений закрываются, как при щелчке
        // по окну другой программы.
        if let Event::Button { window, .. } = ev {
            let on_menubar = self.menubar.as_ref().is_some_and(|m| m.owns(window));
            let on_notify = self.notify.as_ref().is_some_and(|n| n.owns(window));
            if let Some(menubar) = &mut self.menubar
                && menubar.popup_open()
                && !on_menubar
            {
                menubar.close_popup(self.display)?;
            }
            // Щелчок по строке меню центр не закрывает: колокольчик сам его
            // открывает и закрывает.
            if let Some(notify) = &mut self.notify
                && notify.center_open()
                && !on_notify
                && !on_menubar
            {
                notify.close_center(self.display)?;
            }
        }
        // Одно событие может быть нужно и строке меню, и доку (например,
        // сменилось активное окно).
        let mut handled = false;
        if let Some(notify) = &mut self.notify
            && !to_panel
            && notify.wants(&ev)
        {
            return notify.event(self.display, &mut self.fonts, ev);
        }
        if let Some(menubar) = &mut self.menubar
            && !to_panel
            && menubar.wants(self.display, &ev)
        {
            menubar.event(self.display, &mut self.fonts, ev)?;
            handled = true;
        }
        if let Some(dock) = &mut self.dock
            && !to_panel
            && dock.wants(self.display, &ev)
        {
            dock.event(self.display, &mut self.fonts, ev)?;
            handled = true;
        }
        if let Some(desktop) = &mut self.desktop
            && !to_panel
            && desktop.wants(self.display, &ev)
        {
            desktop.event(self.display, &mut self.fonts, ev)?;
            handled = true;
        }
        if !to_panel && self.about.wants(self.display, &ev) {
            return self.about.event(self.display, ev);
        }
        if let Some(folders) = &mut self.folders
            && !to_panel
            && folders.wants(self.display, &ev)
        {
            folders.event(self.display, &mut self.fonts, ev)?;
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
                match launch_app(&app, &self.terminal) {
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
fn watch_apps(settings: Arc<Mutex<Settings>>, tx: Sender<Msg>) {
    let mut watcher = match Watcher::new(&settings.lock().unwrap_or_else(|p| p.into_inner())) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Не удалось следить за ярлыками: {e}");
            return;
        }
    };
    std::thread::spawn(move || {
        while watcher.wait().is_ok() {
            // The settings as they are now: a reload may have changed them.
            let now = settings.lock().unwrap_or_else(|p| p.into_inner()).clone();
            if tx.send(Msg::Apps(mimshel_apps::load_all(&now))).is_err() {
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
    let apps = mimshel_apps::load(&app_settings(&config));
    eprintln!("Загрузка: {:?}", start.elapsed());
    let keys = text.chars().map(|c| KeyPress::plain(Key::Char(c)));
    let start = Instant::now();
    let frame = match panel {
        Panel::Search => {
            let mut s = SearchPanel::new(apps);
            s.set_language(config.language);
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
            l.set_language(config.language);
            keys.for_each(|k| {
                l.key(k);
            });
            let k = mimshel_render::scale();
            let px = |v| mimshel_render::to_px(v, k);
            let background = backdrop(shot.as_ref(), (px(size.0), px(size.1)));
            l.render(&background, k, &mut fonts, &mut icons)
        }
    };
    eprintln!("Рисование: {:?}", start.elapsed());
    frame.save_png(file)?;
    Ok(())
}
