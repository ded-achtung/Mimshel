//! Настройки оболочки: свой файл `~/.config/shell/config.toml`
//! ([`Config::from_toml`]). Первый раз он создаётся из конфига macos-look
//! (`~/.config/macos-look/config.ini`, [`Config::from_macos_look`]), а какие
//! части включены (док, строка меню, уведомления), пока решает macos-look —
//! эти три переключателя служба берёт из его файла.
//! Здесь только разбор и составление текста, файлы читает программа.

use std::collections::HashMap;

use toml_edit::{Array, DocumentMut, Value};

/// Когда прятать док (как `hide_mode` в Plank и macos-look).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockHide {
    /// Никогда: док всегда виден, развёрнутые окна заканчиваются над ним.
    Never,
    /// «Умно»: прятать, когда активное окно заходит на док.
    Smart,
    /// Всегда прятать, показывать у нижнего края экрана.
    Auto,
    /// Прятать, только когда активное окно развёрнуто.
    Maximized,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Основной шрифт.
    pub font: String,
    /// Тема иконок.
    pub icon_theme: String,
    /// Команда терминала для программ с `Terminal=true`; программа идёт после неё.
    pub terminal: Vec<String>,
    /// Сетка Launchpad: столбцы, строки и размер иконки.
    pub launchpad_columns: usize,
    pub launchpad_rows: usize,
    pub launchpad_icon: u32,
    /// Закреплённые в доке программы: идентификаторы ярлыков без `.desktop`.
    pub dock_apps: Vec<String>,
    /// Размер иконки в доке.
    pub dock_icon: u32,
    /// Увеличение под мышью в процентах (`None` — выключено).
    pub dock_zoom: Option<u32>,
    pub dock_hide: DockHide,
    /// Какие части делает shell, а какие — прежние программы (`[parts]`,
    /// см. [`PARTS`]): док (иначе Plank), строка меню (иначе панель Xfce),
    /// уведомления (иначе xfce4-notifyd), Spotlight и Launchpad (иначе rofi).
    pub dock: bool,
    pub menubar: bool,
    pub notify: bool,
    pub spotlight: bool,
    pub launchpad: bool,
    /// Высота строки меню, иконка меню яблока и формат часов (как у `date`) —
    /// из `[panel]`, как у нынешней панели Xfce.
    pub menubar_height: u32,
    pub menu_icon: String,
    pub clock_format: String,
}

/// Части, которые может делать shell: (имя, прежняя программа, что это).
pub const PARTS: &[(&str, &str, &str)] = &[
    ("dock", "plank", "док"),
    ("menubar", "xfce", "строка меню (прежняя — панель Xfce)"),
    ("notify", "xfce", "уведомления (прежние — xfce4-notifyd)"),
    ("spotlight", "rofi", "Spotlight"),
    ("launchpad", "rofi", "Launchpad"),
];

impl Default for Config {
    fn default() -> Config {
        Config {
            font: "Inter".into(),
            icon_theme: "WhiteSur-dark-mac".into(),
            terminal: vec!["xfce4-terminal".into(), "-x".into()],
            launchpad_columns: 7,
            launchpad_rows: 4,
            launchpad_icon: 72,
            dock_apps: [
                "thunar",
                "macos-launchpad",
                "firefox",
                "xfce4-terminal",
                "org.xfce.mousepad",
                "xfce-settings-manager",
            ]
            .map(String::from)
            .to_vec(),
            dock_icon: 48,
            dock_zoom: Some(140),
            dock_hide: DockHide::Smart,
            dock: false,
            menubar: false,
            spotlight: false,
            launchpad: false,
            menubar_height: 28,
            menu_icon: "shell-logo".into(),
            clock_format: "%a %-d %b  %H:%M".into(),
            notify: false,
        }
    }
}

