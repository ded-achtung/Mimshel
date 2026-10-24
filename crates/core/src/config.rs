//! Настройки оболочки: свой файл `~/.config/shell/config.toml`
//! ([`Config::from_toml`]); нет файла — он создаётся из значений по
//! умолчанию. Здесь только разбор и составление текста, файлы читает
//! программа.

use toml_edit::{Array, DocumentMut, Value};

/// Когда прятать док.
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
    /// Тема окон и программ (GTK и xfwm4), курсор и обои: `space`, `waves`,
    /// `aurora` — свои, нарисованные программой, или путь к картинке.
    pub gtk_theme: String,
    pub cursor_theme: String,
    pub wallpaper: String,
    /// Масштаб для крупных экранов (HiDPI): сколько пикселей в точке.
    /// `None` — «auto»: по DPI системы (Xfce: Внешний вид → Шрифты → DPI).
    pub scale: Option<f32>,
    /// Команда терминала для программ с `Terminal=true`; программа идёт после неё.
    pub terminal: Vec<String>,
    /// Сетка окна «Программы»: столбцы, строки и размер иконки.
    pub apps_columns: usize,
    pub apps_rows: usize,
    pub apps_icon: u32,
    /// Закреплённые в доке программы: идентификаторы ярлыков без `.desktop`.
    pub dock_apps: Vec<String>,
    /// Размер иконки в доке.
    pub dock_icon: u32,
    /// Увеличение под мышью в процентах (`None` — выключено).
    pub dock_zoom: Option<u32>,
    pub dock_hide: DockHide,
    /// Какие части делает shell, а какие — прежние программы (`[parts]`,
    /// см. [`PARTS`]): док (иначе Plank), строка меню (иначе панель Xfce),
    /// уведомления (иначе xfce4-notifyd), окна «Поиск» и «Программы» (иначе rofi).
    pub dock: bool,
    pub menubar: bool,
    pub notify: bool,
    /// Рабочий стол: обои и меню по правому щелчку (иначе xfdesktop).
    pub desktop: bool,
    /// Окна папок — свой Проводник (иначе Thunar).
    pub folders: bool,
    pub search_panel: bool,
    pub apps_panel: bool,
    /// Высота строки меню, иконка меню яблока и формат часов (как у `date`) —
    /// из `[panel]`, как у нынешней панели Xfce.
    pub menubar_height: u32,
    pub menu_icon: String,
    pub clock_format: String,
    /// Экран блокировки: подсказка в поле пароля, через сколько минут
    /// бездействия блокировать экран и через сколько гасить.
    pub lock_placeholder: String,
    pub lock_idle: u32,
    pub lock_screen_off: u32,
    /// Программы, которые не показываются в окнах «Программы» и «Поиск»
    /// (остаются установленными и запускаются как раньше).
    pub hide_apps: Vec<String>,
    /// Названия программ в строке меню, окне «Программы» и доке:
    /// (идентификатор ярлыка, новое название).
    pub rename_apps: Vec<(String, String)>,
}

/// Части, которые может делать shell: (имя, прежняя программа, что это).
pub const PARTS: &[(&str, &str, &str)] = &[
    ("dock", "plank", "док"),
    ("menubar", "xfce", "строка меню (прежняя — панель Xfce)"),
    ("notify", "xfce", "уведомления (прежние — xfce4-notifyd)"),
    ("desktop", "xfce", "рабочий стол (прежний — xfdesktop)"),
    (
        "folders",
        "thunar",
        "окна папок — Проводник (прежний — Thunar)",
    ),
    ("search_panel", "rofi", "окно «Поиск»"),
    ("apps_panel", "rofi", "окно «Программы»"),
];

