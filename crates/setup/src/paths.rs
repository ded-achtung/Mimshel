//! Где что лежит у Mimshel и что открывают сочетания клавиш и меню планеты.

use std::path::PathBuf;

use crate::sys::home;

/// Главное меню (значок планеты): (действие и имя ярлыка, подпись,
/// разделитель перед пунктом).
pub const APPLE_MENU: &[(&str, &str, bool)] = &[
    ("about", "Об этом компьютере", false),
    ("settings", "Системные настройки…", true),
    ("forcequit", "Принудительно завершить…", true),
    ("sleep", "Режим сна", true),
    ("restart", "Перезагрузить…", false),
    ("shutdown", "Выключить…", false),
    ("lock", "Заблокировать экран", true),
    ("logout", "Завершить сеанс…", false),
];

/// Папки и файлы Mimshel.
pub struct Paths {
    /// Свои данные: меню планеты, копии настроек, обои.
    pub data: PathBuf,
    pub themes: PathBuf,
    pub icons: PathBuf,
    pub autostart: PathBuf,
    pub applications: PathBuf,
    pub shell_bin: PathBuf,
}

impl Paths {
    pub fn new() -> Paths {
        let h = home();
        Paths {
            data: mimshel_core::names::dir_in(&h.join(".local/share")),
            themes: h.join(".themes"),
            icons: h.join(".local/share/icons"),
            autostart: h.join(".config/autostart"),
            applications: h.join(".local/share/applications"),
            shell_bin: h.join(".local/bin").join(mimshel_core::names::APP),
        }
    }

    /// Файл меню планеты по стандарту freedesktop (его читают своя строка
    /// меню и панель Xfce) и папка с его пунктами.
    pub fn main_menu(&self) -> PathBuf {
        self.data.join("apple.menu")
    }

    pub fn main_menu_items(&self) -> PathBuf {
        self.data.join("menu")
    }
}

impl Default for Paths {
    fn default() -> Paths {
        Paths::new()
    }
}

/// Сочетание клавиш, открывающее окно оболочки.
pub struct Shortcut {
    /// Клавиши, как их пишет xfconf.
    pub keys: &'static str,
    /// Часть в `[parts]`: кто показывает окно — Mimshel или rofi.
    pub part: &'static str,
    /// Команда Mimshel (`mimshel search`).
    pub command: &'static str,
}

/// Окно «Поиск».
pub const SEARCH_SHORTCUT: Shortcut = Shortcut {
    keys: "<Super>space",
    part: "search_panel",
    command: "search",
};

/// Окно «Программы» (и значок в доке).
pub const APPS_SHORTCUT: Shortcut = Shortcut {
    keys: "<Super>a",
    part: "apps_panel",
    command: "apps",
};

pub const SHORTCUTS: &[Shortcut] = &[SEARCH_SHORTCUT, APPS_SHORTCUT];

/// Команда для сочетания клавиш и значка окна: своя программа или, если
/// часть отдана прежней программе, rofi со списком программ.
pub fn shortcut_command(paths: &Paths, s: &Shortcut, shell: bool) -> String {
    if shell {
        format!("{} {}", paths.shell_bin.display(), s.command)
    } else {
        "rofi -show drun".to_string()
    }
}