impl Config {
    /// Настройки из текста `config.ini` macos-look поверх значений по умолчанию.
    /// Непонятные значения пропускаются (остаётся значение по умолчанию).
    pub fn from_macos_look(text: &str) -> Config {
        let ini = parse_ini(text);
        let get = |section: &str, key: &str| ini.get(&(section.to_string(), key.to_string()));
        let num = |section: &str, key: &str| {
            let name = match (section, key) {
                ("panel", "height") => "menubar.height".to_string(),
                ("dock", "zoom_percent") => "dock.zoom".to_string(),
                _ => format!("{section}.{key}"),
            };
            get(section, key)
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|&v| number_range(&name).is_none_or(|(min, max)| (min..=max).contains(&v)))
        };
        let mut c = Config::default();

        if let Some(font) = get("look", "font") {
            // «Inter 10» → «Inter»: размер шрифта задаём сами.
            let family = match font.rsplit_once(' ') {
                Some((name, size)) if size.parse::<f32>().is_ok() => name,
                _ => font.as_str(),
            };
            if !family.is_empty() {
                c.font = family.to_string();
            }
        }
        if let Some(v) = num("launchpad", "columns").filter(|&v| v > 0) {
            c.launchpad_columns = v as usize;
        }
        if let Some(v) = num("launchpad", "rows").filter(|&v| v > 0) {
            c.launchpad_rows = v as usize;
        }
        if let Some(v) = num("launchpad", "icon_size").filter(|&v| v >= 16) {
            c.launchpad_icon = v;
        }
        if let Some(apps) = get("dock", "apps") {
            c.dock_apps = apps
                .split(',')
                .map(|a| a.trim().trim_end_matches(".desktop").to_string())
                .filter(|a| !a.is_empty())
                .collect();
        }
        if let Some(v) = num("dock", "icon_size").filter(|&v| v >= 16) {
            c.dock_icon = v;
        }
        let zoom_on = get("dock", "zoom").map(|v| matches!(v.as_str(), "yes" | "true" | "1"));
        if let Some(p) = num("dock", "zoom_percent").filter(|&p| p >= 100) {
            c.dock_zoom = Some(p);
        }
        if zoom_on == Some(false) {
            c.dock_zoom = None;
        }
        c.dock = get("dock", "engine").is_some_and(|e| e == "shell");
        c.menubar = get("menubar", "engine").is_some_and(|e| e == "shell");
        c.notify = get("notify", "engine").is_some_and(|e| e == "shell");
        c.spotlight = get("spotlight", "engine").is_some_and(|e| e == "shell");
        c.launchpad = get("launchpad", "engine").is_some_and(|e| e == "shell");
        if let Some(v) = num("panel", "height").filter(|&v| v >= 16) {
            c.menubar_height = v;
        }
        if let Some(v) = get("panel", "menu_icon").filter(|v| !v.is_empty()) {
            c.menu_icon = v.clone();
        }
        if let Some(v) = get("panel", "clock_format").filter(|v| !v.is_empty()) {
            c.clock_format = v.clone();
        }
        c.dock_hide = match num("dock", "hide_mode") {
            Some(0) => DockHide::Never,
            Some(2) => DockHide::Auto,
            Some(3) => DockHide::Maximized,
            Some(1) => DockHide::Smart,
            _ => c.dock_hide,
        };
        c
    }
}

impl DockHide {
    /// Слово в файле настроек.
    pub fn name(self) -> &'static str {
        match self {
            DockHide::Never => "never",
            DockHide::Smart => "smart",
            DockHide::Auto => "auto",
            DockHide::Maximized => "maximized",
        }
    }

    fn parse(s: &str) -> Option<DockHide> {
        [
            DockHide::Never,
            DockHide::Smart,
            DockHide::Auto,
            DockHide::Maximized,
        ]
        .into_iter()
        .find(|h| h.name() == s)
    }
}

impl Config {
    /// Делает ли часть shell (`None` — нет такой части).
    pub fn part(&self, name: &str) -> Option<bool> {
        Some(match name {
            "dock" => self.dock,
            "menubar" => self.menubar,
            "notify" => self.notify,
            "spotlight" => self.spotlight,
            "launchpad" => self.launchpad,
            _ => return None,
        })
    }