impl Default for Config {
    fn default() -> Config {
        Config {
            font: "Inter".into(),
            icon_theme: "Colloid-shell".into(),
            gtk_theme: "Colloid-Dark".into(),
            cursor_theme: "Colloid-cursors".into(),
            wallpaper: "space".into(),
            scale: None,
            terminal: vec!["xfce4-terminal".into(), "-x".into()],
            apps_columns: 7,
            apps_rows: 4,
            apps_icon: 72,
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
            search_panel: false,
            apps_panel: false,
            menubar_height: 28,
            menu_icon: "shell-logo".into(),
            clock_format: "%a %-d %b  %H:%M".into(),
            notify: false,
            desktop: false,
            folders: false,
            lock_placeholder: "Введите пароль".into(),
            lock_idle: 10,
            lock_screen_off: 15,
            hide_apps: [
                "rofi",
                "rofi-theme-selector",
                "qv4l2",
                "qvidcap",
                "xterm",
                "uxterm",
                "yad-settings",
                "plank",
                "reflector-simple",
            ]
            .map(String::from)
            .to_vec(),
            rename_apps: [
                ("thunar", "Проводник"),
                ("xfce4-terminal", "Терминал"),
                ("org.xfce.mousepad", "Блокнот"),
                ("xfce-settings-manager", "Системные настройки"),
                ("xfce4-taskmanager", "Мониторинг системы"),
            ]
            .map(|(a, n)| (a.to_string(), n.to_string()))
            .to_vec(),
        }
    }
}

/// Список через запятую → вектор непустых строк.
fn comma_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect()
}

