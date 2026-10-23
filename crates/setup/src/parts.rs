//! Включить части оболочки или вернуть прежние программы (`shell apply`):
//! строка меню или панель Xfce, свой док или Plank, свои уведомления или
//! xfce4-notifyd, Spotlight и Launchpad свои или rofi, служба shell. Делает
//! то же, что `macos-look apply panel|dock|notify` и часть `apply menu`.
//!
//! Порядок важен: сначала прежние программы останавливаются, потом
//! перезапускается служба (она занимает имена на шине — трей, меню,
//! уведомления), и только потом запускаются прежние программы, если части
//! возвращены им: служба к этому времени уже отпустила их имена.

use std::path::Path;

use shell_core::config::{Config, DockHide};

use crate::look::{Look, Paths};
use crate::sys::{
    home, keyboard_ok, kill, pgrep, read, remove, run, running, spawn, write_if_changed, xget_list,
    xkb_query, xreset, xset, xset_array,
};

/// Части, которые умеет применять `shell apply`.
pub const APPLY_PARTS: &[&str] = &[
    "menubar",
    "dock",
    "notify",
    "launchers",
    "lock",
    "power",
    "service",
];

/// Метка ярлыков, которые создали мы (как у macos-look — он их тоже узнаёт).
const MANAGED_MARK: &str = "X-Macos-Look=true";
const SESSION: &str = "/sessions/Failsafe";
/// Строка списка запуска сеанса вместо панели Xfce (пустая команда с
/// пометкой — по ней панель возвращается).
const PANEL_OFF: &[&str] = &["true", "macos-look-no-panel"];
const NOTIFYD_UNIT: &str = "xfce4-notifyd.service";
const PLANK_DCONF: &str = "/net/launchpad/plank/docks/dock1/";
const LAUNCHPAD_ICON: &str = "pop-cosmic-launcher";
const SHORTCUTS: &[(&str, &str)] = &[("<Super>space", "spotlight"), ("<Super>a", "launchpad")];

/// Свойство модуля панели Xfce: (имя, тип, значение).
type Prop<'a> = (&'a str, &'a str, &'a str);

/// Какие части применить (`all` — все) и что запустить, когда служба уже
/// перезапущена.
struct Plan<'a> {
    config: &'a Config,
    look: &'a Look,
    paths: Paths,
    later: Vec<Vec<String>>,
}

/// Применить части (пустой список — все). Ошибка — только если нужна служба,
/// а программы shell нет: тогда ничего не трогаем, чтобы не остаться ни с чем.
pub fn apply(config: &Config, look: &Look, parts: &[&str]) -> Result<(), String> {
    let mut plan = Plan {
        config,
        look,
        paths: Paths::new(),
        later: Vec::new(),
    };
    if config.shell_needed() && !plan.paths.shell_bin.exists() {
        return Err(format!(
            "нет {}: соберите и поставьте программу (cargo build --release; \
             install -m 755 target/release/shell ~/.local/bin/)",
            plan.paths.shell_bin.display()
        ));
    }
    let want = |p: &str| parts.is_empty() || parts.contains(&p);
    if want("menubar") {
        plan.menubar();
    }
    if want("dock") {
        plan.dock();
    }
    if want("notify") {
        plan.notify();
    }
    // Служба читает настройки при запуске — перезапускаем всегда.
    plan.service();
    for args in std::mem::take(&mut plan.later) {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        spawn(&args);
    }
    if want("launchers") {
        plan.launchers();
    }
    if want("lock") {
        plan.lock();
    }
    if want("power") {
        power();
    }
    Ok(())
}