    pub fn set_part(&mut self, name: &str, shell: bool) {
        match name {
            "dock" => self.dock = shell,
            "menubar" => self.menubar = shell,
            "notify" => self.notify = shell,
            "spotlight" => self.spotlight = shell,
            "launchpad" => self.launchpad = shell,
            _ => {}
        }
    }

    /// Нужна ли служба shell вообще.
    pub fn shell_needed(&self) -> bool {
        PARTS.iter().any(|(name, ..)| self.part(name) == Some(true))
    }

    /// Настройки из своего файла поверх значений по умолчанию. Непонятные
    /// значения и сломанный файл не мешают запуску: берётся значение по
    /// умолчанию.
    pub fn from_toml(text: &str) -> Config {
        let mut c = Config::default();
        let Ok(doc) = text.parse::<DocumentMut>() else {
            return c;
        };
        let get = |section: &str, key: &str| doc.get(section).and_then(|t| t.get(key));
        let num = |section: &str, key: &str| {
            get(section, key)
                .and_then(|v| v.as_integer())
                .and_then(|v| u32::try_from(v).ok())
                .filter(|&v| {
                    number_range(&format!("{section}.{key}"))
                        .is_none_or(|(min, max)| (min..=max).contains(&v))
                })
        };
        let text = |section: &str, key: &str| {
            get(section, key)
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let list = |section: &str, key: &str| {
            get(section, key).and_then(|v| v.as_array()).map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(|v| {
                        if section == "dock" {
                            v.trim().trim_end_matches(".desktop").to_string()
                        } else {
                            v.to_string()
                        }
                    })
                    .filter(|v| section != "dock" || !v.is_empty())
                    .collect::<Vec<_>>()
            })
        };