/// «ярлык: Новое имя, ярлык2: Имя2» → вектор пар.
fn parse_renames(text: &str) -> Vec<(String, String)> {
    text.split(',')
        .filter_map(|pair| {
            let (app, name) = pair.split_once(':')?;
            let (app, name) = (app.trim(), name.trim());
            (!app.is_empty() && !name.is_empty()).then(|| (app.to_string(), name.to_string()))
        })
        .collect()
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
            "desktop" => self.desktop,
            "folders" => self.folders,
            "search_panel" => self.search_panel,
            "apps_panel" => self.apps_panel,
            _ => return None,
        })
    }

    pub fn set_part(&mut self, name: &str, shell: bool) {
        match name {
            "dock" => self.dock = shell,
            "menubar" => self.menubar = shell,
            "notify" => self.notify = shell,
            "desktop" => self.desktop = shell,
            "folders" => self.folders = shell,
            "search_panel" => self.search_panel = shell,
            "apps_panel" => self.apps_panel = shell,
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
                            dock_entry(v.trim()).to_string()
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
        if let Some(v) = text("look", "gtk_theme") {
            c.gtk_theme = v;
        }
        if let Some(v) = text("look", "cursor_theme") {
            c.cursor_theme = v;
        }
        if let Some(v) = text("look", "wallpaper") {
            c.wallpaper = v;
        }
        if let Some(v) = text("look", "icon_theme") {
            c.icon_theme = v;
        }
        if let Some(v) = get("look", "scale")
            .and_then(|i| i.as_value())
            .and_then(scale_value)
        {
            c.scale = v;
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
        if let Some(v) = num("apps_panel", "columns").filter(|&v| v > 0) {
            c.apps_columns = v as usize;
        }
        if let Some(v) = num("apps_panel", "rows").filter(|&v| v > 0) {
            c.apps_rows = v as usize;
        }
        if let Some(v) = num("apps_panel", "icon_size").filter(|&v| v >= 16) {
            c.apps_icon = v;
        }
        if let Some(v) = text("lock", "placeholder") {
            c.lock_placeholder = v;
        }
        if let Some(v) = num("lock", "idle_minutes").filter(|&v| v > 0) {
            c.lock_idle = v;
        }
        if let Some(v) = num("lock", "screen_off_minutes").filter(|&v| v > 0) {
            c.lock_screen_off = v;
        }
        if let Some(v) = list("apps", "hide") {
            c.hide_apps = v;
        }
        if let Some(table) = get("apps", "rename").and_then(|v| v.as_inline_table()) {
            let renames: Vec<(String, String)> = table
                .iter()
                .filter_map(|(app, name)| {
                    let name = name.as_str()?.trim();
                    let app = app.trim();
                    (!app.is_empty() && !name.is_empty())
                        .then(|| (app.to_string(), name.to_string()))
                })
                .collect();
            c.rename_apps = renames;
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
# notify — \"shell\" или \"xfce\" (xfce4-notifyd);
# search_panel (окно «Поиск») и apps_panel (окно «Программы») — \"shell\" или \"rofi\".
# desktop (рабочий стол: обои, меню по правому щелчку) — \"shell\" или \"xfce\" (xfdesktop).
# folders (окна папок, Проводник) — \"shell\" или \"thunar\".
dock = {p_dock}
menubar = {p_menubar}
notify = {p_notify}
desktop = {p_desktop}
folders = {p_folders}
search_panel = {p_search}
apps_panel = {p_apps}

[look]
# Шрифт, тема значков, тема окон, курсор и обои (space, waves, aurora —
# свои, нарисованные программой, или путь к картинке). Применить: shell apply look wallpaper
font = {font}
icon_theme = {theme}
gtk_theme = {gtk_theme}
cursor_theme = {cursor_theme}
wallpaper = {wallpaper}
# Масштаб для крупных экранов: \"auto\" — по DPI системы, или число от 1 до 3
# (1.5 — всё в полтора раза крупнее). Меняется командой shell set look.scale 1.5
scale = {scale}

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

[apps_panel]
# Окно «Программы»: столбцов и строк на странице, размер значков.
columns = {columns}
rows = {rows}
icon_size = {lp_icon}

[lock]
# Подсказка в поле пароля; через сколько минут бездействия блокировать экран
# и через сколько гасить его (после блокировки). Применить: shell apply lock
placeholder = {placeholder}
idle_minutes = {idle}
screen_off_minutes = {screen_off}

[apps]
# Чем открывать программы, которым нужен терминал; программа идёт следом.
terminal = {terminal}
# Служебные программы, которых не видно в AppsPanel и SearchPanel
# (сами остаются установленными).
hide = {hide_apps}
# Названия программ в строке меню, окне «Программы» и доке: ярлык = «Новое имя».
rename = {rename}
",
            font = s(&self.font),
            theme = s(&self.icon_theme),
            gtk_theme = s(&self.gtk_theme),
            cursor_theme = s(&self.cursor_theme),
            wallpaper = s(&self.wallpaper),
            scale = self
                .scale
                .map_or_else(|| s("auto"), |k| Value::from(f64::from(k)).to_string()),
            height = self.menubar_height,
            menu_icon = s(&self.menu_icon),
            clock = s(&self.clock_format),
            apps = list(&self.dock_apps),
            icon = self.dock_icon,
            hide = s(self.dock_hide.name()),
            columns = self.apps_columns,
            rows = self.apps_rows,
            lp_icon = self.apps_icon,
            placeholder = s(&self.lock_placeholder),
            idle = self.lock_idle,
            screen_off = self.lock_screen_off,
            terminal = list(&self.terminal),
            hide_apps = list(&self.hide_apps),
            rename = renames_to_toml(&self.rename_apps),
            p_dock = part("dock"),
            p_menubar = part("menubar"),
            p_notify = part("notify"),
            p_desktop = part("desktop"),
            p_folders = part("folders"),
            p_search = part("search_panel"),
            p_apps = part("apps_panel"),
        )
    }
}

/// Масштаб из файла: `"auto"` — `Some(None)`, число от 1 до 3 — `Some(Some(k))`,
/// иначе `None` (ошибка).
fn scale_value(v: &Value) -> Option<Option<f32>> {
    if v.as_str() == Some("auto") {
        return Some(None);
    }
    let k = v.as_float().or_else(|| v.as_integer().map(|n| n as f64))?;
    (1.0..=3.0).contains(&k).then_some(Some(k as f32))
}

/// Инлайн-таблица `rename` для текста настроек: { ярлык = "Имя", … }.
fn renames_to_toml(renames: &[(String, String)]) -> String {
    let s = |v: &str| Value::from(v).to_string();
    let items: Vec<String> = renames
        .iter()
        .map(|(app, name)| format!("{} = {}", s(app), s(name)))
        .collect();
    format!("{{ {} }}", items.join(", "))
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

/// Запись значка в `[dock] apps`: имя ярлыка без одного «.desktop». Только
/// одного: у Telegram ярлык
/// `org.telegram.desktop.desktop`, и запись — `org.telegram.desktop`.
pub fn dock_entry(id: &str) -> &str {
    id.strip_suffix(".desktop").unwrap_or(id)
}

/// Подходит ли запись из `[dock] apps` ярлыку `id` (запись может быть
/// и полным именем файла).
pub fn dock_entry_matches(entry: &str, id: &str) -> bool {
    id == entry || dock_entry(id) == entry
}

/// [`set_value`] для `[dock] apps`.
pub fn set_dock_apps(text: &str, apps: &[String]) -> Option<String> {
    let new: Array = apps.iter().map(String::as_str).collect();
    set_value(text, "dock", "apps", Value::Array(new))
}

/// Прежние имена в файле настроек (до 24.10.2026 окна «Поиск» и «Программы»
/// назывались по-маковски): (раздел, старый ключ или `None` — весь раздел,
/// новое имя).
const RENAMED: &[(&str, Option<&str>, &str)] = &[
    ("parts", Some("spotlight"), "search_panel"),
    ("parts", Some("launchpad"), "apps_panel"),
    ("launchpad", None, "apps_panel"),
];

/// Новое имя настройки для прежнего (`parts.spotlight` → `parts.search_panel`,
/// `launchpad.rows` → `apps_panel.rows`); остальные — как есть: прежние
/// имена могли остаться в чьих-то записках и скриптах.
pub fn current_key(key: &str) -> String {
    let Some((section, name)) = key.split_once('.') else {
        return key.to_string();
    };
    for (s, k, to) in RENAMED {
        match k {
            Some(k) if *s == section && *k == name => return format!("{s}.{to}"),
            None if *s == section => return format!("{to}.{name}"),
            _ => {}
        }
    }
    key.to_string()
}

/// Пояснение в `[parts]` прежних файлов и новое вместо него.
const OLD_PARTS_COMMENT: &str = "# notify — \"shell\" или \"xfce\" (xfce4-notifyd); spotlight и launchpad — \"shell\" или \"rofi\".";
const NEW_PARTS_COMMENT: &str = "# notify — \"shell\" или \"xfce\" (xfce4-notifyd);\n\
     # search_panel (окно «Поиск») и apps_panel (окно «Программы») — \"shell\" или \"rofi\".";

/// Переименовать прежние ключи файла настроек в новые, сохранив значения и
/// пояснения (своё прежнее пояснение о частях — заменить). `None` —
/// переименовывать нечего.
pub fn migrate_names(text: &str) -> Option<String> {
    let mut section = String::new();
    let mut changed = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        let mut new = None;
        if trimmed == OLD_PARTS_COMMENT {
            new = Some(line.replacen(OLD_PARTS_COMMENT, NEW_PARTS_COMMENT, 1));
        } else if let Some(name) = trimmed.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            section = name.trim().to_string();
            if let Some((.., to)) = RENAMED
                .iter()
                .find(|(s, k, _)| k.is_none() && *s == section)
            {
                new = Some(line.replacen(name.trim(), to, 1));
            }
        } else if let Some((key, _)) = trimmed.split_once('=')
            && let Some((.., to)) = RENAMED
                .iter()
                .find(|(s, k, _)| *s == section && *k == Some(key.trim()))
        {
            new = Some(line.replacen(key.trim(), to, 1));
        }
        match new {
            Some(l) => {
                changed = true;
                out.push_str(&l);
            }
            None => out.push_str(line),
        }
    }
    changed.then_some(out)
}

