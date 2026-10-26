//! Minimal multilingual support for Mimshel (i18n-en-ru-zh).
//!
//! Design notes (deliberately small, no gettext):
//! * One static table: `key -> (en, ru, zh, zh_tw)` in [`t`]. Russian strings are
//!   the upstream originals, kept verbatim as `ru`, so a `ru` user sees
//!   exactly what upstream showed.
//! * [`Language`] is resolved, never guessed: the config default is
//!   `en` (v2 user decision); `auto` is an explicit opt-in that follows
//!   the system locale (see `from_env` for the zh_TW / zh_CN mapping).
//!   A locale that is not `en`/`ru`/`zh`/`zh_TW` falls back to `en`.
//!   An explicit value in config always wins over the locale.
//! * Callers pass a resolved [`Language`] in; there is no global mutable
//!   locale state, so the daemon can reload on `mimshel apply` simply by
//!   re-reading config.
//! * Only the most visible UI strings are in this table (see
//!   `docs/i18n.md` for the translated / not-yet-translated lists).
//!   Everything else still uses the upstream Russian literals — see
//!   `TODO-I18N` in that doc. Do not claim the whole UI is translated.
//!
//! Chinese has two columns: Simplified `zh` (zh-CN wording, e.g. 废纸篓,
//! 文件管理器, 搜索) and Traditional `zh_TW` (Taiwan wording, e.g. 垃圾桶,
//! 檔案管理員, 搜尋). Japanese is explicitly out of scope (user decision
//! 2026-10-27: Traditional yes, Japanese no).

use std::env;

/// Interface language. `Auto` means "follow the system locale" (explicit
/// opt-in only; the config default in v2 is `En`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    /// Follow the system locale (config default).
    #[default]
    Auto,
    En,
    Ru,
    Zh,
    ZhTw,
}