impl Plan<'_> {
    fn later(&mut self, args: &[&str]) {
        self.later
            .push(args.iter().map(|a| (*a).to_string()).collect());
    }

    fn menubar(&mut self) {
        tray_apps(&self.paths);
        if self.config.menubar {
            println!("▶ Строка меню (своя, shell)");
            panel_at_login(false);
            quit_panel();
            // blueman рисует значок отдельной программой; без панели она могла
            // выйти. Обе его программы на Python — в командной строке сначала python.
            if !pgrep(r"/blueman-applet(\s|$)").is_empty()
                && pgrep(r"/blueman-tray(\s|$)").is_empty()
            {
                self.later(&["blueman-tray"]);
            }
            return;
        }
        println!("▶ Строка меню — панель Xfce");
        panel_at_login(true);
        quit_panel();
        xreset("xfce4-panel", "/panels");
        xreset("xfce4-panel", "/plugins");
        let c = self.config;
        let height = c.menubar_height.to_string();
        let dark = if self.look.dark() { "true" } else { "false" };
        let font = self.look.get("look", "font").to_string();
        let menu_file = self.paths.app_dir.join("apple.menu");
        let menu_file = menu_file.to_string_lossy();
        // (модуль, [(свойство, тип, значение)]); номер модуля — место в списке.
        let plugins: Vec<(&str, Vec<Prop>)> = vec![
            (
                "applicationsmenu",
                vec![
                    ("custom-menu", "bool", "true"),
                    ("custom-menu-file", "string", &menu_file),
                    ("button-icon", "string", &c.menu_icon),
                    ("button-title", "string", " "),
                    ("show-button-title", "bool", "false"),
                    ("show-menu-icons", "bool", "false"),
                    ("show-generic-names", "bool", "false"),
                    ("show-tooltips", "bool", "false"),
                ],
            ),
            (
                "appmenu",
                vec![
                    ("bold-application-name", "bool", "true"),
                    ("compact-mode", "bool", "false"),
                    ("expand", "bool", "false"),
                ],
            ),
            (
                "separator",
                vec![("style", "uint", "0"), ("expand", "bool", "true")],
            ),
            (
                "systray",
                vec![("square-icons", "bool", "true"), ("icon-size", "int", "16")],
            ),
            (
                "xkb",
                vec![
                    ("display-type", "uint", "1"),
                    ("display-name", "uint", "1"),
                    ("display-scale", "uint", "80"),
                    ("group-policy", "uint", "0"),
                    ("caps-lock-indicator", "bool", "false"),
                ],
            ),
            ("pulseaudio", vec![]),
            ("power-manager-plugin", vec![]),
            ("notification-plugin", vec![]),
            (
                "separator",
                vec![("style", "uint", "0"), ("expand", "bool", "false")],
            ),
            (
                "clock",
                vec![
                    ("mode", "uint", "2"),
                    ("digital-layout", "uint", "3"),
                    ("digital-time-format", "string", &c.clock_format),
                    ("digital-time-font", "string", &font),
                    ("tooltip-format", "string", "%A, %d %B %Y"),
                ],
            ),
        ];
        let ids: Vec<String> = (1..=plugins.len()).map(|i| i.to_string()).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let panel = "/panels/panel-1";
        xset_array("xfce4-panel", "/panels", "int", &["1"]);
        xset("xfce4-panel", "/panels/dark-mode", "bool", dark);
        xset(
            "xfce4-panel",
            &format!("{panel}/position"),
            "string",
            "p=6;x=0;y=0",
        );
        for (key, kind, value) in [
            ("length", "uint", "100"),
            ("position-locked", "bool", "true"),
            ("size", "uint", height.as_str()),
            ("icon-size", "uint", "16"),
            ("enable-struts", "bool", "true"),
        ] {
            xset("xfce4-panel", &format!("{panel}/{key}"), kind, value);
        }
        xset_array("xfce4-panel", &format!("{panel}/plugin-ids"), "int", &ids);
        for (id, (kind, props)) in ids.iter().zip(&plugins) {
            xset(
                "xfce4-panel",
                &format!("/plugins/plugin-{id}"),
                "string",
                kind,
            );
            for (key, t, value) in props {
                xset(
                    "xfce4-panel",
                    &format!("/plugins/plugin-{id}/{key}"),
                    t,
                    value,
                );
            }
        }
        self.later(&["xfce4-panel"]);
    }

    fn dock(&mut self) {
        let plank_autostart = self.paths.autostart.join("plank.desktop");
        if self.config.dock {
            println!("▶ Док (свой, shell)");
            kill("plank");
            remove(&plank_autostart);
            return;
        }
        println!("▶ Док — Plank");
        if crate::sys::which("plank").is_none() {
            println!("✘ Plank не установлен — запустите: macos-look install");
            return;
        }
        let launchers = home().join(".config/plank/dock1/launchers");
        if let Err(e) = std::fs::create_dir_all(&launchers) {
            println!("✘ Не создать {}: {e}", launchers.display());
            return;
        }
        // Значки Plank — его файлы *.dockitem: старые убираем, пишем заново.
        for old in std::fs::read_dir(&launchers)
            .into_iter()
            .flatten()
            .flatten()
        {
            if old.path().extension().is_some_and(|e| e == "dockitem") {
                remove(&old.path());
            }
        }
        let mut items = Vec::new();
        for app in &self.config.dock_apps {
            let Some(desktop) = find_desktop(app) else {
                println!("⚠ Программа «{app}» не найдена, пропускаю");
                continue;
            };
            let name = format!("{app}.dockitem");
            let text = format!(
                "[PlankDockItemPreferences]\nLauncher=file://{}\n",
                desktop.display()
            );
            if let Err(e) = std::fs::write(launchers.join(&name), text) {
                println!("✘ Не записать значок {name}: {e}");
                continue;
            }
            items.push(format!("'{name}'"));
        }
        let theme = self.look.names().theme;
        let theme = if home()
            .join(".local/share/plank/themes")
            .join(&theme)
            .is_dir()
        {
            theme
        } else {
            "Transparent".to_string()
        };
        kill("plank");
        let c = self.config;
        let hide = match c.dock_hide {
            DockHide::Never => "0",
            DockHide::Smart => "1",
            DockHide::Auto => "2",
            DockHide::Maximized => "3",
        };
        for (key, value) in [
            ("dock-items", format!("[{}]", items.join(", "))),
            ("theme", format!("'{theme}'")),
            ("icon-size", c.dock_icon.to_string()),
            ("zoom-enabled", c.dock_zoom.is_some().to_string()),
            ("zoom-percent", c.dock_zoom.unwrap_or(140).to_string()),
            ("hide-mode", hide.to_string()),
            ("position", "'bottom'".into()),
            ("alignment", "'center'".into()),
            ("show-dock-item", "false".into()),
            ("lock-items", "false".into()),
        ] {
            run(&["dconf", "write", &format!("{PLANK_DCONF}{key}"), &value]);
        }
        write(
            &plank_autostart,
            "[Desktop Entry]\nType=Application\nName=Plank\nExec=plank\n\
             X-GNOME-Autostart-enabled=true\nHidden=false\n",
        );
        self.later(&["plank"]);
    }

    fn notify(&mut self) {
        let autostart = self.paths.autostart.join("xfce4-notifyd.desktop");
        if self.config.notify {
            println!("▶ Уведомления (свои, shell)");
            // xfce4-notifyd запускается при входе (автозапуск → systemd) и по
            // требованию через D-Bus — закрываем оба пути, иначе он успеет
            // занять имя раньше службы.
            write(
                &autostart,
                &format!(
                    "[Desktop Entry]\nType=Application\nName=xfce4-notifyd\nHidden=true\n\
                     {MANAGED_MARK}\n"
                ),
            );
            run(&["systemctl", "--user", "mask", NOTIFYD_UNIT]);
            run(&["systemctl", "--user", "stop", NOTIFYD_UNIT]);
            return;
        }
        println!("▶ Уведомления — xfce4-notifyd");
        if read(&autostart).contains(MANAGED_MARK) {
            remove(&autostart);
        }
        run(&["systemctl", "--user", "unmask", NOTIFYD_UNIT]);
        // Запустить, когда служба shell отпустит имя.
        self.later(&["systemctl", "--user", "start", NOTIFYD_UNIT]);
    }

    /// Автозапуск службы и перезапуск (она читает настройки при запуске);
    /// если shell ничего не делает — остановить.
    fn service(&mut self) {
        let autostart = self.paths.autostart.join("shell-daemon.desktop");
        if !self.config.shell_needed() {
            println!("▶ Служба shell не нужна — останавливаю");
            remove(&autostart);
            kill("shell daemon");
            return;
        }
        println!("▶ Служба shell");
        let bin = self.paths.shell_bin.display().to_string();
        write(
            &autostart,
            &desktop_entry(
                "shell: Spotlight, Launchpad и док",
                &format!("{bin} daemon"),
                "",
                &format!("NoDisplay=true\n{MANAGED_MARK}\n"),
            ),
        );
        // Новая служба не займёт сокет, пока старая не вышла.
        kill("shell daemon");
        spawn(&[&bin, "daemon"]);
    }

    /// Экран блокировки: `shell lock` по команде Xfce (меню, перед сном) и
    /// по бездействию — через xss-lock, которому сигнал даёт заставка X.
    fn lock(&mut self) {
        println!("▶ Экран блокировки");
        if crate::sys::which("i3lock").is_none() || crate::sys::which("xss-lock").is_none() {
            println!("✘ Нет i3lock-color или xss-lock — запустите: macos-look install");
            return;
        }
        // Фон рисуем заранее: при блокировке он будет готов сразу.
        let (w, h) = crate::lock::screen_size();
        let bg = crate::lock::background(self.look, w, h);
        println!("  фон: {}", bg.display());
        let bin = self.paths.shell_bin.display().to_string();
        xset(
            "xfce4-session",
            "/general/LockCommand",
            "string",
            &format!("{bin} lock"),
        );
        // xfce4-power-manager гасит экран через заставку X, а xss-lock на её
        // включение (и перед сном системы) запускает блокировку.
        let idle = self.look.get("lock", "idle_minutes");
        let off = self.look.get("lock", "screen_off_minutes");
        let pm = "/xfce4-power-manager";
        for k in ["blank-on-ac", "blank-on-battery"] {
            xset("xfce4-power-manager", &format!("{pm}/{k}"), "int", idle);
        }
        for k in [
            "dpms-on-ac-off",
            "dpms-on-battery-off",
            "dpms-on-ac-sleep",
            "dpms-on-battery-sleep",
        ] {
            xset("xfce4-power-manager", &format!("{pm}/{k}"), "uint", off);
        }
        xset(
            "xfce4-power-manager",
            &format!("{pm}/dpms-enabled"),
            "bool",
            "true",
        );
        let xss = format!("xss-lock --transfer-sleep-lock -- {bin} lock --nofork");
        write(
            &self.paths.autostart.join("macos-look-lock.desktop"),
            &desktop_entry(
                "Блокировка экрана",
                &xss,
                "",
                &format!("NoDisplay=true\n{MANAGED_MARK}\n"),
            ),
        );
        // Штатная заставка Xfce не нужна: она рисовала старое окно пароля.
        write(
            &self.paths.autostart.join("xfce4-screensaver.desktop"),
            &format!(
                "[Desktop Entry]\nType=Application\nName=xfce4-screensaver\nHidden=true\n\
                 {MANAGED_MARK}\n"
            ),
        );
        kill("xfce4-screensaver");
        // Работающий xss-lock не перезапускаем: запущенный не из сеанса (по
        // SSH), он не получит «заблокировать сеанс» рабочего стола (урок 3).
        // Новая команда заработает со следующего входа.
        if !running("xss-lock") {
            let args: Vec<&str> = xss.split(' ').collect();
            spawn(&args);
        }
    }

    /// Сочетания Super+Пробел и Super+A, значок Launchpad в доке и меню яблока.
    fn launchers(&mut self) {
        self.apple_menu();
        let c = self.config;
        let engine = |shell: bool| if shell { "shell" } else { "rofi" };
        println!(
            "▶ Spotlight ({}) и Launchpad ({})",
            engine(c.spotlight),
            engine(c.launchpad)
        );
        let launchpad = Look::shortcut_command(&self.paths, "launchpad", c.launchpad);
        write(
            &self.paths.applications.join("macos-launchpad.desktop"),
            &desktop_entry(
                "Launchpad",
                &launchpad,
                LAUNCHPAD_ICON,
                &format!("NoDisplay=true\n{MANAGED_MARK}\n"),
            ),
        );
        for (keys, action) in SHORTCUTS {
            let shell = c.part(action) == Some(true);
            let command = Look::shortcut_command(&self.paths, action, shell);
            let prop = format!("/commands/custom/{keys}");
            xset("xfce4-keyboard-shortcuts", &prop, "string", &command);
            // «Часики» у курсора ждут окна от самой команды; окно shell
            // открывает служба, и они крутились бы зря.
            xset(
                "xfce4-keyboard-shortcuts",
                &format!("{prop}/startup-notify"),
                "bool",
                if shell { "false" } else { "true" },
            );
        }
    }
}

