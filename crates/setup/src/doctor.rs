//! Проверка, что всё на месте (`shell doctor`): пакеты, тема, панель или своя
//! строка меню, уведомления, раскладки, меню яблока, блокировка, док, сама
//! служба shell. То же, что проверял `macos-look doctor`, и вдобавок своё.
//!
//! Чинить пока помогает macos-look: у каждой проверки есть часть, и
//! [`fix_parts`] говорит, что ему применить.

use std::path::Path;

use shell_core::config::Config;

use crate::look::{APPLE_MENU, Look, Paths};
use crate::sys::{
    bus_owner, home, in_desktop_session, keyboard_ok, output, output_any, parent, pgrep, read,
    running, succeeds, which, xget, xget_list,
};

/// Пакеты из репозитория и из AUR (их ставит `macos-look install`).
const PACKAGES: &[&str] = &[
    "plank",
    "inter-font",
    "rofi",
    "zenity",
    "imagemagick",
    "xss-lock",
    "xorg-xset",
    "appmenu-gtk-module",
    "i3lock-color",
];
/// Глобальное меню — собирает `macos-look build-appmenu`.
const APPMENU_PACKAGES: &[&str] = &[
    "vala-panel-appmenu-xfce",
    "appmenu-glib-translator",
    "vala-panel-appmenu-registrar",
    "vala-panel-appmenu-locale",
];
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
/// Строка списка запуска сеанса, которой macos-look заменяет панель Xfce.
const PANEL_OFF: &[&str] = &["true", "macos-look-no-panel"];
const SESSION: &str = "/sessions/Failsafe";
/// Сочетания клавиш: (клавиши, действие).
const SHORTCUTS: &[(&str, &str)] = &[("<Super>space", "spotlight"), ("<Super>a", "launchpad")];
/// Метка в i3lock, собранном с заплаткой macos-look.
const LOCKER_MARK: &[u8] = b"macos-look-field-v3";
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
pub fn checks(look: &Look, cfg: &Config, config: Option<&Path>) -> Vec<Check> {
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
    let n = look.names();

    add(
        "env",
        "графический сеанс доступен",
        xget("xsettings", "/Net/ThemeName").is_some(),
        "войдите в систему в окне ВМ",
    );

    // Пакеты: одним вызовом pacman — установленные он печатает.
    let all: Vec<&str> = PACKAGES.iter().chain(APPMENU_PACKAGES).copied().collect();
    let mut q = vec!["pacman", "-Q"];
    q.extend(&all);
    let installed = output_any(&q);
    let have = |p: &str| {
        installed
            .lines()
            .any(|l| l.split_whitespace().next() == Some(p))
    };
    for p in PACKAGES {
        add(
            "install",
            &format!("пакет {p}"),
            have(p),
            "macos-look install",
        );
    }
    for p in APPMENU_PACKAGES {
        add(
            "install",
            &format!("пакет {p}"),
            have(p),
            "macos-look build-appmenu",
        );
    }
    let theme_dir = paths.themes.join(&n.theme);
    add(
        "install",
        &format!("тема {}", n.theme),
        theme_dir.join("gtk-3.0").is_dir(),
        "macos-look install",
    );
    add(
        "install",
        &format!("оформление окон {}", n.theme),
        theme_dir.join("xfwm4").is_dir(),
        "macos-look install",
    );
    add(
        "install",
        &format!("иконки {}", n.icons_base),
        paths.icons.join(&n.icons_base).is_dir(),
        "macos-look install",
    );
    add(
        "look",
        &format!("надстройка иконок {}", n.icons),
        paths.icons.join(&n.icons).join("index.theme").exists(),
        "",
    );
    add(
        "install",
        &format!("курсор {}", n.cursor),
        paths.icons.join(&n.cursor).is_dir(),
        "macos-look install",
    );
    let wallpaper = paths.wallpapers.join(format!("{}.jpg", n.wallpaper));
    add(
        "install",
        &format!("обои {}", n.wallpaper),
        wallpaper.exists(),
        "macos-look install",
    );
    add(
        "install",
        "шрифт Inter",
        output_any(&["fc-list", ":family"]).contains("Inter"),
        "macos-look install",
    );

    // Оформление.
    let xs = |p: &str| xget("xsettings", p);
    add(
        "look",
        "тема GTK",
        xs("/Net/ThemeName").as_deref() == Some(&n.theme),
        "",
    );
    add(
        "look",
        "иконки",
        xs("/Net/IconThemeName").as_deref() == Some(&n.icons),
        "",
    );
    add(
        "look",
        "курсор",
        xs("/Gtk/CursorThemeName").as_deref() == Some(&n.cursor),
        "",
    );
    add(
        "look",
        "шрифт",
        xs("/Gtk/FontName").as_deref() == Some(look.get("look", "font")),
        "",
    );
    add(
        "look",
        "тема окон",
        xget("xfwm4", "/general/theme").as_deref() == Some(&n.theme),
        "",
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
        "macos-look apply look",
    );
    let gtk4 = home().join(".config/gtk-4.0/gtk.css");
    add(
        "look",
        "тема для GTK4-программ",
        gtk4.is_symlink()
            && gtk4.exists()
            && std::fs::read_link(&gtk4).is_ok_and(|t| t.to_string_lossy().contains(&n.theme)),
        "",
    );

    // Обои на каждом мониторе, где они заданы.
    let image = wallpaper.to_string_lossy();
    let last_image = |m: &str| {
        xget(
            "xfce4-desktop",
            &format!("/backdrop/screen0/{m}/workspace0/last-image"),
        )
    };
    let mons: Vec<String> = monitors()
        .into_iter()
        .filter(|m| last_image(m).is_some())
        .collect();
    add(
        "wallpaper",
        "обои на всех мониторах",
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
            panel_session_slot(PANEL_OFF) && !panel_session_slot(&["xfce4-panel"]),
            "macos-look apply panel",
        );
        add(
            "panel",
            "панель Xfce остановлена",
            !running("xfce4-panel"),
            "macos-look apply panel",
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
            !panel_session_slot(PANEL_OFF),
            "macos-look apply panel",
        );
    }

    let masked = output_any(&["systemctl", "--user", "is-enabled", "xfce4-notifyd.service"]).trim()
        == "masked";
    if cfg.notify {
        add(
            "notify",
            "уведомления показывает shell",
            bus_owner("org.freedesktop.Notifications") == "shell",
            "macos-look apply notify",
        );
        add(
            "notify",
            "xfce4-notifyd не запускается (служба замаскирована)",
            masked,
            "macos-look apply notify",
        );
    } else {
        add(
            "notify",
            "xfce4-notifyd не замаскирован",
            !masked,
            "macos-look apply notify",
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
            "macos-look apply panel",
        );
    }

    // Меню яблока, Launchpad, Spotlight.
    let menu_dir = paths.app_dir.join("menu");
    add(
        "menu",
        "файл меню яблока",
        paths.app_dir.join("apple.menu").exists(),
        "",
    );
    for (id, label, _) in APPLE_MENU {
        add(
            "menu",
            &format!("пункт «{label}» → shell action {id}"),
            read(&menu_dir.join(format!("macos-{id}.desktop")))
                .contains(&format!("Exec={} action {id}\n", paths.shell_bin.display())),
            "shell apply launchers",
        );
    }
    let launchpad = paths.applications.join("macos-launchpad.desktop");
    add("menu", "значок Launchpad", launchpad.exists(), "");
    for app in look.hidden_apps() {
        if Path::new("/usr/share/applications")
            .join(format!("{app}.desktop"))
            .exists()
        {
            add(
                "menu",
                &format!("скрыто из Launchpad: {app}"),
                read(&paths.applications.join(format!("{app}.desktop"))).contains("NoDisplay=true"),
                "",
            );
        }
    }
    for t in ["spotlight", "launchpad"] {
        add(
            "menu",
            &format!("оформление {t}"),
            paths.app_dir.join(format!("rofi/{t}.rasi")).exists(),
            "",
        );
    }
    for (keys, action) in SHORTCUTS {
        let want = Look::shortcut_command(&paths, action, cfg.part(action) == Some(true));
        add(
            "menu",
            &format!("сочетание {keys} → {want}"),
            xget(
                "xfce4-keyboard-shortcuts",
                &format!("/commands/custom/{keys}"),
            )
            .as_deref()
                == Some(&*want),
            "",
        );
    }
    let launchpad_cmd = Look::shortcut_command(&paths, "launchpad", cfg.launchpad);
    add(
        "menu",
        &format!(
            "значок Launchpad → {}",
            if cfg.launchpad { "shell" } else { "rofi" }
        ),
        read(&launchpad).contains(&format!("Exec={launchpad_cmd}\n")),
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
    add(
        "lock",
        "поле пароля как на Mac (точки при вводе)",
        which("i3lock")
            .and_then(|p| std::fs::read(p).ok())
            .is_some_and(|b| b.windows(LOCKER_MARK.len()).any(|w| w == LOCKER_MARK)),
        "macos-look build-locker",
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
        read(&paths.autostart.join("macos-look-lock.desktop"))
            .contains(&format!("-- {} lock --nofork", paths.shell_bin.display())),
        "shell apply lock",
    );
    add(
        "lock",
        "старая заставка Xfce отключена",
        paths.autostart.join("xfce4-screensaver.desktop").exists() && !running("xfce4-screensaver"),
        "",
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
            // 14 у «бездействия» значит «никогда».
            add(
                "power",
                &format!("ВМ: нет автосна ({key})"),
                xget(
                    "xfce4-power-manager",
                    &format!("/xfce4-power-manager/{key}"),
                )
                .as_deref()
                    == Some("14"),
                "",
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
            "macos-look apply menu из терминала рабочего стола или перезайдите в сеанс",
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
                    "macos-look apply menu",
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
            "macos-look apply dock",
        );
        add(
            "dock",
            "автозапуск Plank выключен",
            !paths.autostart.join("plank.desktop").exists(),
            "macos-look apply dock",
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
        add(
            "dock",
            "тема дока",
            theme.trim().trim_matches('\'') == n.theme,
            "",
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

/// Что применить, чтобы починить найденное: части оформления (macos-look;
/// `install` — сначала поставить пакеты) и части shell ([`crate::parts`]).
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

/// Мониторы: из xrandr и из настроек обоев Xfce.
fn monitors() -> Vec<String> {
    let mut found: Vec<String> = output_any(&["xrandr", "--listmonitors"])
        .lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().last())
        .map(|m| format!("monitor{m}"))
        .collect();
    for line in output_any(&["xfconf-query", "-c", "xfce4-desktop", "-l"]).lines() {
        let parts: Vec<&str> = line.split('/').collect();
        if parts.len() > 3 && parts[1] == "backdrop" && parts[3].starts_with("monitor") {
            found.push(parts[3].to_string());
        }
    }
    found.sort();
    found.dedup();
    if found.is_empty() {
        found.push("monitor0".into());
    }
    found
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
