//! Проверка, что всё на месте (`shell doctor`): пакеты, тема, панель или своя
//! строка меню, уведомления, раскладки, меню яблока, блокировка, док, сама
//! служба shell. У каждой проверки есть часть, и [`fix_parts`] говорит, что
//! применить, чтобы починить.

use std::path::Path;

use shell_core::config::Config;

use crate::paths::{APPLE_MENU, APPS_SHORTCUT, Paths, SHORTCUTS, shortcut_command};
use crate::sys::{DESKTOP_OFF, OLD_PANEL_OFF, PANEL_OFF};
use crate::sys::{
    bus_owner, home, in_desktop_session, keyboard_ok, output, output_any, parent, pgrep, read,
    running, succeeds, xget, xget_list,
};

/// Пакеты, которые ставит наш установщик (`install.sh`).
const PACKAGES: &[&str] = &[
    "inter-font",
    "zenity",
    "xss-lock",
    "xorg-xset",
    "xorg-xrandr",
    "xorg-xkbcomp",
    "xorg-xinput",
    "appmenu-gtk-module",
    "sassc",
    "network-manager-applet",
    "xfce4-taskmanager",
];
/// Глобальное меню в панели Xfce (если строка меню — панель Xfce).
const APPMENU_PACKAGES: &[&str] = &["vala-panel-appmenu-xfce", "vala-panel-appmenu-registrar"];
/// Модули верхней панели Xfce по порядку (когда строка меню — панель Xfce).
const PANEL_PLUGINS: &[&str] = &[
    "applicationsmenu",
    "appmenu",
    "separator",
    "systray",
    "xkb",
    "pulseaudio",
    "power-manager-plugin",
    "notification-plugin",
    "separator",
    "clock",
];
const SESSION: &str = "/sessions/Failsafe";
/// Наблюдатели службы (см. `apps/shell/src/status.rs`).
const WATCHERS: &str = r"^(pactl subscribe|udevadm monitor --kernel --subsystem-match=power_supply|xfconf-query -c xfce4-notifyd -m)";

pub struct Check {
    /// Часть оформления: env, install, look, wallpaper, panel, notify, menu,
    /// lock, power, dock, shell; config — свой файл настроек, его правят руками.
    pub part: &'static str,
    pub what: String,
    pub ok: bool,
    /// Как починить (может быть пустой).
    pub hint: String,
}

