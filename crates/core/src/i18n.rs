//! Minimal multilingual support for Mimshel (i18n-en-ru-zh).
//!
//! Design notes (deliberately small, no gettext):
//! * One static table: `key -> (en, ru, zh)` in [`t`]. Russian strings are
//!   the upstream originals, kept verbatim as `ru`, so a `ru` user sees
//!   exactly what upstream showed.
//! * [`Language`] is resolved, never guessed: `auto` (the config default)
//!   follows the system locale; a locale that is not `en`/`ru`/`zh`
//!   falls back to `en`. An explicit `en`/`ru`/`zh` in config always wins
//!   over the locale.
//! * Callers pass a resolved [`Language`] in; there is no global mutable
//!   locale state, so the daemon can reload on `mimshel apply` simply by
//!   re-reading config.
//! * Only the most visible UI strings are in this table (see
//!   `docs/i18n.md` for the translated / not-yet-translated lists).
//!   Everything else still uses the upstream Russian literals — see
//!   `TODO-I18N` in that doc. Do not claim the whole UI is translated.
//!
//! Chinese here is Simplified Chinese (zh-CN style wording, e.g. 废纸篓,
//! 文件管理器, 搜索). Traditional Chinese and Japanese are explicitly
//! out of scope for this branch; adding them is a new table column, not
//! a redesign (see `docs/i18n.md`).

use std::env;

/// Interface language. `Auto` means "follow the system locale".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    /// Follow the system locale (config default).
    #[default]
    Auto,
    En,
    Ru,
    Zh,
}

impl Language {
    /// Config-file word for this language (`auto` / `en` / `ru` / `zh`).
    pub fn name(self) -> &'static str {
        match self {
            Language::Auto => "auto",
            Language::En => "en",
            Language::Ru => "ru",
            Language::Zh => "zh",
        }
    }

    /// Parse a config word. Unknown words are `None` (caller keeps `auto`).
    pub fn parse(s: &str) -> Option<Language> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Language::Auto),
            "en" | "eng" | "english" => Some(Language::En),
            "ru" | "rus" | "russian" => Some(Language::Ru),
            "zh" | "chi" | "chinese" | "zh-cn" | "zh-tw" | "zh-hk" => Some(Language::Zh),
            _ => None,
        }
    }

    /// Language from the system locale.
    ///
    /// Checks `LANGUAGE`, then `LANG`, then `LC_ALL`, then `LC_MESSAGES`
    /// (first entry of a `:`-separated list, first two letters, lowercased).
    /// `zh_*` (including `zh_TW`/`zh_HK` for now) maps to [`Language::Zh`]
    /// (Simplified wording); anything else maps to [`Language::En`].
    pub fn from_env() -> Language {
        for key in ["LANGUAGE", "LANG", "LC_ALL", "LC_MESSAGES"] {
            if let Ok(v) = env::var(key)
                && !v.is_empty()
            {
                let first = v.split(':').next().unwrap_or("").trim();
                let code = first
                    .split(['.', '@'])
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let lang = code.split('_').next().unwrap_or("");
                return match lang {
                    "ru" => Language::Ru,
                    "zh" => Language::Zh,
                    "en" => Language::En,
                    // C / POSIX / anything else: English fallback.
                    _ => Language::En,
                };
            }
        }
        Language::En
    }

    /// Resolve `auto` via the system locale; explicit values pass through.
    pub fn resolve(self) -> Language {
        match self {
            Language::Auto => Language::from_env(),
            other => other,
        }
    }

    /// Translate `key` for this (possibly `auto`) language.
    pub fn text(self, key: &str) -> &str {
        t(self.resolve(), key)
    }
}