/// В ВМ — без автоматического сна: из него ВМ не просыпается.
fn power() {
    if !crate::actions::in_vm() {
        return;
    }
    println!("▶ ВМ: без автоматического сна");
    for key in ["inactivity-on-ac", "inactivity-on-battery"] {
        // 14 у «бездействия» значит «никогда».
        xset(
            "xfce4-power-manager",
            &format!("/xfce4-power-manager/{key}"),
            "uint",
            "14",
        );
    }
}

impl Plan<'_> {
    /// Меню яблока: ярлыки `shell action …` и файл меню freedesktop с их
    /// порядком и разделителями (его читают и своя строка меню, и панель Xfce).
    fn apple_menu(&self) {
        let dir = self.paths.app_dir.join("menu");
        let bin = self.paths.shell_bin.display();
        let mut wanted = Vec::new();
        let mut layout = Vec::new();
        for (id, label, separator) in crate::look::APPLE_MENU {
            if *separator {
                layout.push("    <Separator/>".to_string());
            }
            let file = format!("macos-{id}.desktop");
            write(
                &dir.join(&file),
                &desktop_entry(label, &format!("{bin} action {id}"), "", ""),
            );
            layout.push(format!("    <Filename>{file}</Filename>"));
            wanted.push(file);
        }
        // Папку не пересоздаём: панель Xfce следит за ней. Лишнее — убрать.
        for old in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if !wanted
                .iter()
                .any(|w| *w == old.file_name().to_string_lossy())
            {
                remove(&old.path());
            }
        }
        write(
            &self.paths.app_dir.join("apple.menu"),
            &format!(
                "<!DOCTYPE Menu PUBLIC \"-//freedesktop//DTD Menu 1.0//EN\"\n \"http://www.freedesktop.org/standards/menu-spec/menu-1.0.dtd\">\n\
                 <Menu>\n  <Name>Apple</Name>\n  <AppDir>{}</AppDir>\n  \
                 <Include><All/></Include>\n  <Layout>\n{}\n  </Layout>\n</Menu>\n",
                dir.display(),
                layout.join("\n")
            ),
        );
    }
}

