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
    /// Показывать свой док (`[dock] engine = shell`); иначе док — Plank.
    pub dock: bool,
    /// Своя строка меню (`[menubar] engine = shell`); иначе — панель Xfce.
    pub menubar: bool,
    /// Высота строки меню, иконка меню яблока и формат часов (как у `date`) —
    /// из `[panel]`, как у нынешней панели Xfce.
    pub menubar_height: u32,
    pub menu_icon: String,
    pub clock_format: String,
    /// Свои уведомления (`[notify] engine = shell`); иначе — xfce4-notifyd.
    pub notify: bool,
}

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
            menubar_height: 28,
            menu_icon: "start-here".into(),
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
        let num = |section: &str, key: &str| get(section, key).and_then(|v| v.parse::<u32>().ok());
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
                    .map(|v| v.trim().trim_end_matches(".desktop").to_string())
                    .filter(|v| !v.is_empty())
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
                    && p >= 100
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
        c
    }

    /// Текст своего файла настроек с пояснениями — для первого создания.
    /// Переключатели частей (`dock`, `menubar`, `notify`) сюда не пишутся.
    pub fn to_toml(&self) -> String {
        let s = |v: &str| Value::from(v).to_string();
        let list = |v: &[String]| {
            let items: Vec<String> = v.iter().map(|v| s(v)).collect();
            format!("[{}]", items.join(", "))
        };
        let zoom = self
            .dock_zoom
            .map_or("false".to_string(), |p| p.to_string());
        format!(
            "\
# Настройки оболочки shell. После правки перезапустите службу: macos-look apply menu
# Какие части включены (док, строка меню, уведомления), пока задаёт macos-look:
#   macos-look set dock.engine shell   (или plank; menubar.engine, notify.engine)

[look]
# Шрифт и тема иконок.
font = {font}
icon_theme = {theme}

[menubar]
# Высота в точках, иконка меню яблока (имя в теме или путь к файлу),
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
        )
    }
}

/// Текст файла настроек, в котором `[dock] apps` заменён на `apps`.
/// Пояснения и остальные строки не меняются. Сломанный файл — `None`
/// (не затираем то, что человек, может быть, правит прямо сейчас).
pub fn set_dock_apps(text: &str, apps: &[String]) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    if !doc.contains_table("dock") {
        doc["dock"] = toml_edit::table();
    }
    let new: Array = apps.iter().map(String::as_str).collect();
    match doc["dock"].get_mut("apps").and_then(|v| v.as_value_mut()) {
        // Пробелы и пояснение после значения остаются прежними.
        Some(old) => {
            let decor = old.decor().clone();
            *old = Value::Array(new);
            *old.decor_mut() = decor;
        }
        None => doc["dock"]["apps"] = toml_edit::value(new),
    }
    Some(doc.to_string())
}

/// `[раздел]` и `ключ = значение`; строки с `#` и `;` — комментарии.
fn parse_ini(text: &str) -> HashMap<(String, String), String> {
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
        // Переключатели частей в свой файл не пишутся.
        c.dock = false;
        let text = c.to_toml();
        assert_eq!(Config::from_toml(&text), c, "{text}");
        assert_eq!(Config::from_toml(&text).clock_format, "%H:%M \"x\"");
        assert_eq!(
            Config::from_toml(&Config::default().to_toml()),
            Config::default()
        );
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