/// Translate `key` for a resolved language.
///
/// Unknown keys fall back to the key itself (never an empty string), so a
/// missing translation is visible in review instead of silently blank.
pub fn t(lang: Language, key: &str) -> &str {
    let lang = lang.resolve();
    let (en, ru, zh) = match key {
        // --- App names (also used as Config rename defaults) ---
        "app.terminal" => ("Terminal", "Терминал", "终端"),
        "app.file_manager" => ("File Manager", "Проводник", "文件管理器"),
        "app.xfce_terminal" => ("Xfce Terminal", "Терминал Xfce", "Xfce 终端"),
        "app.text_editor" => ("Text Editor", "Блокнот", "文本编辑器"),
        "app.settings" => ("Settings", "Системные настройки", "系统设置"),
        "app.task_manager" => ("System Monitor", "Мониторинг системы", "系统监视器"),
        "app.home" => ("Home", "Домашняя папка", "主文件夹"),
        // --- Terminal window menus (macOS-style) ---
        "menu.shell" => ("Shell", "Оболочка", "Shell"),
        "menu.edit" => ("Edit", "Правка", "编辑"),
        "menu.view" => ("View", "Вид", "显示"),
        "menu.window" => ("Window", "Окно", "窗口"),
        "term.new_window" => ("New Window", "Новое окно", "新建窗口"),
        "term.new_tab" => ("New Tab", "Новая вкладка", "新建标签页"),
        "term.close_tab" => ("Close Tab", "Закрыть вкладку", "关闭标签页"),
        "term.close_window" => ("Close Window", "Закрыть окно", "关闭窗口"),
        "term.copy" => ("Copy", "Копировать", "复制"),
        "term.paste" => ("Paste", "Вставить", "粘贴"),
        "term.select_all" => ("Select All", "Выделить всё", "全选"),
        "term.clear_history" => ("Clear History", "Очистить историю", "清除历史记录"),
        "term.zoom_in" => ("Zoom In", "Крупнее", "放大"),
        "term.zoom_out" => ("Zoom Out", "Мельче", "缩小"),
        "term.zoom_reset" => ("Actual Size", "Обычный размер", "实际大小"),
        "term.next_tab" => ("Next Tab", "Следующая вкладка", "下一个标签页"),
        "term.prev_tab" => ("Previous Tab", "Предыдущая вкладка", "上一个标签页"),
        // --- Dock ---
        "dock.trash" => ("Trash", "Корзина", "废纸篓"),
        "dock.open" => ("Open", "Открыть", "打开"),
        "dock.new_window" => ("New Window", "Новое окно", "新建窗口"),
        "dock.keep_in_dock" => ("Keep in Dock", "Оставить в Dock", "保留在程序坞"),
        "dock.quit" => ("Quit", "Завершить", "退出"),
        "dock.empty_trash" => ("Empty Trash…", "Очистить Корзину…", "清倒废纸篓…"),
        "dock.run_on_dgpu" => (
            "Run on discrete GPU",
            "Запускать на видеокарте NVIDIA",
            "使用独立显卡运行",
        ),
        // --- Folder / file-manager windows ---
        "folder.places" => ("Places", "Места", "位置"),
        "folder.devices" => ("Devices", "Устройства", "设备"),
        "folder.filesystem" => ("File System", "Файловая система", "文件系统"),
        "folder.open" => ("Open", "Открыть", "打开"),
        "folder.open_in_new_window" => (
            "Open in New Window",
            "Открыть в новом окне",
            "在新窗口中打开",
        ),
        "folder.rename" => ("Rename", "Переименовать", "重命名"),
        "folder.copy" => ("Copy", "Копировать", "复制"),
        "folder.cut" => ("Cut", "Вырезать", "剪切"),
        "folder.move_to_trash" => ("Move to Trash", "В корзину", "移到废纸篓"),
        "folder.new_folder" => ("New Folder", "Новая папка", "新建文件夹"),
        "folder.new_text_file" => ("New Text File", "Новый текстовый файл", "新建文本文档"),
        "folder.paste" => ("Paste", "Вставить", "粘贴"),
        "folder.open_terminal_here" => (
            "Open Terminal Here",
            "Открыть терминал здесь",
            "在此处打开终端",
        ),
        // --- Desktop context menu (shared wording with folder menu) ---
        "desktop.arrange" => ("Arrange", "Упорядочить", "整理"),
        "desktop.change_wallpaper" => ("Change Wallpaper…", "Изменить обои…", "更改墙纸…"),
        // --- Search / Apps windows ---
        "panel.search" => ("Search", "Поиск", "搜索"),
        "panel.apps" => ("Apps", "Программы", "应用"),
        // --- Notifications / Do Not Disturb / lock screen ---
        "notify.notifications" => ("Notifications", "Уведомления", "通知"),
        "notify.do_not_disturb" => ("Do Not Disturb", "Не беспокоить", "勿扰模式"),
        "notify.settings" => (
            "Notification Settings…",
            "Настройки уведомлений…",
            "通知设置…",
        ),
        "notify.clear" => ("Clear", "Очистить", "清除"),
        "lock.enter_password" => ("Enter password", "Введите пароль", "输入密码"),
        // --- Generic file counts (folder status bar uses these words) ---
        // Note: the status bar still formats counts in code; these are the
        // nouns it should use. Full plural rules are a TODO (see docs/i18n.md).
        "status.folders" => ("folders", "папки", "个文件夹"),
        "status.files" => ("files", "файла", "个文件"),
        "status.free" => ("Free", "Свободно", "可用"),
        _ => return key,
    };
    match lang {
        Language::En => en,
        Language::Ru => ru,
        Language::Zh => zh,
        Language::Auto => en, // unreachable: resolve() already handled Auto
    }
}