        if let Some(v) = text("look", "font") {
            c.font = v;
        }
        if let Some(v) = text("look", "icon_theme") {
            c.icon_theme = v;
        }
        if let Some(v) = list("apps", "terminal").filter(|v| !v.is_empty()) {
            c.terminal = v;
        }
        if let Some(v) = num("menubar", "height").filter(|&v| v >= 16) {
            c.menubar_height = v;
        }
        if let Some(v) = text("menubar", "menu_icon") {
            c.menu_icon = v;
        }
        if let Some(v) = text("menubar", "clock_format") {
            c.clock_format = v;
        }
        if let Some(v) = list("dock", "apps") {
            c.dock_apps = v;
        }
        if let Some(v) = num("dock", "icon_size").filter(|&v| v >= 16) {
            c.dock_icon = v;
        }
        // Увеличение: число процентов или `false` — выключено.
        match get("dock", "zoom") {
            Some(v) if v.as_bool() == Some(false) => c.dock_zoom = None,
            Some(v) => {
                if let Some(p) = v.as_integer().and_then(|p| u32::try_from(p).ok())
                    && (100..=300).contains(&p)
                {
                    c.dock_zoom = Some(p);
                }
            }
            None => {}
        }
        if let Some(h) = text("dock", "hide").and_then(|h| DockHide::parse(&h)) {
            c.dock_hide = h;
        }
        if let Some(v) = num("launchpad", "columns").filter(|&v| v > 0) {
            c.launchpad_columns = v as usize;
        }
        if let Some(v) = num("launchpad", "rows").filter(|&v| v > 0) {
            c.launchpad_rows = v as usize;
        }
        if let Some(v) = num("launchpad", "icon_size").filter(|&v| v >= 16) {
            c.launchpad_icon = v;
        }
        for (name, old, _) in PARTS {
            if let Some(v) = get("parts", name).and_then(|v| v.as_str())
                && (v == "shell" || v == *old)
            {
                c.set_part(name, v == "shell");
            }
        }
        c
    }

    /// Текст своего файла настроек с пояснениями — для первого создания.
    pub fn to_toml(&self) -> String {
        let s = |v: &str| Value::from(v).to_string();
        let list = |v: &[String]| {
            let items: Vec<String> = v.iter().map(|v| s(v)).collect();
            format!("[{}]", items.join(", "))
        };
        let zoom = self
            .dock_zoom
            .map_or("false".to_string(), |p| p.to_string());
        let part = |name: &str| {
            let old = PARTS.iter().find(|p| p.0 == name).map_or("", |p| p.1);
            s(if self.part(name) == Some(true) {
                "shell"
            } else {
                old
            })
        };
        format!(
            "\
# Настройки оболочки shell. Менять удобнее командой, она сразу применяет:
#   shell set dock.icon_size 56
# После правки руками: shell apply

[parts]
# Что делает shell, а что — прежние программы:
# dock — \"shell\" или \"plank\"; menubar — \"shell\" или \"xfce\" (панель Xfce);
# notify — \"shell\" или \"xfce\" (xfce4-notifyd); spotlight и launchpad — \"shell\" или \"rofi\".
dock = {p_dock}
menubar = {p_menubar}
notify = {p_notify}
spotlight = {p_spotlight}
launchpad = {p_launchpad}

[look]
# Шрифт и тема иконок.
font = {font}
icon_theme = {theme}

[menubar]
# Высота в точках, значок главного меню (shell-logo — наша планета; имя в теме или путь к файлу),
# формат часов как у команды date: %a — день недели, %-d — число, %b — месяц.
height = {height}
menu_icon = {menu_icon}
clock_format = {clock}

[dock]
# Закреплённые программы — имена ярлыков без .desktop. Меняется и само,
# когда закрепляете или открепляете значок в доке.
apps = {apps}
icon_size = {icon}
# Увеличение под мышью в процентах; false — без увеличения.
zoom = {zoom}
# Когда прятать: never — никогда, smart — когда окно заходит на док,
# auto — всегда, maximized — когда окно развёрнуто.
hide = {hide}

[launchpad]
columns = {columns}
rows = {rows}
icon_size = {lp_icon}

[apps]
# Чем открывать программы, которым нужен терминал; программа идёт следом.
terminal = {terminal}
",
            font = s(&self.font),
            theme = s(&self.icon_theme),
            height = self.menubar_height,
            menu_icon = s(&self.menu_icon),
            clock = s(&self.clock_format),
            apps = list(&self.dock_apps),
            icon = self.dock_icon,
            hide = s(self.dock_hide.name()),
            columns = self.launchpad_columns,
            rows = self.launchpad_rows,
            lp_icon = self.launchpad_icon,
            terminal = list(&self.terminal),
            p_dock = part("dock"),
            p_menubar = part("menubar"),
            p_notify = part("notify"),
            p_spotlight = part("spotlight"),
            p_launchpad = part("launchpad"),
        )
    }
}

/// Текст файла настроек, в котором `[раздел] ключ` заменён на `value` (или
/// добавлен). Пояснения и остальные строки не меняются. Сломанный файл —
/// `None` (не затираем то, что человек, может быть, правит прямо сейчас).
pub fn set_value(text: &str, section: &str, key: &str, value: Value) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    if !doc.contains_table(section) {
        doc[section] = toml_edit::table();
    }
    match doc[section].get_mut(key).and_then(|v| v.as_value_mut()) {
        // Пробелы и пояснение после значения остаются прежними.
        Some(old) => {
            let decor = old.decor().clone();
            *old = value;
            *old.decor_mut() = decor;
        }
        None => doc[section][key] = toml_edit::value(value),
    }
    Some(doc.to_string())
}

/// [`set_value`] для `[dock] apps`.
pub fn set_dock_apps(text: &str, apps: &[String]) -> Option<String> {
    let new: Array = apps.iter().map(String::as_str).collect();
    set_value(text, "dock", "apps", Value::Array(new))
}

/// Есть ли в файле раздел `[parts]` (в файлах, созданных до него, нет).
pub fn has_parts(text: &str) -> bool {
    text.parse::<DocumentMut>()
        .is_ok_and(|d| d.contains_table("parts"))
}

