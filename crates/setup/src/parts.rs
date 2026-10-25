//! Включить части оболочки или вернуть прежние программы (`mimshel apply`):
//! строка меню или панель Xfce, свой док или Plank, свои уведомления или
//! xfce4-notifyd, окна «Поиск» и «Программы» свои или rofi, оформление, обои,
//! экран блокировки, служба Mimshel.
//!
//! Порядок важен: сначала прежние программы останавливаются, потом
//! перезапускается служба (она занимает имена на шине — трей, меню,
//! уведомления), и только потом запускаются прежние программы, если части
//! возвращены им: служба к этому времени уже отпустила их имена.

use std::path::Path;

use mimshel_core::config::{Config, DockHide};

use crate::paths::{APPS_SHORTCUT, Paths, SHORTCUTS, shortcut_command};
use crate::sys::{
    DESKTOP_OFF, MANAGED_MARK, OLD_DESKTOP_OFF, OLD_PANEL_OFF, PANEL_OFF, is_managed,
};
use crate::sys::{
    home, keyboard_ok, kill, pgrep, read, remove, run, running, spawn, write_if_changed, xget_list,
    xkb_query, xreset, xset, xset_array,
};

/// Части, которые умеет применять `mimshel apply`.
pub const APPLY_PARTS: &[&str] = &[
    "menubar",
    "dock",
    "notify",
    "desktop",
    "folders",
    "launchers",
    "lock",
    "power",
    "icons",
    "look",
    "wallpaper",
    "service",
];

const SESSION: &str = "/sessions/Failsafe";
const NOTIFYD_UNIT: &str = "xfce4-notifyd.service";
/// Автозапуск xss-lock (блокировка по бездействию и перед сном).
pub const LOCK_AUTOSTART: &str = "mimshel-lock.desktop";
/// Он же под прежними именами (до переименования, от macos-look) —
/// заменяется новым.
const OLD_LOCK_AUTOSTARTS: &[&str] = &["shell-lock.desktop", "macos-look-lock.desktop"];
/// Автозапуск службы и он же под прежним именем.
pub const DAEMON_AUTOSTART: &str = "mimshel.desktop";
const OLD_DAEMON_AUTOSTART: &str = "shell-daemon.desktop";
const PLANK_DCONF: &str = "/net/launchpad/plank/docks/dock1/";
/// Значок окна «Программы» в доке и его ярлык (имя ярлыка — от прежнего
/// Launchpad: под ним значок закреплён в доке у тех, кто ставил раньше).
const APPS_ICON: &str = "pop-cosmic-launcher";
pub const APPS_DESKTOP: &str = "macos-launchpad";
/// Ярлык своего Проводника (он же класс его окон) и прежнее имя.
const FOLDER_DESKTOP: &str = "mimshel-folder";
const OLD_FOLDER_DESKTOP: &str = "shell-folder";

/// Свойство модуля панели Xfce: (имя, тип, значение).
type Prop<'a> = (&'a str, &'a str, &'a str);

/// Какие части применить (`all` — все) и что запустить, когда служба уже
/// перезапущена.
struct Plan<'a> {
    config: &'a Config,
    paths: Paths,
    later: Vec<Vec<String>>,
}