/// Есть ли в файле раздел `[lock]` (в файлах, созданных до него, нет).
pub fn has_lock(text: &str) -> bool {
    text.parse::<DocumentMut>()
        .is_ok_and(|d| d.contains_table("lock"))
}

/// Дописать в старый файл настройки блокировки, скрытия и переименования
/// программ (`base` — откуда взять значения: прежний конфиг или умолчания).
pub fn add_lock_settings(text: &str, base: &Config) -> Option<String> {
    let mut out = set_value(
        text,
        "lock",
        "placeholder",
        Value::from(base.lock_placeholder.as_str()),
    )?;
    out = set_value(
        &out,
        "lock",
        "idle_minutes",
        Value::from(i64::from(base.lock_idle)),
    )?;
    out = set_value(
        &out,
        "lock",
        "screen_off_minutes",
        Value::from(i64::from(base.lock_screen_off)),
    )?;
    let hide: Array = base.hide_apps.iter().map(String::as_str).collect();
    out = set_value(&out, "apps", "hide", Value::Array(hide))?;
    let mut table = toml_edit::InlineTable::new();
    for (app, name) in &base.rename_apps {
        table.insert(app, Value::from(name.as_str()));
    }
    set_value(&out, "apps", "rename", Value::InlineTable(table))
}