fn write(path: &Path, text: &str) {
    if let Err(e) = write_if_changed(path, text) {
        println!("✘ Не записать {}: {e}", path.display());
    }
}

fn desktop_entry(name: &str, exec: &str, icon: &str, extra: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName={name}\nExec={exec}\n\
         Icon={icon}\nTerminal=false\n{extra}"
    )
}

fn find_desktop(app: &str) -> Option<std::path::PathBuf> {
    [
        home().join(".local/share/applications"),
        "/usr/share/applications".into(),
        "/var/lib/flatpak/exports/share/applications".into(),
        home().join(".local/share/flatpak/exports/share/applications"),
    ]
    .into_iter()
    .map(|d| d.join(format!("{app}.desktop")))
    .find(|f| f.exists())
}

/// Строка списка запуска сеанса с такой командой.
fn session_slot(command: &[&str]) -> Option<usize> {
    let count: usize = crate::sys::xget("xfce4-session", &format!("{SESSION}/Count"))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (0..count)
        .find(|i| xget_list("xfce4-session", &format!("{SESSION}/Client{i}_Command")) == command)
}

/// Запускать ли панель Xfce при входе: её строку в списке сеанса меняем на
/// пустую команду или сбрасываем к системной (там xfce4-panel).
fn panel_at_login(on: bool) {
    if on {
        if let Some(i) = session_slot(PANEL_OFF) {
            xreset("xfce4-session", &format!("{SESSION}/Client{i}_Command"));
        }
    } else if let Some(i) = session_slot(&["xfce4-panel"]) {
        xset_array(
            "xfce4-session",
            &format!("{SESSION}/Client{i}_Command"),
            "string",
            PANEL_OFF,
        );
    }
}