/// Настройки, которые меняет `shell set`: (раздел.ключ, что это).
pub const SETTINGS: &[(&str, &str)] = &[
    ("parts.dock", "\"shell\" или \"plank\""),
    ("parts.menubar", "\"shell\" или \"xfce\""),
    ("parts.notify", "\"shell\" или \"xfce\""),
    ("parts.spotlight", "\"shell\" или \"rofi\""),
    ("parts.launchpad", "\"shell\" или \"rofi\""),
    ("look.font", "название шрифта"),
    ("look.icon_theme", "тема иконок"),
    ("menubar.height", "высота строки меню, 16–128"),
    (
        "menubar.menu_icon",
        "значок главного меню: shell-logo (планета) или имя из темы",
    ),
    ("menubar.clock_format", "формат часов, как у date"),
    ("dock.apps", "программы через запятую"),
    ("dock.icon_size", "размер значков, 16–256"),
    ("dock.zoom", "увеличение в процентах (100–300) или false"),
    ("dock.hide", "never, smart, auto или maximized"),
    ("launchpad.columns", "столбцов, 1–32"),
    ("launchpad.rows", "строк, 1–32"),
    ("launchpad.icon_size", "размер значков, 16–256"),
    ("apps.terminal", "команда терминала через пробел"),
];

/// Значение для файла из того, что набрали в `shell set` (`раздел.ключ`).
pub fn setting_value(key: &str, input: &str) -> Result<Value, String> {
    let input = input.trim();
    if let Some((min, max)) = number_range(key)
        && !(key == "dock.zoom" && matches!(input, "false" | "no"))
    {
        return match input.parse::<u32>() {
            Ok(n) if (min..=max).contains(&n) => Ok(Value::from(i64::from(n))),
            _ => Err(format!("нужно целое число от {min} до {max}")),
        };
    }
    let words = |list: &[&str]| {
        if list.contains(&input) {
            Ok(Value::from(input))
        } else {
            Err(format!("можно: {}", list.join(", ")))
        }
    };
    match key {
        _ if key.starts_with("parts.") => {
            let name = &key["parts.".len()..];
            let (_, old, _) = PARTS
                .iter()
                .find(|p| p.0 == name)
                .ok_or_else(|| format!("нет части {name}"))?;
            words(&["shell", old])
        }
        "look.font" | "look.icon_theme" | "menubar.menu_icon" | "menubar.clock_format" => {
            if input.is_empty() {
                Err("пустое значение".into())
            } else {
                Ok(Value::from(input))
            }
        }
        "dock.zoom" if input == "false" || input == "no" => Ok(Value::from(false)),
        "dock.hide" => words(&["never", "smart", "auto", "maximized"]),
        "dock.apps" => Ok(Value::Array(
            input
                .split(',')
                .map(|a| a.trim().trim_end_matches(".desktop"))
                .filter(|a| !a.is_empty())
                .collect(),
        )),
        "apps.terminal" if !input.is_empty() => {
            Ok(Value::Array(input.split_whitespace().collect()))
        }
        _ if SETTINGS.iter().any(|s| s.0 == key) => Err("пустое значение".into()),
        _ => Err(format!("нет настройки {key}")),
    }
}

/// Разумные пределы размеров: защищают от огромных буферов отрисовки.
fn number_range(key: &str) -> Option<(u32, u32)> {
    match key {
        "menubar.height" => Some((16, 128)),
        "dock.icon_size" | "launchpad.icon_size" => Some((16, 256)),
        "dock.zoom" => Some((100, 300)),
        "launchpad.columns" | "launchpad.rows" => Some((1, 32)),
        _ => None,
    }
}