/// Настройки, которые меняет `shell set`: (раздел.ключ, что это).
pub const SETTINGS: &[(&str, &str)] = &[
    ("parts.dock", "\"shell\" или \"plank\""),
    ("parts.menubar", "\"shell\" или \"xfce\""),
    ("parts.notify", "\"shell\" или \"xfce\""),
    ("parts.search_panel", "окно «Поиск»: \"shell\" или \"rofi\""),
    (
        "parts.apps_panel",
        "окно «Программы»: \"shell\" или \"rofi\"",
    ),
    (
        "parts.desktop",
        "рабочий стол: \"shell\" или \"xfce\" (xfdesktop)",
    ),
    (
        "parts.folders",
        "окна папок (Проводник): \"shell\" или \"thunar\"",
    ),
    ("look.font", "название шрифта"),
    ("look.icon_theme", "тема иконок"),
    (
        "look.gtk_theme",
        "тема окон и программ, например Colloid-Dark",
    ),
    ("look.cursor_theme", "курсор, например Colloid-cursors"),
    (
        "look.wallpaper",
        "обои: space, waves, aurora или путь к картинке",
    ),
    (
        "look.scale",
        "масштаб для крупных экранов: auto (по DPI) или от 1 до 3, например 1.5",
    ),
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
    ("apps_panel.columns", "окно «Программы»: столбцов, 1–32"),
    ("apps_panel.rows", "окно «Программы»: строк, 1–32"),
    (
        "apps_panel.icon_size",
        "окно «Программы»: размер значков, 16–256",
    ),
    ("lock.placeholder", "подсказка в поле пароля"),
    (
        "lock.idle_minutes",
        "блокировать экран после бездействия, минут (1–600)",
    ),
    (
        "lock.screen_off_minutes",
        "гасить экран после блокировки, минут (1–600)",
    ),
    ("apps.terminal", "команда терминала через пробел"),
    (
        "apps.hide",
        "программы, скрытые из окон «Программы» и «Поиск», через запятую",
    ),
    (
        "apps.rename",
        "названия программ: «ярлык: Новое имя» через запятую",
    ),
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
        "look.font"
        | "look.icon_theme"
        | "look.gtk_theme"
        | "look.cursor_theme"
        | "look.wallpaper"
        | "menubar.menu_icon"
        | "menubar.clock_format"
        | "lock.placeholder" => {
            if input.is_empty() {
                Err("пустое значение".into())
            } else {
                Ok(Value::from(input))
            }
        }
        "look.scale" => match input.replace(',', ".").as_str() {
            "auto" => Ok(Value::from("auto")),
            v => match v.parse::<f64>() {
                Ok(k) if (1.0..=3.0).contains(&k) => Ok(Value::from((k * 100.0).round() / 100.0)),
                _ => Err("нужно auto или число от 1 до 3, например 1.5".into()),
            },
        },
        "dock.zoom" if input == "false" || input == "no" => Ok(Value::from(false)),
        "dock.hide" => words(&["never", "smart", "auto", "maximized"]),
        "dock.apps" => Ok(Value::Array(
            input
                .split(',')
                .map(|a| dock_entry(a.trim()))
                .filter(|a| !a.is_empty())
                .collect(),
        )),
        // Пустой список — ничего не скрывать (и не переименовывать).
        "apps.hide" => Ok(Value::Array(comma_list(input).into_iter().collect())),
        "apps.rename" => {
            let mut table = toml_edit::InlineTable::new();
            for (app, name) in parse_renames(input) {
                table.insert(&app, Value::from(name));
            }
            Ok(Value::InlineTable(table))
        }
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
        "dock.icon_size" | "apps_panel.icon_size" => Some((16, 256)),
        "dock.zoom" => Some((100, 300)),
        "apps_panel.columns" | "apps_panel.rows" => Some((1, 32)),
        "lock.idle_minutes" | "lock.screen_off_minutes" => Some((1, 600)),
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
            } else if key == "dock.apps" || key == "apps.terminal" || key == "apps.hide" {
                item.as_array().is_some_and(|a| {
                    a.iter().all(|v| {
                        v.as_str()
                            .is_some_and(|s| key == "apps.terminal" || !s.trim().is_empty())
                    }) && (key != "apps.terminal"
                        || a.get(0)
                            .and_then(Value::as_str)
                            .is_some_and(|s| !s.trim().is_empty()))
                })
            } else if key == "look.scale" {
                item.as_value().and_then(scale_value).is_some()
            } else if key == "apps.rename" {
                item.as_inline_table().is_some_and(|t| {
                    t.iter().all(|(app, name)| {
                        !app.trim().is_empty()
                            && name.as_str().is_some_and(|n| !n.trim().is_empty())
                    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_auto_or_number() {
        assert_eq!(Config::default().scale, None);
        assert!(Config::default().to_toml().contains("scale = \"auto\""));
        for (text, want) in [
            ("[look]\nscale = \"auto\"", None),
            ("[look]\nscale = 1.5", Some(1.5)),
            ("[look]\nscale = 2", Some(2.0)),
        ] {
            assert!(toml_error(text).is_none(), "{text}");
            assert_eq!(Config::from_toml(text).scale, want, "{text}");
        }
        for text in [
            "[look]\nscale = 5",
            "[look]\nscale = \"big\"",
            "[look]\nscale = 0.5",
        ] {
            assert!(toml_error(text).is_some(), "{text}");
        }
        assert_eq!(
            setting_value("look.scale", "1,5").unwrap().as_float(),
            Some(1.5)
        );
        assert_eq!(
            setting_value("look.scale", "auto").unwrap().as_str(),
            Some("auto")
        );
        assert!(setting_value("look.scale", "4").is_err());
    }

    #[test]
    fn validates_types_ranges_and_unknown_keys() {
        assert!(toml_error(&Config::default().to_toml()).is_none());
        assert!(toml_error("[dock]\napps = []\nzoom = false").is_none());
        for text in [
            "[parts]\ndock = \"shel\"",
            "[dock]\nicon_size = 4294967295",
            "[dock]\nzoom = true",
            "[dock]\nzoom = 301",
            "[apps_panel]\nrows = 33",
            "[apps_panel]\ncolumns = 0",
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
        assert!(setting_value("apps_panel.rows", "33").is_err());
        assert_eq!(
            Config::from_toml("[dock]\nicon_size = 4294967295").dock_icon,
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
    fn old_names_migrate_keeping_values_and_comments() {
        let old = "[parts]\n# что делает shell\n\
                   # notify — \"shell\" или \"xfce\" (xfce4-notifyd); spotlight и launchpad — \"shell\" или \"rofi\".\n\
                   dock = \"shell\"\nspotlight = \"shell\"\n\
                   launchpad = \"rofi\"\n\n[launchpad]\n# сетка\ncolumns = 6\nrows = 5\n";
        assert!(toml_error(old).is_some(), "старые имена — уже не настройки");
        let new = migrate_names(old).unwrap();
        assert!(toml_error(&new).is_none(), "{new}");
        assert!(new.contains("# что делает shell") && new.contains("# сетка"));
        assert!(
            !new.contains("spotlight") && new.contains("окно «Поиск»"),
            "{new}"
        );
        let c = Config::from_toml(&new);
        assert!(c.search_panel && !c.apps_panel && c.dock);
        assert_eq!((c.apps_columns, c.apps_rows), (6, 5));
        // Новый файл не меняется; «launchpad» в значениях не трогаем.
        assert_eq!(migrate_names(&new), None);
        assert_eq!(
            migrate_names("[dock]\napps = [\"macos-launchpad\"]\n"),
            None
        );
        assert_eq!(current_key("parts.spotlight"), "parts.search_panel");
        assert_eq!(current_key("launchpad.rows"), "apps_panel.rows");
        assert_eq!(current_key("dock.apps"), "dock.apps");
    }

    #[test]
    fn toml_round_trip() {
        let mut c = Config {
            dock_apps: vec!["thunar".into(), "firefox".into()],
            dock_zoom: None,
            dock_hide: DockHide::Auto,
            clock_format: "%H:%M \"x\"".into(),
            ..Config::default()
        };
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

    #[test]
    fn lock_hide_rename_round_trip() {
        let c = Config::default();
        let text = c.to_toml();
        assert!(toml_error(&text).is_none(), "{text}");
        assert_eq!(Config::from_toml(&text), c);
        assert_eq!(c.lock_placeholder, "Введите пароль");
        assert!((c.lock_idle, c.lock_screen_off) == (10, 15));
        assert!(c.hide_apps.contains(&"xterm".to_string()));
        assert!(
            c.rename_apps
                .contains(&("thunar".to_string(), "Проводник".to_string()))
        );
    }

    #[test]
    fn lock_hide_rename_from_command() {
        assert!(setting_value("lock.idle_minutes", "0").is_err());
        assert!(setting_value("lock.idle_minutes", "601").is_err());
        assert!(setting_value("lock.idle_minutes", "30").is_ok());
        assert!(setting_value("lock.placeholder", "").is_err());
        let hide = setting_value("apps.hide", "a, b").unwrap();
        let rename = setting_value("apps.rename", "thunar: Проводник, : bad").unwrap();
        let text = set_value("[apps]\n", "apps", "hide", hide).unwrap();
        let text = set_value(&text, "apps", "rename", rename).unwrap();
        assert!(toml_error(&text).is_none(), "{text}");
        let c = Config::from_toml(&text);
        assert_eq!(c.hide_apps, ["a", "b"]);
        assert_eq!(
            c.rename_apps,
            [("thunar".to_string(), "Проводник".to_string())]
        );
        // Пустые значения — сброс списков.
        let text = set_value(
            &text,
            "apps",
            "hide",
            setting_value("apps.hide", "").unwrap(),
        )
        .unwrap();
        assert!(toml_error(&text).is_none());
        assert!(Config::from_toml(&text).hide_apps.is_empty());
        for bad in [
            "[apps]\nrename = { a = 1 }",
            "[apps]\nrename = [\"a\"]",
            "[apps]\nhide = [1]",
            "[apps]\nhide = \"rofi\"",
            "[lock]\nidle_minutes = 601",
            "[lock]\nplaceholder = 5",
        ] {
            assert!(toml_error(bad).is_some(), "{bad}");
        }
    }
}