fn quit_panel() {
    if !running("xfce4-panel") {
        return;
    }
    run(&["xfce4-panel", "--quit"]);
    for _ in 0..50 {
        if !running("xfce4-panel") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    keep_keyboard_layouts();
}

/// Панель Xfce при выходе иногда оставляет одну раскладку (урок 15) — вернуть все.
fn keep_keyboard_layouts() {
    if keyboard_ok() {
        return;
    }
    restore_layouts();
    println!("⚠ Раскладки клавиатуры восстановлены после остановки панели");
}

/// Вернуть все раскладки виртуальной клавиатуре XTEST, если у неё их нет
/// (после входа — урок 18). Служба делает это при запуске; `true` — вернула.
pub fn fix_virtual_keyboard() -> bool {
    if crate::sys::virtual_keyboard_ok() {
        return false;
    }
    restore_layouts();
    true
}

/// Применить раскладки из настроек X заново (`setxkbmap` — всем клавиатурам).
fn restore_layouts() {
    let rules = xkb_query("rules");
    let model = xkb_query("model");
    let layout = xkb_query("layout");
    let options = xkb_query("options");
    let mut args = vec![
        "setxkbmap",
        "-rules",
        if rules.is_empty() { "evdev" } else { &rules },
        "-model",
        if model.is_empty() { "pc105" } else { &model },
        "-layout",
        &layout,
        "-option",
        "",
    ];
    for opt in options.split(',').filter(|o| !o.is_empty()) {
        args.extend(["-option", opt]);
    }
    run(&args);
}

/// Значки трея — современным способом (StatusNotifierItem): его понимают и
/// своя строка меню, и панель Xfce. nm-applet для этого нужен `--indicator`.
fn tray_apps(paths: &Paths) {
    let system = Path::new("/etc/xdg/autostart/nm-applet.desktop");
    if let Ok(text) = std::fs::read_to_string(system) {
        let mut text: String = text
            .lines()
            .map(|l| {
                if l.trim_end() == "Exec=nm-applet" {
                    "Exec=nm-applet --indicator"
                } else {
                    l
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !text.contains(MANAGED_MARK) {
            text = format!("{}\n{MANAGED_MARK}", text.trim_end_matches('\n'));
        }
        text.push('\n');
        write(&paths.autostart.join("nm-applet.desktop"), &text);
    }
    let running = crate::sys::output(&[
        "pgrep",
        "-u",
        &crate::sys::uid().to_string(),
        "-a",
        "-x",
        "nm-applet",
    ])
    .unwrap_or_default();
    if !running.trim().is_empty() && !running.contains("--indicator") {
        kill("nm-applet");
        std::thread::sleep(std::time::Duration::from_millis(500));
        spawn(&["nm-applet", "--indicator"]);
    }
}