/// All keys in [`t`] (used by tests: every key must have all 3 languages).
pub const KEYS: &[&str] = &[
    "app.terminal",
    "app.file_manager",
    "app.xfce_terminal",
    "app.text_editor",
    "app.settings",
    "app.task_manager",
    "app.home",
    "menu.shell",
    "menu.edit",
    "menu.view",
    "menu.window",
    "term.new_window",
    "term.new_tab",
    "term.close_tab",
    "term.close_window",
    "term.copy",
    "term.paste",
    "term.select_all",
    "term.clear_history",
    "term.zoom_in",
    "term.zoom_out",
    "term.zoom_reset",
    "term.next_tab",
    "term.prev_tab",
    "dock.trash",
    "dock.open",
    "dock.new_window",
    "dock.keep_in_dock",
    "dock.quit",
    "dock.empty_trash",
    "dock.run_on_dgpu",
    "folder.places",
    "folder.devices",
    "folder.filesystem",
    "folder.open",
    "folder.open_in_new_window",
    "folder.rename",
    "folder.copy",
    "folder.cut",
    "folder.move_to_trash",
    "folder.new_folder",
    "folder.new_text_file",
    "folder.paste",
    "folder.open_terminal_here",
    "desktop.arrange",
    "desktop.change_wallpaper",
    "panel.search",
    "panel.apps",
    "notify.notifications",
    "notify.do_not_disturb",
    "notify.settings",
    "notify.clear",
    "lock.enter_password",
    "status.folders",
    "status.files",
    "status.free",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_resolve() {
        assert_eq!(Language::parse("en"), Some(Language::En));
        assert_eq!(Language::parse("ru"), Some(Language::Ru));
        assert_eq!(Language::parse("zh"), Some(Language::Zh));
        assert_eq!(Language::parse("auto"), Some(Language::Auto));
        assert_eq!(Language::parse("fr"), None);
        assert_eq!(Language::En.name(), "en");
        assert_eq!(Language::Auto.name(), "auto");
        // Explicit values never fall back.
        assert_eq!(Language::Ru.resolve(), Language::Ru);
        assert_eq!(Language::Zh.resolve(), Language::Zh);
    }

    #[test]
    fn every_key_has_all_languages() {
        for key in KEYS {
            for lang in [Language::En, Language::Ru, Language::Zh] {
                let s = t(lang, key);
                assert!(!s.is_empty(), "{key} is empty for {lang:?}");
                assert_ne!(s, *key, "{key} fell back to the key for {lang:?}");
            }
        }
    }

    #[test]
    fn macos_style_spot_checks() {
        assert_eq!(t(Language::En, "dock.trash"), "Trash");
        assert_eq!(t(Language::Zh, "dock.trash"), "废纸篓");
        assert_eq!(t(Language::Ru, "dock.trash"), "Корзина");
        assert_eq!(t(Language::En, "app.file_manager"), "File Manager");
        assert_eq!(t(Language::Zh, "app.file_manager"), "文件管理器");
        assert_eq!(t(Language::En, "menu.edit"), "Edit");
        assert_eq!(t(Language::Zh, "menu.edit"), "编辑");
        // Unknown keys are visible, not blank.
        assert_eq!(t(Language::En, "not.a.key"), "not.a.key");
    }
}
