//! Настройки оформления из конфига macos-look (`~/.config/macos-look/config.ini`).
//! Пока macos-look не переписан целиком, тема, обои, блокировка и то, какие
//! части включены, задаются там; чего в файле нет — значения по умолчанию,
//! те же, что у macos-look.

use std::collections::HashMap;
use std::path::PathBuf;

use shell_core::config::parse_ini;

use crate::sys::home;

/// Значения по умолчанию: (раздел, ключ, значение).
const DEFAULTS: &[(&str, &str, &str)] = &[
    ("look", "variant", "dark"),
    ("look", "font", "Inter 10"),
    ("wallpaper", "dark", "Sonoma-dark"),
    ("wallpaper", "light", "Sonoma-light"),
    ("dock", "engine", "plank"),
    ("launchpad", "engine", "rofi"),
    (
        "launchpad",
        "hide",
        "rofi, rofi-theme-selector, qv4l2, qvidcap, xterm, uxterm, yad-settings, plank, \
         reflector-simple",
    ),
    ("spotlight", "engine", "rofi"),
    ("menubar", "engine", "xfce"),
    ("notify", "engine", "xfce"),
];

/// Меню яблока: (имя ярлыка, подпись).
pub const APPLE_MENU: &[(&str, &str)] = &[
    ("about", "Об этом компьютере"),
    ("settings", "Системные настройки…"),
    ("forcequit", "Принудительно завершить…"),
    ("sleep", "Режим сна"),
    ("restart", "Перезагрузить…"),
    ("shutdown", "Выключить…"),
    ("lock", "Заблокировать экран"),
    ("logout", "Завершить сеанс…"),
];

/// Пути, которыми пользуется macos-look.
pub struct Paths {
    pub app_dir: PathBuf,
    pub themes: PathBuf,
    pub icons: PathBuf,
    pub wallpapers: PathBuf,
    pub autostart: PathBuf,
    pub applications: PathBuf,
    pub tool: PathBuf,
    pub shell_bin: PathBuf,
}

impl Paths {
    pub fn new() -> Paths {
        let h = home();
        Paths {
            app_dir: h.join(".local/share/macos-look"),
            themes: h.join(".themes"),
            icons: h.join(".local/share/icons"),
            wallpapers: h.join(".local/share/backgrounds/macos-look"),
            autostart: h.join(".config/autostart"),
            applications: h.join(".local/share/applications"),
            tool: h.join(".local/bin/macos-look"),
            shell_bin: h.join(".local/bin/shell"),
        }
    }
}

impl Default for Paths {
    fn default() -> Paths {
        Paths::new()
    }
}

/// Названия темы, иконок, курсора и обоев для светлого или тёмного вида.
pub struct Names {
    pub theme: String,
    pub icons_base: String,
    /// Надстройка над иконками (её строит macos-look).
    pub icons: String,
    pub cursor: String,
    pub wallpaper: String,
}

pub struct Look {
    ini: HashMap<(String, String), String>,
}

impl Look {
    pub fn load() -> Look {
        let text = std::fs::read_to_string(home().join(".config/macos-look/config.ini"))
            .unwrap_or_default();
        Look::from_text(&text)
    }

    pub fn from_text(text: &str) -> Look {
        Look {
            ini: parse_ini(text),
        }
    }

    pub fn get(&self, section: &str, key: &str) -> &str {
        self.ini
            .get(&(section.to_string(), key.to_string()))
            .map(String::as_str)
            .or_else(|| {
                DEFAULTS
                    .iter()
                    .find(|(s, k, _)| *s == section && *k == key)
                    .map(|(_, _, v)| *v)
            })
            .unwrap_or("")
            .trim()
    }

    /// Делает ли эту часть (dock, menubar, notify, spotlight, launchpad)
    /// своя программа shell.
    pub fn shell(&self, part: &str) -> bool {
        self.get(part, "engine") == "shell"
    }

    /// Нужна ли служба shell вообще.
    pub fn shell_needed(&self) -> bool {
        ["dock", "menubar", "notify", "spotlight", "launchpad"]
            .iter()
            .any(|p| self.shell(p))
    }

    pub fn dark(&self) -> bool {
        !self.get("look", "variant").eq_ignore_ascii_case("light")
    }

    pub fn names(&self) -> Names {
        let dark = self.dark();
        let icons_base = if dark {
            "WhiteSur-dark"
        } else {
            "WhiteSur-light"
        };
        Names {
            theme: format!("WhiteSur-{}", if dark { "Dark" } else { "Light" }),
            icons_base: icons_base.to_string(),
            icons: format!("{icons_base}-mac"),
            cursor: "WhiteSur-cursors".into(),
            wallpaper: self
                .get("wallpaper", if dark { "dark" } else { "light" })
                .to_string(),
        }
    }

    /// Программы, скрытые из Launchpad.
    pub fn hidden_apps(&self) -> Vec<String> {
        self.get("launchpad", "hide")
            .split(',')
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Команда для сочетания клавиш и значка Launchpad.
    pub fn shortcut_command(&self, paths: &Paths, action: &str) -> String {
        let program = if self.shell(action) {
            &paths.shell_bin
        } else {
            &paths.tool
        };
        format!("{} {action}", program.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_names() {
        let look = Look::from_text("[look]\nvariant = light\n[dock]\nengine = shell\n");
        assert!(look.shell("dock"));
        assert!(!look.shell("menubar"));
        assert!(look.shell_needed());
        let n = look.names();
        assert_eq!(n.theme, "WhiteSur-Light");
        assert_eq!(n.icons, "WhiteSur-light-mac");
        assert_eq!(n.wallpaper, "Sonoma-light");
        assert_eq!(Look::from_text("").names().theme, "WhiteSur-Dark");
        assert!(
            Look::from_text("")
                .hidden_apps()
                .contains(&"xterm".to_string())
        );
    }
}
