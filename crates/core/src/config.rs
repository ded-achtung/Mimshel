//! Настройки оболочки. Пока только значения по умолчанию; чтение из файла
//! (`~/.config/shell/config.toml`) появится позже.

#[derive(Debug, Clone)]
pub struct Config {
    /// Основной шрифт.
    pub font: String,
    /// Тема иконок.
    pub icon_theme: String,
    /// Команда терминала для программ с `Terminal=true`; программа идёт после неё.
    pub terminal: Vec<String>,
    /// Сетка Launchpad: столбцы, строки и размер иконки (как в macos-look).
    pub launchpad_columns: usize,
    pub launchpad_rows: usize,
    pub launchpad_icon: u32,
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
        }
    }
}
