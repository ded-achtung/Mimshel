//! Настройки оболочки. Пока берутся из конфига macos-look
//! (`~/.config/macos-look/config.ini`) — это единственный конфиг, который
//! сейчас ведёт пользователь; чего там нет — значения по умолчанию.
//! Здесь только разбор текста, файл читает программа.

use std::collections::HashMap;

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

#[derive(Debug, Clone)]
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
}
