//! Настройки оформления из конфига macos-look (`~/.config/macos-look/config.ini`).
//! Пока macos-look не переписан целиком, тема, обои, блокировка и то, какие
//! части включены, задаются там; чего в файле нет — значения по умолчанию,
//! те же, что у macos-look.

use std::collections::HashMap;
use std::path::PathBuf;

use shell_core::config::parse_ini;

use crate::sys::{home, write_if_changed};

/// Значения по умолчанию: (раздел, ключ, значение).
const DEFAULTS: &[(&str, &str, &str)] = &[
    ("look", "variant", "dark"),
    ("look", "font", "Inter 10"),
    ("wallpaper", "dark", "Sonoma-dark"),
    ("wallpaper", "light", "Sonoma-light"),
    ("lock", "idle_minutes", "10"),
    ("lock", "screen_off_minutes", "15"),
    ("lock", "date_locale", "ru_RU.UTF-8"),
    ("lock", "placeholder", "Введите пароль"),
    (
        "launchpad",
        "hide",
        "rofi, rofi-theme-selector, qv4l2, qvidcap, xterm, uxterm, yad-settings, plank, \
         reflector-simple",
    ),
];

/// Меню яблока: (действие и имя ярлыка, подпись, разделитель перед пунктом).
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

/// Записать, кто делает часть, и в конфиг macos-look: пока он применяет
/// оформление, он должен видеть то же, что и shell, иначе вернёт прежнюю
/// программу.
pub fn set_macos_look_engine(part: &str, value: &str) -> std::io::Result<()> {
    let path = home().join(".config/macos-look/config.ini");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        // Нет macos-look — и согласовывать не с кем.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    write_if_changed(&path, &set_ini_value(&text, part, "engine", value)).map(|_| ())
}

/// Текст ini, в котором `ключ = значение` в `[раздел]` заменено (или добавлено).
/// Остальные строки, комментарии и порядок не меняются.
pub fn set_ini_value(text: &str, section: &str, key: &str, value: &str) -> String {
    let new_line = format!("{key} = {value}");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut current = String::new();
    // Последняя непустая строка нужного раздела — после неё допишем ключ.
    let mut section_end = None;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if let Some(s) = t.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = s.trim().to_string();
            if current == section {
                section_end = Some(i);
            }
        } else if current == section {
            section_end = Some(i);
            if t.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
                lines[i] = new_line;
                return join_lines(lines, text);
            }
        }
    }
    match section_end {
        Some(i) => lines.insert(i + 1, new_line),
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{section}]"));
            lines.push(new_line);
        }
    }
    join_lines(lines, text)
}

fn join_lines(lines: Vec<String>, original: &str) -> String {
    let mut out = lines.join("\n");
    if original.is_empty() || original.ends_with('\n') {
        out.push('\n');
    }
    out
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

    /// Команда для сочетания клавиш и значка Launchpad: своя программа
    /// напрямую (запуск Python добавил бы к открытию ~0,1 с) или rofi через
    /// macos-look.
    pub fn shortcut_command(paths: &Paths, action: &str, shell: bool) -> String {
        let program = if shell { &paths.shell_bin } else { &paths.tool };
        format!("{} {action}", program.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_names() {
        let look = Look::from_text("[look]\nvariant = light\n");
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

    #[test]
    fn set_value_keeps_the_rest() {
        let text = "# шапка\n[dock]\nengine = shell\napps = a, b\n\n[lock]\napps = x\n";
        let out = set_ini_value(text, "dock", "apps", "a, b, c");
        assert_eq!(
            out,
            "# шапка\n[dock]\nengine = shell\napps = a, b, c\n\n[lock]\napps = x\n"
        );

        // Ключа нет — дописывается в конец своего раздела.
        let out = set_ini_value("[dock]\nengine = shell\n\n[lock]\n", "dock", "apps", "a");
        assert_eq!(out, "[dock]\nengine = shell\napps = a\n\n[lock]\n");

        // Раздела нет — появляется в конце.
        let out = set_ini_value("[look]\nfont = Inter", "dock", "apps", "a");
        assert_eq!(out, "[look]\nfont = Inter\n\n[dock]\napps = a");
        assert_eq!(set_ini_value("", "dock", "apps", "a"), "[dock]\napps = a\n");
    }
}