/// Все проверки. `cfg` — настройки shell, `config` — их файл.
pub fn checks(cfg: &Config, config: Option<&Path>) -> Vec<Check> {
    let mut out = Vec::new();
    let mut add = |part: &'static str, what: &str, ok: bool, hint: &str| {
        out.push(Check {
            part,
            what: what.to_string(),
            ok,
            hint: hint.to_string(),
        });
    };
    let paths = Paths::new();

    add(
        "env",
        "графический сеанс доступен",
        xget("xsettings", "/Net/ThemeName").is_some(),
        "войдите в систему в окне ВМ",
    );

    // Пакеты: свои и прежних программ, которым отданы части.
    let hint = "./install.sh в папке проекта";
    let mut extra: Vec<&str> = Vec::new();
    if !cfg.dock {
        extra.push("plank");
    }
    if !cfg.search_panel || !cfg.apps_panel {
        extra.push("rofi");
    }
    if !cfg.menubar {
        extra.extend(APPMENU_PACKAGES);
    }
    // Одним вызовом pacman — установленные он печатает.
    let all: Vec<&str> = PACKAGES.iter().chain(&extra).copied().collect();
    let mut q = vec!["pacman", "-Q"];
    q.extend(&all);
    let installed = output_any(&q);
    let have = |p: &str| {
        installed
            .lines()
            .any(|l| l.split_whitespace().next() == Some(p))
    };
    for p in &all {
        add("install", &format!("пакет {p}"), have(p), hint);
    }
    let theme_dir = paths.themes.join(&cfg.gtk_theme);
    add(
        "install",
        &format!("тема {}", cfg.gtk_theme),
        theme_dir.join("gtk-3.0").is_dir(),
        "поставьте тему в ~/.themes",
    );
    add(
        "install",
        &format!("оформление окон {}", cfg.gtk_theme),
        theme_dir.join("xfwm4").is_dir(),
        "поставьте тему в ~/.themes",
    );
    add(
        "install",
        &format!("курсор {}", cfg.cursor_theme),
        paths.icons.join(&cfg.cursor_theme).is_dir(),
        "поставьте курсор в ~/.local/share/icons",
    );
    let (sw, sh) = crate::lock::screen_size();
    let wallpaper = shell_wallpaper::Style::parse(&cfg.wallpaper).map_or_else(
        || std::path::PathBuf::from(&cfg.wallpaper),
        |st| {
            home().join(format!(
                ".local/share/shell/wallpapers/{}-{sw}x{sh}.png",
                st.name()
            ))
        },
    );
    add(
        "install",
        &format!("обои {}", cfg.wallpaper),
        wallpaper.exists(),
        "shell apply wallpaper",
    );
    add(
        "install",
        "шрифт Inter",
        output_any(&["fc-list", ":family"]).contains("Inter"),
        hint,
    );

    // Оформление.
    let xs = |p: &str| xget("xsettings", p);
    add(
        "look",
        &format!("тема программ: {}", cfg.gtk_theme),
        xs("/Net/ThemeName").as_deref() == Some(&cfg.gtk_theme),
        "shell apply look",
    );
    add(
        "look",
        &format!("значки: {}", cfg.icon_theme),
        xs("/Net/IconThemeName").as_deref() == Some(&cfg.icon_theme),
        "shell apply icons",
    );
    if cfg.icon_theme == crate::parts::OVERLAY_THEME {
        add(
            "look",
            "надстройка значков: знак вместо яблока Apple",
            paths
                .icons
                .join(crate::parts::OVERLAY_THEME)
                .join("places/scalable/start-here.svg")
                .exists(),
            "shell apply icons",
        );
    }
    add(
        "look",
        &format!("курсор: {}", cfg.cursor_theme),
        xs("/Gtk/CursorThemeName").as_deref() == Some(&cfg.cursor_theme),
        "shell apply look",
    );
    add(
        "look",
        "шрифт",
        xs("/Gtk/FontName").as_deref() == Some(&*format!("{} 10", cfg.font)),
        "shell apply look",
    );
    add(
        "look",
        &format!("тема окон: {}", cfg.gtk_theme),
        xget("xfwm4", "/general/theme").as_deref() == Some(&cfg.gtk_theme),
        "shell apply look",
    );
    add(
        "look",
        "композитинг (тени, прозрачность)",
        xget("xfwm4", "/general/use_compositing").as_deref() == Some("true"),
        "",
    );
    let session = crate::sys::session_env();
    add(
        "look",
        "меню GTK-программ уходит в строку меню (GTK_MODULES в сеансе)",
        session
            .get("GTK_MODULES")
            .is_some_and(|m| m.contains("appmenu-gtk-module")),
        "выйдите из системы и войдите снова (модуль подключается при входе)",
    );
    add(
        "look",
        "модуль меню не включён «на лету» (роняет программы)",
        xs("/Gtk/Modules").is_none(),
        "shell apply look",
    );
    let gtk4 = home().join(".config/gtk-4.0/gtk.css");
    add(
        "look",
        "тема для GTK4-программ",
        gtk4.is_symlink()
            && gtk4.exists()
            && std::fs::read_link(&gtk4)
                .is_ok_and(|t| t.to_string_lossy().contains(&cfg.gtk_theme)),
        "shell apply look",
    );

    // Обои на каждом мониторе, где они заданы.
    let image = wallpaper.to_string_lossy();
    let last_image = |m: &str| {
        xget(
            "xfce4-desktop",
            &format!("/backdrop/screen0/{m}/workspace0/last-image"),
        )
    };
    let mons: Vec<String> = crate::sys::monitors()
        .into_iter()
        .filter(|m| last_image(m).is_some())
        .collect();
    add(
        "wallpaper",
        &format!("обои на всех мониторах: {}", cfg.wallpaper),
        !mons.is_empty()
            && mons
                .iter()
                .all(|m| last_image(m).as_deref() == Some(&*image)),
        "",
    );

    // Строка меню: своя или панель Xfce.
    if cfg.menubar {
        add(
            "panel",
            "строка меню — shell: панель Xfce не запускается при входе",
            (panel_session_slot(PANEL_OFF) || panel_session_slot(OLD_PANEL_OFF))
                && !panel_session_slot(&["xfce4-panel"]),
            "shell apply menubar",
        );
        add(
            "panel",
            "панель Xfce остановлена",
            !running("xfce4-panel"),
            "shell apply menubar",
        );
    } else {
        let ids = xget_list("xfce4-panel", "/panels/panel-1/plugin-ids");
        let have: Vec<String> = ids
            .iter()
            .filter_map(|i| xget("xfce4-panel", &format!("/plugins/plugin-{i}")))
            .collect();
        add(
            "panel",
            "одна верхняя панель",
            xget_list("xfce4-panel", "/panels") == ["1"],
            "",
        );
        add(
            "panel",
            "состав верхней панели",
            have == PANEL_PLUGINS,
            &format!("ожидалось {PANEL_PLUGINS:?}, сейчас {have:?}"),
        );
        add("panel", "панель запущена", running("xfce4-panel"), "");
        add(
            "panel",
            "панель Xfce запускается при входе",
            !(panel_session_slot(PANEL_OFF) || panel_session_slot(OLD_PANEL_OFF)),
            "shell apply menubar",
        );
    }

    if cfg.menu_icon == crate::parts::LOGO_NAME {
        add(
            "panel",
            "знак меню (планета) на месте",
            crate::parts::logo_path().exists(),
            "shell apply menubar",
        );
    }
    let masked = output_any(&["systemctl", "--user", "is-enabled", "xfce4-notifyd.service"]).trim()
        == "masked";
    if cfg.notify {
        add(
            "notify",
            "уведомления показывает shell",
            bus_owner("org.freedesktop.Notifications") == "shell",
            "shell apply notify",
        );
        add(
            "notify",
            "xfce4-notifyd не запускается (служба замаскирована)",
            masked,
            "shell apply notify",
        );
    } else {
        add(
            "notify",
            "xfce4-notifyd не замаскирован",
            !masked,
            "shell apply notify",
        );
    }
    if cfg.desktop {
        add(
            "desktop",
            "рабочий стол рисует shell: xfdesktop не запущен",
            !running("xfdesktop"),
            "shell apply desktop",
        );
        add(
            "desktop",
            "xfdesktop не запускается при входе",
            panel_session_slot(DESKTOP_OFF) && !panel_session_slot(&["xfdesktop"]),
            "shell apply desktop",
        );
    } else {
        add(
            "desktop",
            "рабочий стол — xfdesktop запущен",
            running("xfdesktop"),
            "shell apply desktop",
        );
    }
    add(
        "panel",
        "все раскладки клавиатуры загружены",
        keyboard_ok(),
        "setxkbmap — см. урок 9 в ~/shell/CLAUDE.md",
    );
    if Path::new("/etc/xdg/autostart/nm-applet.desktop").exists() {
        add(
            "panel",
            "nm-applet — значок современным способом (--indicator)",
            read(&paths.autostart.join("nm-applet.desktop")).contains("--indicator"),
            "shell apply menubar",
        );
    }

    // Меню яблока, окна «Программы» и «Поиск».
    let menu_dir = paths.main_menu_items();
    add(
        "menu",
        "файл главного меню",
        paths.main_menu().exists(),
        "shell apply launchers",
    );
    for (id, label, _) in APPLE_MENU {
        add(
            "menu",
            &format!("пункт «{label}» → shell action {id}"),
            read(&menu_dir.join(format!("shell-{id}.desktop")))
                .contains(&format!("Exec={} action {id}\n", paths.shell_bin.display())),
            "shell apply launchers",
        );
    }
    let apps_desktop = paths
        .applications
        .join(format!("{}.desktop", crate::parts::APPS_DESKTOP));
    add(
        "menu",
        "значок «Программы» в доке",
        apps_desktop.exists(),
        "",
    );
    for s in SHORTCUTS {
        let want = shortcut_command(&paths, s, cfg.part(s.part) == Some(true));
        add(
            "menu",
            &format!("сочетание {} → {want}", s.keys),
            xget(
                "xfce4-keyboard-shortcuts",
                &format!("/commands/custom/{}", s.keys),
            )
            .as_deref()
                == Some(&*want),
            "",
        );
    }
    let apps_cmd = shortcut_command(&paths, &APPS_SHORTCUT, cfg.apps_panel);
    add(
        "menu",
        &format!(
            "значок «Программы» → {}",
            if cfg.apps_panel { "shell" } else { "rofi" }
        ),
        read(&apps_desktop).contains(&format!("Exec={apps_cmd}\n")),
        "",
    );

    // Блокировка экрана и питание.
    add(
        "lock",
        "команда блокировки → shell lock",
        xget("xfce4-session", "/general/LockCommand").as_deref()
            == Some(&*format!("{} lock", paths.shell_bin.display())),
        "shell apply lock",
    );
    let xss = pgrep("^xss-lock(\\s|$)");
    add(
        "lock",
        "xss-lock запущен (блокировка при бездействии и перед сном)",
        !xss.is_empty(),
        "",
    );
    add(
        "lock",
        "xss-lock в сеансе рабочего стола",
        xss.first().is_some_and(|&p| in_desktop_session(p)),
        "выйдите из системы и войдите снова (или перезагрузите ВМ)",
    );
    // Запущенный xss-lock не перезапускают (урок 3) — новая команда
    // заработает со следующего входа.
    add(
        "lock",
        "xss-lock вызывает shell lock",
        xss.first().is_some_and(|p| {
            std::fs::read(format!("/proc/{p}/cmdline"))
                .is_ok_and(|c| String::from_utf8_lossy(&c).contains("shell\0lock"))
        }),
        "выйдите из системы и войдите снова",
    );
    add(
        "lock",
        "автозапуск xss-lock → shell lock",
        read(&paths.autostart.join(crate::parts::LOCK_AUTOSTART))
            .contains(&format!("-- {} lock --nofork", paths.shell_bin.display())),
        "shell apply lock",
    );
    add(
        "lock",
        "старая заставка Xfce отключена",
        paths.autostart.join("xfce4-screensaver.desktop").exists() && !running("xfce4-screensaver"),
        "",
    );
    add(
        "lock",
        "заставку Xfce не запустит D-Bus (иначе блокирует она, своим окном)",
        read(&crate::lock::screensaver_service()).contains("Exec=/bin/false"),
        "shell apply lock",
    );
    let (w, h) = crate::lock::screen_size();
    add(
        "lock",
        &format!("фон блокировки {w}x{h}"),
        home()
            .join(format!(".cache/shell/lock/bg-{w}x{h}.png"))
            .exists(),
        "shell apply lock",
    );
    if succeeds(&["systemd-detect-virt", "-q"]) {
        for key in ["inactivity-on-ac", "inactivity-on-battery"] {
            // «Никогда» — 0 (xfce4-power-manager 4.20); 14 — уже через 14 минут.
            add(
                "power",
                &format!("ВМ: нет автосна ({key})"),
                xget(
                    "xfce4-power-manager",
                    &format!("/xfce4-power-manager/{key}"),
                )
                .as_deref()
                    == Some(crate::parts::SLEEP_NEVER),
                "shell apply power",
            );
        }
    }

    // Служба shell.
    if cfg.shell_needed() {
        add(
            "shell",
            "программа shell установлена",
            paths.shell_bin.exists(),
            "cd ~/shell && cargo build --release && install -m 755 target/release/shell ~/.local/bin/",
        );
        add(
            "shell",
            "автозапуск службы shell",
            paths.autostart.join("shell-daemon.desktop").exists(),
            "",
        );
        let guard = pgrep(r"^(\S*/)?shell daemon$");
        add("shell", "служба shell запущена", !guard.is_empty(), "");
        add(
            "shell",
            "служба shell в сеансе рабочего стола (не по SSH)",
            guard.first().is_some_and(|&p| in_desktop_session(p)),
            "shell apply service из терминала рабочего стола или перезайдите в сеанс",
        );
        if let Some(path) = config {
            let error = match std::fs::read_to_string(path) {
                Ok(text) => shell_core::config::toml_error(&text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => Some(e.to_string()),
            };
            let hint = error.unwrap_or_default();
            add(
                "config",
                &format!("файл настроек {} без ошибок", path.display()),
                hint.is_empty(),
                &hint,
            );
        }
        if cfg.menubar {
            for (name, what) in [
                ("org.kde.StatusNotifierWatcher", "значки трея"),
                ("com.canonical.AppMenu.Registrar", "глобальное меню"),
            ] {
                add(
                    "shell",
                    &format!("{what} принимает shell"),
                    bus_owner(name) == "shell",
                    "shell apply service",
                );
            }
        }
        // Наблюдатели (звук, батарея, «Не беспокоить») — только у живой службы.
        let child = pgrep(r"^(\S*/)?shell daemon --child$");
        let orphans = pgrep(WATCHERS)
            .into_iter()
            .filter(|&w| parent(w).is_none_or(|p| !child.contains(&p)))
            .count();
        add(
            "shell",
            "нет брошенных наблюдателей (pactl, udevadm, xfconf-query)",
            orphans == 0,
            &format!("лишних процессов: {orphans}"),
        );
    }

    // Док.
    if cfg.dock {
        add(
            "dock",
            "док — shell, Plank не запущен",
            !running("plank"),
            "shell apply dock",
        );
        add(
            "dock",
            "автозапуск Plank выключен",
            !paths.autostart.join("plank.desktop").exists(),
            "shell apply dock",
        );
    } else {
        add("dock", "Plank запущен", running("plank"), "");
        add(
            "dock",
            "автозапуск дока",
            paths.autostart.join("plank.desktop").exists(),
            "",
        );
        let theme = output(&["dconf", "read", "/net/launchpad/plank/docks/dock1/theme"])
            .unwrap_or_default();
        // Та же тема, что ставит `shell apply dock`.
        let want = if home()
            .join(".local/share/plank/themes")
            .join(&cfg.gtk_theme)
            .is_dir()
        {
            cfg.gtk_theme.as_str()
        } else {
            "Transparent"
        };
        add(
            "dock",
            "тема дока",
            theme.trim().trim_matches('\'') == want,
            "shell apply dock",
        );
        let items: Vec<_> = std::fs::read_dir(home().join(".config/plank/dock1/launchers"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "dockitem"))
            .collect();
        add("dock", "значки дока", !items.is_empty(), "");
        for item in items {
            let text = read(&item);
            let target = text.rsplit_once("file://").map_or("", |t| t.1.trim());
            add(
                "dock",
                &format!(
                    "значок {} ведёт на программу",
                    item.file_stem().unwrap_or_default().to_string_lossy()
                ),
                Path::new(target).exists(),
                "удалите программу из dock.apps или установите её",
            );
        }
    }
    out
}