/// Проверка синтаксиса, известных ключей, типов и допустимых значений.
/// Только прошедший её файл можно сохранять как последнюю исправную копию.
pub fn toml_error(text: &str) -> Option<String> {
    let doc = match text.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(e) => return Some(e.to_string().trim().to_string()),
    };
    for (section, table) in doc.iter() {
        if !SETTINGS
            .iter()
            .any(|(key, _)| key.split_once('.').is_some_and(|(s, _)| s == section))
        {
            return Some(format!("неизвестный раздел {section}"));
        }
        let Some(table) = table.as_table_like() else {
            return Some(format!("{section}: нужен раздел настроек"));
        };
        for (name, item) in table.iter() {
            let key = format!("{section}.{name}");
            if !SETTINGS.iter().any(|(k, _)| *k == key) {
                return Some(format!("неизвестная настройка {key}"));
            }
            let valid = if let Some((min, max)) = number_range(&key) {
                (key == "dock.zoom" && item.as_bool() == Some(false))
                    || item
                        .as_integer()
                        .is_some_and(|n| (i64::from(min)..=i64::from(max)).contains(&n))
            } else if key == "dock.apps" || key == "apps.terminal" {
                item.as_array().is_some_and(|a| {
                    a.iter().all(|v| {
                        v.as_str()
                            .is_some_and(|s| key == "apps.terminal" || !s.trim().is_empty())
                    }) && (key != "apps.terminal"
                        || a.get(0)
                            .and_then(Value::as_str)
                            .is_some_and(|s| !s.trim().is_empty()))
                })
            } else {
                item.as_str().is_some_and(|v| {
                    setting_value(&key, v).is_ok_and(|parsed| parsed.as_str() == Some(v))
                })
            };
            if !valid {
                return Some(format!("{key}: недопустимый тип или значение"));
            }
        }
    }
    None
}