impl Language {
    /// Config-file word for this language (`auto` / `en` / `ru` / `zh` / `zh_TW`).
    pub fn name(self) -> &'static str {
        match self {
            Language::Auto => "auto",
            Language::En => "en",
            Language::Ru => "ru",
            Language::Zh => "zh",
            Language::ZhTw => "zh_TW",
        }
    }

    /// Parse a config word. Unknown words are `None` (caller keeps the
    /// previous value, which is `en` by default in v2).
    pub fn parse(s: &str) -> Option<Language> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Language::Auto),
            "en" | "eng" | "english" => Some(Language::En),
            "ru" | "rus" | "russian" => Some(Language::Ru),
            "zh" | "chi" | "chinese" | "simplified" | "zh-cn" | "zh_cn" | "zh-sg" | "zh_sg" => {
                Some(Language::Zh)
            }
            "zh-tw" | "zh_tw" | "zh-hk" | "zh_hk" | "traditional" => Some(Language::ZhTw),
            _ => None,
        }
    }

    /// Language from the system locale (`auto` mode only).
    ///
    /// Checks `LANGUAGE`, then `LANG`, then `LC_ALL`, then `LC_MESSAGES`
    /// (first entry of a `:`-separated list, lowercased, `_` kept).
    /// Mapping (user decision 2026-10-27):
    /// `zh_TW` / `zh_HK` -> `ZhTw`; `zh_CN` / `zh_SG` / bare `zh` -> `Zh`;
    /// `ru_*` -> `Ru`; `en_*` -> `En`; anything else (including C/POSIX)
    /// -> `En`.
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
                return match code.as_str() {
                    "zh_tw" | "zh_hk" => Language::ZhTw,
                    "zh_cn" | "zh_sg" | "zh" => Language::Zh,
                    _ => {
                        let lang = code.split('_').next().unwrap_or("");
                        match lang {
                            "ru" => Language::Ru,
                            "zh" => Language::Zh,
                            "en" => Language::En,
                            // C / POSIX / anything else: English fallback.
                            _ => Language::En,
                        }
                    }
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
    let (en, ru, zh, zh_tw) = match key {
        // --- App names (also used as Config rename defaults) ---
        "app.terminal" => ("Terminal", "Терминал", "终端", "終端機"),
        "app.file_manager" => ("File Manager", "Проводник", "文件管理器", "檔案管理員"),
        "app.xfce_terminal" => ("Xfce Terminal", "Терминал Xfce", "Xfce 终端", "Xfce 終端機"),
        "app.text_editor" => ("Text Editor", "Блокнот", "文本编辑器", "文字編輯器"),
        "app.settings" => ("Settings", "Системные настройки", "系统设置", "系統設定"),
        "app.task_manager" => (
            "System Monitor",
            "Мониторинг системы",
            "系统监视器",
            "系統監控",
        ),
        "app.home" => ("Home", "Домашняя папка", "主文件夹", "家目錄"),
        // --- Terminal window menus (macOS-style) ---
        "menu.shell" => ("Shell", "Оболочка", "Shell", "Shell"),
        "menu.edit" => ("Edit", "Правка", "编辑", "編輯"),
        "menu.view" => ("View", "Вид", "显示", "檢視"),
        "menu.window" => ("Window", "Окно", "窗口", "視窗"),
        "term.new_window" => ("New Window", "Новое окно", "新建窗口", "新增視窗"),
        "term.new_tab" => ("New Tab", "Новая вкладка", "新建标签页", "新增分頁"),
        "term.close_tab" => ("Close Tab", "Закрыть вкладку", "关闭标签页", "關閉分頁"),
        "term.close_window" => ("Close Window", "Закрыть окно", "关闭窗口", "關閉視窗"),
        "term.copy" => ("Copy", "Копировать", "复制", "複製"),
        "term.paste" => ("Paste", "Вставить", "粘贴", "貼上"),
        "term.select_all" => ("Select All", "Выделить всё", "全选", "全選"),
        "term.clear_history" => (
            "Clear History",
            "Очистить историю",
            "清除历史记录",
            "清除歷史紀錄",
        ),
        "term.zoom_in" => ("Zoom In", "Крупнее", "放大", "放大"),
        "term.zoom_out" => ("Zoom Out", "Мельче", "缩小", "縮小"),
        "term.zoom_reset" => ("Actual Size", "Обычный размер", "实际大小", "實際大小"),
        "term.next_tab" => (
            "Next Tab",
            "Следующая вкладка",
            "下一个标签页",
            "下一個分頁",
        ),
        "term.prev_tab" => (
            "Previous Tab",
            "Предыдущая вкладка",
            "上一个标签页",
            "上一個分頁",
        ),
        // --- Dock ---
        "dock.trash" => ("Trash", "Корзина", "废纸篓", "垃圾桶"),
        "dock.open" => ("Open", "Открыть", "打开", "開啟"),
        "dock.new_window" => ("New Window", "Новое окно", "新建窗口", "新增視窗"),
        "dock.keep_in_dock" => (
            "Keep in Dock",
            "Оставить в Dock",
            "保留在程序坞",
            "保留在 Dock",
        ),
        "dock.quit" => ("Quit", "Завершить", "退出", "結束"),
        "dock.empty_trash" => (
            "Empty Trash…",
            "Очистить Корзину…",
            "清倒废纸篓…",
            "清空垃圾桶…",
        ),
        "dock.run_on_dgpu" => (
            "Run on discrete GPU",
            "Запускать на видеокарте NVIDIA",
            "使用独立显卡运行",
            "使用獨立顯示卡執行",
        ),
        // --- Folder / file-manager windows ---
        "folder.places" => ("Places", "Места", "位置", "位置"),
        "folder.devices" => ("Devices", "Устройства", "设备", "裝置"),
        "folder.filesystem" => ("File System", "Файловая система", "文件系统", "檔案系統"),
        "folder.open" => ("Open", "Открыть", "打开", "開啟"),
        "folder.open_in_new_window" => (
            "Open in New Window",
            "Открыть в новом окне",
            "在新窗口中打开",
            "在新視窗中開啟",
        ),
        "folder.rename" => ("Rename", "Переименовать", "重命名", "重新命名"),
        "folder.copy" => ("Copy", "Копировать", "复制", "複製"),
        "folder.cut" => ("Cut", "Вырезать", "剪切", "剪下"),
        "folder.move_to_trash" => ("Move to Trash", "В корзину", "移到废纸篓", "移到垃圾桶"),
        "folder.new_folder" => ("New Folder", "Новая папка", "新建文件夹", "新增資料夾"),
        "folder.new_text_file" => (
            "New Text File",
            "Новый текстовый файл",
            "新建文本文档",
            "新增文字檔案",
        ),
        "folder.paste" => ("Paste", "Вставить", "粘贴", "貼上"),
        "folder.open_terminal_here" => (
            "Open Terminal Here",
            "Открыть терминал здесь",
            "在此处打开终端",
            "在此開啟終端機",
        ),
        // --- Desktop context menu (shared wording with folder menu) ---
        "desktop.arrange" => ("Arrange", "Упорядочить", "整理", "整理"),
        "desktop.change_wallpaper" => (
            "Change Wallpaper…",
            "Изменить обои…",
            "更改墙纸…",
            "更改桌布…",
        ),
        // --- Search / Apps windows ---
        "panel.search" => ("Search", "Поиск", "搜索", "搜尋"),
        "panel.apps" => ("Apps", "Программы", "应用", "應用程式"),
        // --- Notifications / Do Not Disturb / lock screen ---
        "notify.notifications" => ("Notifications", "Уведомления", "通知", "通知"),
        "notify.do_not_disturb" => ("Do Not Disturb", "Не беспокоить", "勿扰模式", "勿擾模式"),
        "notify.settings" => (
            "Notification Settings…",
            "Настройки уведомлений…",
            "通知设置…",
            "通知設定…",
        ),
        "notify.clear" => ("Clear", "Очистить", "清除", "清除"),
        "lock.enter_password" => ("Enter password", "Введите пароль", "输入密码", "輸入密碼"),
        // --- Generic file counts (folder status bar uses these words) ---
        // Note: the status bar still formats counts in code; these are the
        // nouns it should use. Full plural rules are a TODO (see docs/i18n.md).
        "status.folders" => ("folders", "папки", "个文件夹", "個資料夾"),
        "status.files" => ("files", "файла", "个文件", "個檔案"),
        "status.free" => ("Free", "Свободно", "可用", "可用"),
        _ => return key,
    };
    match lang {
        Language::En => en,
        Language::Ru => ru,
        Language::Zh => zh,
        Language::ZhTw => zh_tw,
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
        assert_eq!(Language::parse("zh_TW"), Some(Language::ZhTw));
        assert_eq!(Language::parse("zh-tw"), Some(Language::ZhTw));
        assert_eq!(Language::parse("zh_HK"), Some(Language::ZhTw));
        assert_eq!(Language::parse("zh_CN"), Some(Language::Zh));
        assert_eq!(Language::parse("auto"), Some(Language::Auto));
        assert_eq!(Language::parse("fr"), None);
        assert_eq!(Language::En.name(), "en");
        assert_eq!(Language::Auto.name(), "auto");
        // Explicit values never fall back.
        assert_eq!(Language::Ru.resolve(), Language::Ru);
        assert_eq!(Language::Zh.resolve(), Language::Zh);
        assert_eq!(Language::ZhTw.resolve(), Language::ZhTw);
    }

    #[test]
    fn every_key_has_all_languages() {
        for key in KEYS {
            for lang in [Language::En, Language::Ru, Language::Zh, Language::ZhTw] {
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
        assert_eq!(t(Language::ZhTw, "app.file_manager"), "檔案管理員");
        assert_eq!(t(Language::ZhTw, "dock.trash"), "垃圾桶");
        assert_eq!(t(Language::ZhTw, "menu.view"), "檢視");
        assert_eq!(t(Language::ZhTw, "panel.search"), "搜尋");
        assert_eq!(t(Language::En, "menu.edit"), "Edit");
        assert_eq!(t(Language::Zh, "menu.edit"), "编辑");
        // Unknown keys are visible, not blank.
        assert_eq!(t(Language::En, "not.a.key"), "not.a.key");
    }
}