/// Применить части (пустой список — все). Ошибка — только если нужна служба,
/// а программы Mimshel нет: тогда ничего не трогаем, чтобы не остаться ни с чем.
pub fn apply(config: &Config, parts: &[&str]) -> Result<(), String> {
    let mut plan = Plan {
        config,
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
    if want("desktop") {
        plan.desktop();
    }
    if want("folders") {
        plan.folders();
    }
    // Значки — до перезапуска службы: тему она читает при запуске.
    if want("icons") {
        plan.icons();
    }
    if want("look") {
        plan.look();
    }
    if want("wallpaper") {
        plan.wallpaper();
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
        install_logo();
        if self.config.menubar {
            println!("▶ Строка меню (своя, Mimshel)");
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
        let dark = if c.gtk_theme.to_lowercase().contains("light") {
            "false"
        } else {
            "true"
        };
        let font = format!("{} 10", c.font);
        let menu_file = self.paths.main_menu();
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
            println!("▶ Док (свой, Mimshel)");
            kill("plank");
            remove(&plank_autostart);
            return;
        }
        println!("▶ Док — Plank");
        if crate::sys::which("plank").is_none() {
            println!("✘ Plank не установлен — поставьте: sudo pacman -S plank");
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
        // Тема Plank с тем же именем, что тема окон, если есть; иначе прозрачная.
        let theme = if home()
            .join(".local/share/plank/themes")
            .join(&self.config.gtk_theme)
            .is_dir()
        {
            self.config.gtk_theme.clone()
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
            println!("▶ Уведомления (свои, Mimshel)");
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
        if is_managed(&read(&autostart)) {
            remove(&autostart);
        }
        run(&["systemctl", "--user", "unmask", NOTIFYD_UNIT]);
        // Запустить, когда служба Mimshel отпустит имя.
        self.later(&["systemctl", "--user", "start", NOTIFYD_UNIT]);
    }

    /// Рабочий стол: свой (обои и меню рисует служба) или xfdesktop. Как с
    /// панелью: строку xfdesktop в списке сеанса меняем на пустую команду —
    /// иначе он запустится при входе и закроет наш стол.
    fn desktop(&mut self) {
        if self.config.desktop {
            println!("▶ Рабочий стол (свой, Mimshel)");
            if let Some(i) = session_slot(&["xfdesktop"]).or_else(|| old_slot(OLD_DESKTOP_OFF)) {
                xset_array(
                    "xfce4-session",
                    &format!("{SESSION}/Client{i}_Command"),
                    "string",
                    DESKTOP_OFF,
                );
            }
            // --quit, а не kill: убитый xfdesktop сеанс Xfce запустил бы снова.
            if running("xfdesktop") {
                run(&["xfdesktop", "--quit"]);
            }
            return;
        }
        println!("▶ Рабочий стол — xfdesktop");
        if let Some(i) = session_slot(DESKTOP_OFF).or_else(|| old_slot(OLD_DESKTOP_OFF)) {
            xreset("xfce4-session", &format!("{SESSION}/Client{i}_Command"));
        }
        if !running("xfdesktop") {
            self.later(&["xfdesktop"]);
        }
    }

    /// Окна папок: свой Проводник (ярлык `mimshel-folder.desktop` открывает
    /// папки через службу и стоит программой для папок) или Thunar.
    fn folders(&mut self) {
        let file = self
            .paths
            .applications
            .join(format!("{FOLDER_DESKTOP}.desktop"));
        let old = self
            .paths
            .applications
            .join(format!("{OLD_FOLDER_DESKTOP}.desktop"));
        if is_managed(&read(&old)) {
            remove(&old);
        }
        if self.config.folders {
            println!("▶ Окна папок — свой Проводник");
            write(
                &file,
                &desktop_entry(
                    "Проводник",
                    &format!("{} folder %U", self.paths.shell_bin.display()),
                    "system-file-manager",
                    &format!(
                        "Comment=Папки и файлы\nCategories=System;FileTools;FileManager;\n\
                         MimeType=inode/directory;\nStartupWMClass={FOLDER_DESKTOP}\n\
                         {MANAGED_MARK}\n"
                    ),
                ),
            );
            run(&[
                "xdg-mime",
                "default",
                &format!("{FOLDER_DESKTOP}.desktop"),
                "inode/directory",
            ]);
        } else {
            println!("▶ Окна папок — Thunar");
            remove(&file);
            run(&["xdg-mime", "default", "thunar.desktop", "inode/directory"]);
        }
        if crate::sys::which("update-desktop-database").is_some() {
            run(&[
                "update-desktop-database",
                &self.paths.applications.display().to_string(),
            ]);
        }
    }

    /// Автозапуск службы и перезапуск (она читает настройки при запуске);
    /// если Mimshel ничего не делает — остановить.
    fn service(&mut self) {
        let autostart = self.paths.autostart.join(DAEMON_AUTOSTART);
        let old = self.paths.autostart.join(OLD_DAEMON_AUTOSTART);
        if is_managed(&read(&old)) {
            remove(&old);
        }
        compat_link(&self.paths.shell_bin);
        if !self.config.shell_needed() {
            println!("▶ Служба Mimshel не нужна — останавливаю");
            remove(&autostart);
            crate::sys::kill_daemon();
            return;
        }
        println!("▶ Служба Mimshel");
        let bin = self.paths.shell_bin.display().to_string();
        write(
            &autostart,
            &desktop_entry(
                "Mimshel: строка меню, док, стол, «Поиск» и «Программы»",
                &format!("{bin} daemon"),
                "",
                &format!("NoDisplay=true\n{MANAGED_MARK}\n"),
            ),
        );
        // Новая служба не займёт сокет, пока старая не вышла.
        crate::sys::kill_daemon();
        spawn(&[&bin, "daemon"]);
    }

    /// Экран блокировки: `mimshel lock` по команде Xfce (меню, перед сном) и
    /// по бездействию — через xss-lock, которому сигнал даёт заставка X.
    fn lock(&mut self) {
        println!("▶ Экран блокировки");
        if crate::sys::which("xss-lock").is_none() {
            println!("✘ Нет xss-lock — запустите ./install.sh в папке проекта");
            return;
        }
        // Фон рисуем заранее: при блокировке он будет готов сразу.
        let (w, h) = crate::lock::screen_size();
        match crate::lock::background(&self.config.wallpaper, w, h) {
            Some(bg) => println!("  фон: {}", bg.display()),
            None => println!("  ⚠ фон не нарисован — блокировка будет на тёмном фоне"),
        }
        let bin = self.paths.shell_bin.display().to_string();
        xset(
            "xfce4-session",
            "/general/LockCommand",
            "string",
            &format!("{bin} lock"),
        );
        // Super+L — как в большинстве систем; Ctrl+Alt+L у Xfce остаётся.
        // xflock4 сам вызывает LockCommand.
        xset(
            "xfce4-keyboard-shortcuts",
            &format!("/commands/custom/{}", crate::lock::LOCK_SHORTCUT),
            "string",
            crate::lock::LOCK_SHORTCUT_COMMAND,
        );
        // xfce4-power-manager гасит экран через заставку X, а xss-lock на её
        // включение (и перед сном системы) запускает блокировку.
        let idle = self.config.lock_idle.to_string();
        let off = self.config.lock_screen_off.to_string();
        let pm = "/xfce4-power-manager";
        for k in ["blank-on-ac", "blank-on-battery"] {
            xset("xfce4-power-manager", &format!("{pm}/{k}"), "int", &idle);
        }
        for k in [
            "dpms-on-ac-off",
            "dpms-on-battery-off",
            "dpms-on-ac-sleep",
            "dpms-on-battery-sleep",
        ] {
            xset("xfce4-power-manager", &format!("{pm}/{k}"), "uint", &off);
        }
        xset(
            "xfce4-power-manager",
            &format!("{pm}/dpms-enabled"),
            "bool",
            "true",
        );
        let xss = format!("xss-lock --transfer-sleep-lock -- {bin} lock --nofork");
        write(
            &self.paths.autostart.join(LOCK_AUTOSTART),
            &desktop_entry(
                "Блокировка экрана",
                &xss,
                "",
                &format!("NoDisplay=true\n{MANAGED_MARK}\n"),
            ),
        );
        for name in OLD_LOCK_AUTOSTARTS {
            let old = self.paths.autostart.join(name);
            if is_managed(&read(&old)) {
                remove(&old);
            }
        }
        // Штатная заставка Xfce не нужна: она рисовала старое окно пароля.
        write(
            &self.paths.autostart.join("xfce4-screensaver.desktop"),
            &format!(
                "[Desktop Entry]\nType=Application\nName=xfce4-screensaver\nHidden=true\n\
                 {MANAGED_MARK}\n"
            ),
        );
        // …и по требованию через D-Bus (как xfce4-notifyd) — тоже закрыть.
        write(
            &crate::lock::screensaver_service(),
            &crate::lock::screensaver_service_text(),
        );
        // dbus-broker (шина в Arch) сам папки служб не перечитывает.
        run(&[
            "dbus-send",
            "--session",
            "--type=method_call",
            "--dest=org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.ReloadConfig",
        ]);
        kill("xfce4-screensaver");
        // Работающий xss-lock не перезапускаем: запущенный не из сеанса (по
        // SSH), он не получит «заблокировать сеанс» рабочего стола (урок 3).
        // Новая команда заработает со следующего входа.
        if !running("xss-lock") {
            let args: Vec<&str> = xss.split(' ').collect();
            spawn(&args);
        }
    }

    /// Сочетания Super+Пробел и Super+A, значок «Программы» в доке и меню яблока.
    fn launchers(&mut self) {
        self.apple_menu();
        let c = self.config;
        let engine = |ours: bool| if ours { "Mimshel" } else { "rofi" };
        println!(
            "▶ Поиск ({}) и Программы ({})",
            engine(c.search_panel),
            engine(c.apps_panel)
        );
        let apps = shortcut_command(&self.paths, &APPS_SHORTCUT, c.apps_panel);
        // Имя файла прежнее: под ним значок закреплён в доке (`[dock] apps`).
        write(
            &self
                .paths
                .applications
                .join(format!("{APPS_DESKTOP}.desktop")),
            &desktop_entry(
                "Программы",
                &apps,
                APPS_ICON,
                &format!("NoDisplay=true\n{MANAGED_MARK}\n"),
            ),
        );
        // Список «тип файла → программа» для своих ярлыков: без него копии
        // ярлыков в ~/.local/share/applications заслоняют системные, и папки
        // нечем открыть (gio open).
        if crate::sys::which("update-desktop-database").is_some() {
            run(&[
                "update-desktop-database",
                &self.paths.applications.display().to_string(),
            ]);
        }
        for s in SHORTCUTS {
            let shell = c.part(s.part) == Some(true);
            let command = shortcut_command(&self.paths, s, shell);
            let prop = format!("/commands/custom/{}", s.keys);
            xset("xfce4-keyboard-shortcuts", &prop, "string", &command);
            // «Часики» у курсора ждут окна от самой команды; окно Mimshel
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

/// Надстройка над Colloid: наши правки значков поверх набора, обновления
/// Colloid их не затирают. Сейчас — знак главного меню вместо яблока Apple
/// (`start-here` в Colloid — яблоко).
pub const OVERLAY_THEME: &str = "Colloid-mimshel";
/// Надстройка под прежним именем (до переименования проекта) — её и её
/// готовые PNG рисовал `shell apply icons`; при новом `apply icons` удаляются.
const OLD_OVERLAY_THEMES: &[&str] = &["Colloid-shell", "Colloid-shell-png"];
const OVERLAY_BASE: &str = "Colloid-Dark";
const LOGO_SYMBOLIC_SVG: &str = include_str!("../data/mimshel-logo-symbolic.svg");
/// Значки, которые надстройка берёт из других установленных наборов: (тема,
/// начало имени). Значок сети nm-applet из Colloid неудобен — как в WhiteSur.
const BORROWED: &[(&str, &str)] = &[("WhiteSur-dark", "nm-")];

/// Свой знак меню (планета с кольцом вместо яблока) — в общую тему значков
/// hicolor: его находят и своя строка меню, и панель Xfce.
pub const LOGO_NAME: &str = "mimshel-logo";
const OLD_LOGO_NAME: &str = "shell-logo";
const LOGO_SVG: &str = include_str!("../data/mimshel-logo.svg");

pub fn logo_path() -> std::path::PathBuf {
    home().join(format!(
        ".local/share/icons/hicolor/scalable/apps/{LOGO_NAME}.svg"
    ))
}

fn install_logo() {
    write(&logo_path(), LOGO_SVG);
    // Прежний знак (до переименования проекта) — наш, больше не нужен.
    remove(&logo_path().with_file_name(format!("{OLD_LOGO_NAME}.svg")));
}

/// Прежняя команда `~/.local/bin/shell` — ссылкой на новую: ею ещё вызывают
/// сочетания клавиш, команда блокировки и автозапуск, поставленные до
/// переименования (пока `apply` их не перепишет), и записки людей.
fn compat_link(bin: &Path) {
    let old = bin.with_file_name(mimshel_core::names::OLD);
    if std::fs::read_link(&old).is_ok_and(|t| t == bin.file_name().unwrap_or_default()) {
        return;
    }
    // Там прежняя программа (наша) или ничего — заменить ссылкой.
    if old.exists() || old.is_symlink() {
        let _ = std::fs::remove_file(&old);
    }
    if let Some(name) = bin.file_name()
        && let Err(e) = std::os::unix::fs::symlink(name, &old)
    {
        println!("  ⚠ не создать {} → {}: {e}", old.display(), bin.display());
    }
}

/// Скопировать в надстройку `overlay` значки темы `src`, имена которых
/// начинаются с `prefix` (ссылки — их содержимым). Вернуть папки, куда что-то
/// легло, с описанием из `index.theme` темы-источника (размер, тип).
fn borrow(src: &Path, prefix: &str, overlay: &Path) -> Vec<(String, String)> {
    let index = read(&src.join("index.theme"));
    let section = |name: &str| -> Option<String> {
        let mut lines = index
            .lines()
            .skip_while(|l| l.trim() != format!("[{name}]"));
        lines.next()?;
        let body: Vec<&str> = lines
            .take_while(|l| !l.trim_start().starts_with('['))
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .collect();
        Some(body.join("\n") + "\n")
    };
    let dirs = index
        .lines()
        .find_map(|l| l.strip_prefix("Directories="))
        .unwrap_or("");
    let mut out = Vec::new();
    for sub in dirs.split(',').map(str::trim).filter(|d| !d.is_empty()) {
        let Ok(entries) = std::fs::read_dir(src.join(sub)) else {
            continue;
        };
        let mut copied = false;
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with(prefix) {
                continue;
            }
            // read идёт по ссылкам: копируем саму картинку.
            if let Ok(svg) = std::fs::read_to_string(e.path()) {
                write(&overlay.join(sub).join(&name), &svg);
                copied = true;
            }
        }
        if copied && let Some(body) = section(sub) {
            out.push((sub.to_string(), body));
        }
    }
    out
}

/// «Не засыпать при бездействии» у xfce4-power-manager 4.20 (минуты; 0 —
/// никогда). Не 14: так было «никогда» в старых версиях, а в 4.20 это сон
/// через 14 минут — ВМ засыпала и не просыпалась, будто зависла.
pub const SLEEP_NEVER: &str = "0";

/// В ВМ — без автоматического сна: из него ВМ не просыпается.
fn power() {
    if !crate::actions::in_vm() {
        return;
    }
    println!("▶ ВМ: без автоматического сна");
    for key in ["inactivity-on-ac", "inactivity-on-battery"] {
        xset(
            "xfce4-power-manager",
            &format!("/xfce4-power-manager/{key}"),
            "uint",
            SLEEP_NEVER,
        );
    }
}

/// Мониторы в пикселях: (x, y, ширина, высота) из `xrandr --listmonitors`
/// (строки вида `0: +*eDP-1 2560/344x1440/194+0+0  eDP-1`). Пусто — xrandr
/// не ответил.
pub fn monitor_rects() -> Vec<(i32, i32, u32, u32)> {
    parse_monitors(&crate::sys::output_any(&["xrandr", "--listmonitors"]))
}

fn parse_monitors(text: &str) -> Vec<(i32, i32, u32, u32)> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let geo = l.split_whitespace().nth(2)?;
            let (w, rest) = geo.split_once('x')?;
            let w: u32 = w.split('/').next()?.parse().ok()?;
            let (h, pos) = rest.split_once('+')?;
            let h: u32 = h.split('/').next()?.parse().ok()?;
            let (x, y) = pos.split_once('+')?;
            Some((x.parse().ok()?, y.parse().ok()?, w, h))
        })
        .collect()
}

/// Где хранятся свои обои `style`, нарисованные под `w`×`h` пикселей.
pub fn wallpaper_path(style: mimshel_wallpaper::Style, w: u32, h: u32) -> std::path::PathBuf {
    mimshel_core::names::dir_in(&home().join(".local/share"))
        .join(format!("wallpapers/{}-{w}x{h}.png", style.name()))
}

/// Файл обоев: свои (`space`, `waves`, `aurora`) рисуются под экран `w`×`h`
/// один раз и хранятся в `~/.local/share/mimshel/wallpapers`; иначе — путь к
/// картинке как есть. Нарисовать не вышло или файла нет — `None`.
pub fn wallpaper_file(value: &str, w: i64, h: i64) -> Option<std::path::PathBuf> {
    let Some(style) = mimshel_wallpaper::Style::parse(value) else {
        let path = std::path::PathBuf::from(value);
        return path.is_file().then_some(path);
    };
    let (w, h) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    let out = wallpaper_path(style, w, h);
    if out.exists() {
        return Some(out);
    }
    let png = mimshel_wallpaper::draw(style, w, h)?.encode_png().ok()?;
    std::fs::create_dir_all(out.parent()?).ok()?;
    let tmp = out.with_extension("png.tmp");
    std::fs::write(&tmp, png).ok()?;
    std::fs::rename(&tmp, &out).ok()?;
    Some(out)
}

impl Plan<'_> {
    /// Тема окон и программ (GTK, xfwm4, GTK4) и курсор.
    fn look(&self) {
        let c = self.config;
        println!(
            "▶ Оформление: окна {}, курсор {}",
            c.gtk_theme, c.cursor_theme
        );
        let theme_dir = self.paths.themes.join(&c.gtk_theme);
        if !theme_dir.join("index.theme").exists() && !theme_dir.join("gtk-3.0").is_dir() {
            println!(
                "✘ Нет темы {} в {} — поставьте её (Colloid: \
                 https://github.com/vinceliuice/Colloid-gtk-theme)",
                c.gtk_theme,
                self.paths.themes.display()
            );
            return;
        }
        xset("xsettings", "/Net/ThemeName", "string", &c.gtk_theme);
        xset("xfwm4", "/general/theme", "string", &c.gtk_theme);
        xset(
            "xsettings",
            "/Gtk/CursorThemeName",
            "string",
            &c.cursor_theme,
        );
        xset("xsettings", "/Gtk/CursorThemeSize", "int", "24");
        // Шрифт и сглаживание, как в macOS: мягкие края, лёгкая подгонка.
        let font = format!("{} 10", c.font);
        xset("xsettings", "/Gtk/FontName", "string", &font);
        xset("xsettings", "/Xft/Antialias", "int", "1");
        xset("xsettings", "/Xft/HintStyle", "string", "hintslight");
        xset("xsettings", "/Xft/RGBA", "string", "rgb");
        // Меню программ — в строке меню. Модуль appmenu подключается при входе
        // (GTK_MODULES); через xsettings /Gtk/Modules — нельзя, роняет
        // запущенные программы (урок 4).
        xreset("xsettings", "/Gtk/Modules");
        xset("xsettings", "/Gtk/ShellShowsMenubar", "bool", "true");
        // Терминалу нужна своя строка меню — иначе отдавать наверх нечего.
        xset("xfce4-terminal", "/misc-menubar-default", "bool", "true");
        // Окна: заголовок по центру, кнопки слева, тени и прозрачность.
        let title = format!("{} Semi-Bold 10", c.font);
        xset("xfwm4", "/general/title_font", "string", &title);
        xset("xfwm4", "/general/title_alignment", "string", "center");
        xset("xfwm4", "/general/button_layout", "string", "CHM|");
        xset("xfwm4", "/general/use_compositing", "bool", "true");
        xset("xfwm4", "/general/show_dock_shadow", "bool", "false");
        // Курсор для X11 и программ не на GTK — тема «default».
        write(
            &self.paths.icons.join("default/index.theme"),
            &format!("[Icon Theme]\nInherits={}\n", c.cursor_theme),
        );
        // Ссылки GTK4 принадлежат Shell только при наличии записи о них.
        crate::gtk_links::apply(&theme_dir);
        // Рамка выделения в Проводнике — без обрывков скругления.
        // Проводник работает службой (`thunar --daemon`): закрытые окна её не
        // останавливают, и новые правила он прочтёт только после `thunar -q`.
        if crate::gtk_fixes::apply() {
            println!("  Проводник увидит изменения после перезапуска: thunar -q");
        }
        crate::terminal::apply(&c.terminal);
    }

    /// Обои на все мониторы (и фон блокировки — из них же).
    fn wallpaper(&self) {
        let (w, h) = crate::lock::screen_size();
        println!("▶ Обои: {} ({w}x{h})", self.config.wallpaper);
        let Some(file) = wallpaper_file(&self.config.wallpaper, w, h) else {
            println!(
                "✘ Обои «{}» не нарисованы и не найдены",
                self.config.wallpaper
            );
            return;
        };
        let file = file.display().to_string();
        for m in crate::sys::monitors() {
            let base = format!("/backdrop/screen0/{m}/workspace0");
            xset(
                "xfce4-desktop",
                &format!("{base}/last-image"),
                "string",
                &file,
            );
            // 5 — растянуть с сохранением пропорций на весь экран.
            xset("xfce4-desktop", &format!("{base}/image-style"), "int", "5");
            xset("xfce4-desktop", &format!("{base}/color-style"), "int", "0");
        }
        match crate::lock::background(&self.config.wallpaper, w, h) {
            Some(bg) => println!("  фон блокировки: {}", bg.display()),
            None => println!("  ⚠ фон блокировки не нарисован"),
        }
    }

    /// Тема значков для всех программ (xsettings) и, если выбрана наша
    /// надстройка над Colloid, — сама надстройка.
    fn icons(&self) {
        let theme = &self.config.icon_theme;
        println!("▶ Значки: {theme}");
        if theme == OVERLAY_THEME {
            for old in OLD_OVERLAY_THEMES {
                let dir = self.paths.icons.join(old);
                if dir.join("index.theme").exists() {
                    let _ = std::fs::remove_dir_all(&dir);
                }
            }
            let base = self.paths.icons.join(OVERLAY_BASE);
            if !base.join("index.theme").exists() {
                println!(
                    "✘ Нет набора {OVERLAY_BASE} в {} — поставьте Colloid: \
                     https://github.com/vinceliuice/Colloid-icon-theme",
                    self.paths.icons.display()
                );
                return;
            }
            let dir = self.paths.icons.join(OVERLAY_THEME);
            let mut dirs = vec!["places/scalable".to_string()];
            let mut sections = String::from(
                "[places/scalable]\nContext=Places\nSize=48\nMinSize=8\nMaxSize=512\n\
                 Type=Scalable\n",
            );
            for (theme, prefix) in BORROWED {
                for (sub, section) in borrow(&self.paths.icons.join(theme), prefix, &dir) {
                    if !dirs.contains(&sub) {
                        sections.push_str(&format!("\n[{sub}]\n{section}"));
                        dirs.push(sub);
                    }
                }
            }
            write(
                &dir.join("index.theme"),
                &format!(
                    "[Icon Theme]\nName={OVERLAY_THEME}\n\
                     Comment=Colloid с правками shell (свой знак вместо яблока)\n\
                     Inherits={OVERLAY_BASE},hicolor,breeze\n\
                     Directories={}\n\n{sections}",
                    dirs.join(",")
                ),
            );
            // Одна залитая фигура: GTK перекрашивает одноцветные значки,
            // заливая все фигуры, — маски и линии при этом ломаются.
            for name in ["start-here", "start-here-symbolic"] {
                write(
                    &dir.join(format!("places/scalable/{name}.svg")),
                    LOGO_SYMBOLIC_SVG,
                );
            }
            let png = crate::png_icons::theme_name(OVERLAY_THEME);
            if !self.config.png_icons {
                // Выключили — тема снова SVG, готовые картинки не нужны.
                xset("xsettings", "/Net/IconThemeName", "string", theme);
                let _ = std::fs::remove_dir_all(self.paths.icons.join(&png));
                return;
            }
            // Программам GTK — готовые PNG: SVG они грузят в ~10 раз дольше
            // (урок 24). Сама служба рисует SVG сама и берёт обычную тему.
            let start = std::time::Instant::now();
            let apps: Vec<String> = mimshel_apps::load_all(&mimshel_apps::Settings::from_env())
                .into_iter()
                .filter_map(|a| a.icon)
                .collect();
            let stats = crate::png_icons::build(
                &self.paths.icons,
                OVERLAY_THEME,
                &[OVERLAY_THEME, OVERLAY_BASE],
                &apps,
            );
            if crate::sys::which("gtk-update-icon-cache").is_some() {
                run(&[
                    "gtk-update-icon-cache",
                    "-f",
                    "-q",
                    &self.paths.icons.join(&png).display().to_string(),
                ]);
            }
            println!(
                "  готовые PNG для программ: нарисовано {}, уже были {}, оставлено в SVG {} \
                 ({:.1} с)",
                stats.drawn,
                stats.kept,
                stats.skipped,
                start.elapsed().as_secs_f32()
            );
            xset("xsettings", "/Net/IconThemeName", "string", &png);
            return;
        }
        xset("xsettings", "/Net/IconThemeName", "string", theme);
    }

    /// Меню яблока: ярлыки `mimshel action …` и файл меню freedesktop с их
    /// порядком и разделителями (его читают и своя строка меню, и панель Xfce).
    fn apple_menu(&self) {
        let dir = self.paths.main_menu_items();
        let bin = self.paths.shell_bin.display();
        let mut wanted = Vec::new();
        let mut layout = Vec::new();
        for (id, label, separator) in crate::paths::APPLE_MENU {
            if *separator {
                layout.push("    <Separator/>".to_string());
            }
            let file = format!("{}-{id}.desktop", mimshel_core::names::APP);
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
            &self.paths.main_menu(),
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

/// Строка списка запуска сеанса с одной из прежних пометок.
fn old_slot(list: &[&[&str]]) -> Option<usize> {
    list.iter().find_map(|command| session_slot(command))
}

/// Запускать ли панель Xfce при входе: её строку в списке сеанса меняем на
/// пустую команду или сбрасываем к системной (там xfce4-panel).
fn panel_at_login(on: bool) {
    if on {
        if let Some(i) = session_slot(PANEL_OFF).or_else(|| old_slot(OLD_PANEL_OFF)) {
            xreset("xfce4-session", &format!("{SESSION}/Client{i}_Command"));
        }
    } else if let Some(i) = session_slot(&["xfce4-panel"]).or_else(|| old_slot(OLD_PANEL_OFF)) {
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
        if !is_managed(&text) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitors_from_xrandr() {
        let text = "Monitors: 2\n 0: +*eDP-1 2560/344x1440/194+0+0  eDP-1\n \
                    1: +HDMI-1 1920/530x1080/300+2560+0  HDMI-1\n";
        assert_eq!(
            parse_monitors(text),
            [(0, 0, 2560, 1440), (2560, 0, 1920, 1080)]
        );
        assert_eq!(
            parse_monitors("Monitors: 1\n 0: +Virtual-1 1104/292x846/224+0+0  Virtual-1\n"),
            [(0, 0, 1104, 846)]
        );
        assert!(parse_monitors("").is_empty());
    }
}