/// `[раздел]` и `ключ = значение`; строки с `#` и `;` — комментарии.
pub fn parse_ini(text: &str) -> HashMap<(String, String), String> {
    let mut out = HashMap::new();
    let mut section = String::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = s.trim().to_string();
        } else if let Some((k, v)) = line.split_once('=') {
            out.insert(
                (section.clone(), k.trim().to_string()),
                v.trim().to_string(),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_types_ranges_and_unknown_keys() {
        assert!(toml_error(&Config::default().to_toml()).is_none());
        assert!(toml_error("[dock]\napps = []\nzoom = false").is_none());
        for text in [
            "[parts]\ndock = \"shel\"",
            "[dock]\nicon_size = 4294967295",
            "[dock]\nzoom = true",
            "[dock]\nzoom = 301",
            "[launchpad]\nrows = 33",
            "[launchpad]\ncolumns = 0",
            "[dock]\nicon_size = \"48\"",
            "[apps]\nterminal = []",
            "[apps]\nterminal = [1]",
            "[apps]\nterminal = [\"\"]",
            "[dock]\napps = [1]",
            "[dock]\nicon_szie = 48",
            "[dok]\napps = []",
            "dock = 4",
        ] {
            assert!(toml_error(text).is_some(), "{text}");
        }
        assert!(setting_value("dock.icon_size", "4294967296").is_err());
        assert!(setting_value("launchpad.rows", "33").is_err());
        assert_eq!(
            Config::from_toml("[dock]\nicon_size = 4294967295").dock_icon,
            48
        );
        assert_eq!(
            Config::from_macos_look("[dock]\nicon_size = 4294967295").dock_icon,
            48
        );
    }

    #[test]
    fn terminal_arguments_are_preserved() {
        let text = "[apps]\nterminal = [\"terminal.desktop\", \"\", \" spaced \"]";
        assert!(toml_error(text).is_none());
        assert_eq!(
            Config::from_toml(text).terminal,
            ["terminal.desktop", "", " spaced "]
        );
    }

    #[test]
    fn reads_macos_look_config() {
        let c = Config::from_macos_look(
            "# комментарий\n[look]\nfont = Inter Display 11\n\n[dock]\nicon_size = 56\n\
             zoom = yes\nzoom_percent = 160\nhide_mode = 0\napps = thunar, firefox.desktop,\n\
             engine = shell\n\n\
             [launchpad]\ncolumns = 6\nrows = 5\n",
        );
        assert_eq!(c.font, "Inter Display");
        assert_eq!(c.dock_icon, 56);
        assert_eq!(c.dock_zoom, Some(160));
        assert_eq!(c.dock_hide, DockHide::Never);
        assert!(c.dock);
        assert_eq!(c.dock_apps, ["thunar", "firefox"]);
        assert_eq!(
            (c.launchpad_columns, c.launchpad_rows, c.launchpad_icon),
            (6, 5, 72)
        );
    }

    #[test]
    fn zoom_off_and_bad_values() {
        let c = Config::from_macos_look(
            "[dock]\nzoom = no\nzoom_percent = 150\nicon_size = abc\nhide_mode = 9\n",
        );
        assert_eq!(c.dock_zoom, None);
        assert_eq!(c.dock_icon, 48);
        assert_eq!(c.dock_hide, DockHide::Smart);
        assert_eq!(Config::from_macos_look("").dock_apps.len(), 6);
    }

    #[test]
    fn toml_round_trip() {
        let mut c = Config::from_macos_look(
            "[dock]\napps = thunar, firefox\nzoom = no\nhide_mode = 2\n\
             [panel]\nclock_format = %H:%M \"x\"\n",
        );
        c.notify = true;
        let text = c.to_toml();
        assert_eq!(Config::from_toml(&text), c, "{text}");
        assert_eq!(Config::from_toml(&text).clock_format, "%H:%M \"x\"");
        assert_eq!(
            Config::from_toml(&Config::default().to_toml()),
            Config::default()
        );
    }

    #[test]
    fn settings_from_command() {
        let text = Config::default().to_toml();
        assert!(has_parts(&text));
        assert!(!has_parts("[dock]\n"));
        let set = |text: &str, key: &str, input: &str| {
            let (section, k) = key.split_once('.').unwrap();
            set_value(text, section, k, setting_value(key, input).unwrap()).unwrap()
        };
        let text = set(&text, "parts.dock", "shell");
        let text = set(&text, "dock.zoom", "false");
        let text = set(&text, "dock.apps", "thunar, firefox.desktop");
        let text = set(&text, "apps.terminal", "kitty -e");
        let c = Config::from_toml(&text);
        assert!(c.dock && !c.menubar);
        assert_eq!(c.dock_zoom, None);
        assert_eq!(c.dock_apps, ["thunar", "firefox"]);
        assert_eq!(c.terminal, ["kitty", "-e"]);
        assert!(
            text.contains("# Увеличение под мышью"),
            "пояснения остались"
        );
        assert!(setting_value("parts.dock", "xfce").is_err());
        assert!(setting_value("parts.menubar", "xfce").is_ok());
        assert!(setting_value("dock.icon_size", "8").is_err());
        assert!(setting_value("dock.size", "48").is_err());
    }

    #[test]
    fn toml_bad_values_keep_defaults() {
        let c = Config::from_toml(
            "[dock]\nzoom = 150\nicon_size = \"big\"\nhide = \"sometimes\"\n\
             [menubar]\nheight = -5\n",
        );
        assert_eq!(c.dock_zoom, Some(150));
        assert_eq!(c.dock_icon, 48);
        assert_eq!(c.dock_hide, DockHide::Smart);
        assert_eq!(c.menubar_height, 28);
        assert_eq!(Config::from_toml("это не toml ["), Config::default());
    }

    #[test]
    fn dock_apps_keep_the_rest() {
        let text = "# шапка\n[dock]\napps = [\"a\"]  # пояснение\nicon_size = 48\n";
        let out = set_dock_apps(text, &["a".into(), "b".into()]).unwrap();
        assert_eq!(
            out,
            "# шапка\n[dock]\napps = [\"a\", \"b\"]  # пояснение\nicon_size = 48\n"
        );
        let out = set_dock_apps("[look]\nfont = \"Inter\"\n", &["a".into()]).unwrap();
        assert_eq!(Config::from_toml(&out).dock_apps, ["a"]);
        assert_eq!(Config::from_toml(&out).font, "Inter");
        assert!(set_dock_apps("[dock\n", &[]).is_none());
    }
}