/// Что применить, чтобы починить найденное: части [`crate::parts`]
/// (`install` — не хватает пакетов: их ставит установщик).
/// `None` — чинить нечего или нельзя (нет сеанса).
pub fn fix_parts(checks: &[Check]) -> Option<Vec<&'static str>> {
    let mut parts: Vec<&'static str> = Vec::new();
    for c in checks.iter().filter(|c| !c.ok) {
        match c.part {
            "env" => return None,
            // Ошибку в файле настроек исправит только человек.
            "config" => continue,
            _ => {}
        }
        let part = match c.part {
            "panel" => "menubar",
            "shell" => "service",
            p => p,
        };
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    (!parts.is_empty()).then_some(parts)
}

/// Есть ли в списке запуска сеанса строка с такой командой.
fn panel_session_slot(command: &[&str]) -> bool {
    let count: usize = xget("xfce4-session", &format!("{SESSION}/Count"))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (0..count)
        .any(|i| xget_list("xfce4-session", &format!("{SESSION}/Client{i}_Command")) == command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(part: &'static str, ok: bool) -> Check {
        Check {
            part,
            what: String::new(),
            ok,
            hint: String::new(),
        }
    }

    #[test]
    fn parts_to_fix() {
        assert_eq!(fix_parts(&[check("look", true)]), None);
        assert_eq!(
            fix_parts(&[
                check("shell", false),
                check("dock", false),
                check("menu", false),
                check("look", true),
                check("config", false),
            ]),
            Some(vec!["service", "dock", "menu"])
        );
        assert_eq!(fix_parts(&[check("config", false)]), None);
        assert_eq!(
            fix_parts(&[check("dock", false), check("env", false)]),
            None
        );
    }
}
