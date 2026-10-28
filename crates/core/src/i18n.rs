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

/// Language from the on-disk config file (`[look] language = ...`),
/// falling back to English. Used by processes that run outside the
/// daemon (guard, `mimshel action`, sound watcher) and therefore do not
/// already have a resolved language in hand.
pub fn language_from_config() -> Language {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = format!("{home}/.config/mimshel/config.toml");
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("language") {
                let v = v.trim().trim_start_matches('=').trim().trim_matches('"');
                if let Some(lang) = Language::parse(v) {
                    return lang;
                }
            }
        }
    }
    Language::En
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
        // --- App menu (top-bar app name menu) ---
        "menu.minimize" => ("Minimize", "Свернуть", "最小化", "最小化"),
        // --- Control center ---
        "control.network" => ("Network", "Сеть", "网络", "網路"),
        "control.on" => ("On", "Включён", "已开启", "已開啟"),
        "control.off" => ("Off", "Выключен", "已关闭", "已關閉"),
        "control.not_connected" => ("Not connected", "Не подключено", "未连接", "未連線"),
        "control.unavailable" => ("Unavailable", "Недоступен", "不可用", "無法使用"),
        "control.dnd1" => ("Do Not", "Не", "请勿", "請勿"),
        "control.dnd2" => ("Disturb", "беспокоить", "打扰", "打擾"),
        "control.display" => ("Display", "Дисплей", "显示器", "顯示器"),
        "control.sound" => ("Sound", "Звук", "声音", "聲音"),
        // --- Network panel ---
        "net.network" => ("Network", "Сеть", "网络", "網路"),
        "net.wired" => ("Wired network", "Проводная сеть", "有线网络", "有線網路"),
        "net.wifi_networks" => ("Wi-Fi networks", "Сети Wi-Fi", "Wi-Fi 网络", "Wi-Fi 網路"),
        "net.searching" => (
            "Searching for networks…",
            "Поиск сетей…",
            "正在搜索网络…",
            "正在搜尋網路…",
        ),
        "net.no_networks" => (
            "No networks nearby",
            "Сетей рядом не найдено",
            "附近没有网络",
            "附近沒有網路",
        ),
        "net.wifi_off" => (
            "Wi-Fi is off",
            "Wi-Fi выключен",
            "Wi-Fi 已关闭",
            "Wi-Fi 已關閉",
        ),
        "net.no_devices" => (
            "No network devices",
            "Нет сетевых устройств",
            "没有网络设备",
            "沒有網路裝置",
        ),
        "net.cable_unplugged" => (
            "Cable unplugged",
            "Кабель не подключён",
            "网线未连接",
            "網路線未連接",
        ),
        "net.connected" => ("Connected", "Подключено", "已连接", "已連線"),
        "net.connecting" => ("Connecting…", "Подключение…", "正在连接…", "正在連線…"),
        "net.settings" => (
            "Network Settings…",
            "Настройки сети…",
            "网络设置…",
            "網路設定…",
        ),
        // --- Bluetooth panel ---
        "bt.my_devices" => ("My devices", "Мои устройства", "我的设备", "我的裝置"),
        "bt.other_devices" => ("Other devices", "Другие устройства", "其他设备", "其他裝置"),
        "bt.searching" => (
            "Searching for devices…",
            "Поиск устройств…",
            "正在搜索设备…",
            "正在搜尋裝置…",
        ),
        "bt.no_devices" => (
            "No devices nearby",
            "Устройств рядом не найдено",
            "附近没有设备",
            "附近沒有裝置",
        ),
        "bt.off" => (
            "Bluetooth is off",
            "Bluetooth выключен",
            "蓝牙已关闭",
            "藍牙已關閉",
        ),
        "bt.disconnecting" => (
            "Disconnecting…",
            "Отключение…",
            "正在断开…",
            "正在中斷連線…",
        ),
        "bt.connecting" => ("Connecting…", "Подключение…", "正在连接…", "正在連線…"),
        "bt.pairing" => ("Pairing…", "Сопряжение…", "正在配对…", "正在配對…"),
        "bt.connected" => ("Connected", "Подключено", "已连接", "已連線"),
        "bt.settings" => (
            "Bluetooth Settings…",
            "Настройки Bluetooth…",
            "蓝牙设置…",
            "藍牙設定…",
        ),
        // --- Sound panel ---
        "sound.title" => ("Sound", "Звук", "声音", "聲音"),
        "sound.off" => ("Off", "выкл.", "关", "關"),
        "sound.apps" => ("Apps", "Программы", "应用", "應用程式"),
        "sound.output" => ("Output", "Выход", "输出", "輸出"),
        "sound.settings" => (
            "Sound Settings…",
            "Настройки звука…",
            "声音设置…",
            "聲音設定…",
        ),
        // --- Battery menu ---
        "battery.title" => ("Battery", "Аккумулятор", "电池", "電池"),
        "battery.fully_charged" => ("Fully charged", "Полностью заряжен", "已充满", "已充滿"),
        "battery.charging_full" => (
            "Charging, full in {t}",
            "Заряжается, до полного {t}",
            "正在充电，{t}后充满",
            "正在充電，{t}後充滿",
        ),
        "battery.charging" => ("Charging", "Заряжается", "正在充电", "正在充電"),
        "battery.on_battery_left" => (
            "On battery, {t} left",
            "От аккумулятора, осталось {t}",
            "使用电池，还剩{t}",
            "使用電池，還剩{t}",
        ),
        "battery.on_battery" => ("On battery", "От аккумулятора", "使用电池", "使用電池"),
        "battery.plugged_not_charging" => (
            "Plugged in, not charging",
            "От сети, не заряжается",
            "已接电源，未充电",
            "已接電源，未充電",
        ),
        "battery.empty" => ("Empty", "Разряжен", "电量耗尽", "電量耗盡"),
        "battery.unknown" => (
            "Status unknown",
            "Состояние неизвестно",
            "状态未知",
            "狀態未知",
        ),
        "battery.time_hour" => ("{n} h", "{n} ч", "{n}小时", "{n}小時"),
        "battery.time_min" => ("{n} min", "{n} мин", "{n}分钟", "{n}分鐘"),
        "battery.less_than_minute" => (
            "less than a minute",
            "меньше минуты",
            "不到一分钟",
            "不到一分鐘",
        ),
        "battery.power_settings" => (
            "Power Settings…",
            "Настройки энергосбережения…",
            "电源设置…",
            "電源設定…",
        ),
        // --- Search / Apps panels ---
        "panel.nothing_found" => (
            "Nothing found",
            "Ничего не найдено",
            "未找到",
            "找不到任何項目",
        ),
        // --- Wallpaper picker ---
        "wallpaper.title" => ("Wallpaper", "Обои", "壁纸", "桌布"),
        "wallpaper.custom" => ("Custom image", "Своя картинка", "自定义图片", "自訂圖片"),
        "wallpaper.pick" => (
            "Choose image…",
            "Выбрать картинку…",
            "选择图片…",
            "選擇圖片…",
        ),
        "wallpaper.space" => ("Space", "Космос", "太空", "太空"),
        "wallpaper.waves" => ("Waves", "Волны", "波浪", "波浪"),
        "wallpaper.aurora" => ("Aurora", "Сияние", "极光", "極光"),
        // --- About window (labels come from setup in Russian; translated at render) ---
        "about.computer" => ("Computer", "Компьютер", "电脑", "電腦"),
        "about.processor" => ("Processor", "Процессор", "处理器", "處理器"),
        "about.memory" => ("Memory", "Память", "内存", "記憶體"),
        "about.graphics" => ("Graphics", "Графика", "显卡", "顯示卡"),
        "about.disk" => ("Disk", "Диск", "磁盘", "磁碟"),
        "about.shell" => ("Shell", "Оболочка", "桌面壳", "桌面殼"),
        "about.screen" => ("Display", "Экран", "屏幕", "螢幕"),
        "about.kernel" => ("Kernel", "Ядро", "内核", "核心"),
        // --- Apple / start menu (top-left logo menu) ---
        "apple.about" => (
            "About This Computer",
            "Об этом компьютере",
            "关于本机",
            "關於這台電腦",
        ),
        "apple.settings" => (
            "System Settings…",
            "Системные настройки…",
            "系统设置…",
            "系統設定…",
        ),
        "apple.forcequit" => (
            "Force Quit…",
            "Принудительно завершить…",
            "强制退出…",
            "強制結束…",
        ),
        "apple.sleep" => ("Sleep", "Режим сна", "睡眠", "睡眠"),
        "apple.restart" => ("Restart…", "Перезагрузить…", "重新启动…", "重新啟動…"),
        "apple.shutdown" => ("Shut Down…", "Выключить…", "关机…", "關機…"),
        "apple.lock" => ("Lock Screen", "Заблокировать экран", "锁定屏幕", "鎖定螢幕"),
        "apple.logout" => ("Log Out…", "Завершить сеанс…", "退出登录…", "登出…"),
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
        "lock.wrong_password" => ("Wrong password", "Неверный пароль", "密码错误", "密碼錯誤"),
        "lock.too_many_tries" => (
            "Too many attempts — wait and try again",
            "Слишком много попыток — подождите и попробуйте снова",
            "尝试次数过多，请稍后再试",
            "嘗試次數過多，請稍後再試",
        ),
        "lock.password_expired" => (
            "The password has expired — change it in a text console (Ctrl+Alt+F3)",
            "Срок действия пароля истёк — смените его в текстовой консоли (Ctrl+Alt+F3)",
            "密码已过期，请在文本控制台中修改（Ctrl+Alt+F3）",
            "密碼已過期，請在文字主控台中變更（Ctrl+Alt+F3）",
        ),
        "lock.account_expired" => (
            "The account has expired",
            "Срок действия учётной записи истёк",
            "账户已过期",
            "帳號已過期",
        ),
        "lock.check_failed" => (
            "The password could not be checked",
            "Не удалось проверить пароль",
            "无法验证密码",
            "無法驗證密碼",
        ),
        // --- v5: previously hardcoded Russian GUI strings ---
        "notify.crash_title" => (
            "Mimshel restarted after a crash",
            "Mimshel перезапустился после сбоя",
            "Mimshel 崩溃后已重启",
            "Mimshel 當機後已重新啟動",
        ),
        "notify.crash_details" => (
            "Details: mimshel crashes",
            "Подробности: mimshel crashes",
            "详情：mimshel crashes",
            "詳情：mimshel crashes",
        ),
        "crash.give_up_head" => (
            "Mimshel stopped — the service crashed 5 times in a row.",
            "Mimshel остановлен — служба падала 5 раз подряд.",
            "Mimshel 已停止——服务连续崩溃 5 次。",
            "Mimshel 已停止——服務連續當機 5 次。",
        ),
        "crash.give_up_reason" => (
            "Last reason:",
            "Последняя причина:",
            "最近原因：",
            "最近原因：",
        ),
        "crash.give_up_restore" => (
            "Restore the Xfce panel: mimshel restore",
            "Вернуть панель Xfce: mimshel restore",
            "恢复 Xfce 面板：mimshel restore",
            "還原 Xfce 面板：mimshel restore",
        ),
        "dock.remove" => ("Remove", "Удалить", "移除", "移除"),
        "folder.search_placeholder" => (
            "Search in folder",
            "Поиск в папке",
            "在文件夹中搜索",
            "在資料夾中搜尋",
        ),
        "menubar.sound_unnamed" => ("Unnamed", "Без названия", "未命名", "未命名"),
        "action.about_title" => (
            "About This Computer",
            "Об этом компьютере",
            "关于这台电脑",
            "關於這台電腦",
        ),
        "action.restart_title" => ("Restart", "Перезагрузить", "重启", "重新啟動"),
        "action.restart_text" => (
            "Do you really want to restart your computer?",
            "Вы действительно хотите перезагрузить компьютер?",
            "确实要重启计算机吗？",
            "確定要重新啟動電腦嗎？",
        ),
        "action.shutdown_title" => ("Shut Down", "Выключить", "关机", "關機"),
        "action.shutdown_text" => (
            "Do you really want to shut down your computer?",
            "Вы действительно хотите выключить компьютер?",
            "确实要关闭计算机吗？",
            "確定要關閉電腦嗎？",
        ),
        "action.logout_title" => ("Log Out", "Завершить сеанс", "退出登录", "登出"),
        "action.logout_text" => (
            "Close all programs and log out?",
            "Закрыть все программы и выйти из системы?",
            "关闭所有程序并退出登录吗？",
            "關閉所有程式並登出嗎？",
        ),
        "action.cancel" => ("Cancel", "Отменить", "取消", "取消"),
        // --- v6: desktop / wallpaper zenity + file-op errors missed by v5 ---
        "desktop.new_text_file_name" => (
            "New Document.txt",
            "Новый документ.txt",
            "新建文档.txt",
            "新增文件.txt",
        ),
        "desktop.launch_title" => (
            "Launch shortcut?",
            "Запустить ярлык?",
            "启动快捷方式？",
            "啟動捷徑？",
        ),
        "desktop.launch_ok" => ("Launch", "Запустить", "启动", "啟動"),
        "desktop.launch_text" => (
            "«{name}» is a program launcher, not a document.\nIt will run the command:\n\n<tt>{exec}</tt>\n\nOnly run it if you trust where it came from.",
            "«{name}» — ярлык программы, а не документ.\nОн выполнит команду:\n\n<tt>{exec}</tt>\n\nЗапускайте, только если доверяете тому, откуда он взялся.",
            "«{name}»是程序快捷方式，不是文档。\n它将执行命令：\n\n<tt>{exec}</tt>\n\n只有在信任其来源时才运行。",
            "«{name}»是程式捷徑，不是文件。\n它將執行指令：\n\n<tt>{exec}</tt>\n\n只有在信任其來源時才執行。",
        ),
        "desktop.error_title" => ("Desktop", "Рабочий стол", "桌面", "桌面"),
        "desktop.launcher_not_executable" => (
            "Launcher not marked executable: {e}",
            "Ярлык не помечен выполняемым: {e}",
            "快捷方式未标记为可执行：{e}",
            "捷徑未標記為可執行：{e}",
        ),
        "desktop.rename_invalid" => (
            "Invalid file name.",
            "Недопустимое имя файла.",
            "文件名无效。",
            "檔名無效。",
        ),
        "desktop.rename_no_dir" => ("No directory", "нет папки", "没有文件夹", "沒有資料夾"),
        "desktop.rename_exists" => (
            "«{new}» already exists in this folder.",
            "«{new}» уже есть в этой папке.",
            "“{new}”已存在于此文件夹中。",
            "「{new}」已存在於此資料夾中。",
        ),
        "desktop.rename_check_failed" => (
            "Could not check destination name: {e}",
            "Не проверить имя назначения: {e}",
            "无法检查目标名称：{e}",
            "無法檢查目標名稱：{e}",
        ),
        "desktop.rename_failed" => (
            "Could not rename: {e}",
            "Не удалось переименовать: {e}",
            "重命名失败：{e}",
            "重新命名失敗：{e}",
        ),
        "desktop.rename_not_done" => (
            "Rename not completed: name is taken or file is unavailable.",
            "Переименование не выполнено: имя занято или файл недоступен.",
            "重命名未完成：名称已被占用或文件不可用。",
            "重新命名未完成：名稱已被佔用或檔案無法使用。",
        ),
        "desktop.transfer_exists" => (
            "Destination file {dest} already exists.",
            "Файл назначения {dest} уже существует.",
            "目标文件 {dest} 已存在。",
            "目標檔案 {dest} 已存在。",
        ),
        "desktop.transfer_check_failed" => (
            "Could not check {dest}: {e}",
            "Не проверить {dest}: {e}",
            "无法检查 {dest}：{e}",
            "無法檢查 {dest}：{e}",
        ),
        "desktop.transfer_read_failed" => (
            "Could not read {src}: {e}",
            "Не прочитать {src}: {e}",
            "无法读取 {src}：{e}",
            "無法讀取 {src}：{e}",
        ),
        "desktop.transfer_failed" => (
            "Could not move «{name}»: destination is taken or unavailable.",
            "Не удалось перенести «{name}»: файл назначения занят или недоступен.",
            "无法移动“{name}”：目标已被占用或不可用。",
            "無法移動「{name}」：目標已被佔用或無法使用。",
        ),
        "wallpaper.pick_title" => (
            "Choose a picture for wallpaper",
            "Картинка для обоев",
            "为墙纸选择图片",
            "為桌布選擇圖片",
        ),
        "wallpaper.pick_filter" => ("Pictures", "Картинки", "图片", "圖片"),
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
    "lock.wrong_password",
    "lock.too_many_tries",
    "lock.password_expired",
    "lock.account_expired",
    "lock.check_failed",
    "notify.crash_title",
    "notify.crash_details",
    "crash.give_up_head",
    "crash.give_up_reason",
    "crash.give_up_restore",
    "dock.remove",
    "folder.search_placeholder",
    "menubar.sound_unnamed",
    "action.about_title",
    "action.restart_title",
    "action.restart_text",
    "action.shutdown_title",
    "action.shutdown_text",
    "action.logout_title",
    "action.logout_text",
    "action.cancel",
    "desktop.new_text_file_name",
    "desktop.launch_title",
    "desktop.launch_ok",
    "desktop.launch_text",
    "desktop.error_title",
    "desktop.launcher_not_executable",
    "desktop.rename_invalid",
    "desktop.rename_no_dir",
    "desktop.rename_exists",
    "desktop.rename_check_failed",
    "desktop.rename_failed",
    "desktop.rename_not_done",
    "desktop.transfer_exists",
    "desktop.transfer_check_failed",
    "desktop.transfer_read_failed",
    "desktop.transfer_failed",
    "wallpaper.pick_title",
    "wallpaper.pick_filter",
    "status.folders",
    "status.files",
    "status.free",
    "apple.about",
    "apple.settings",
    "apple.forcequit",
    "apple.sleep",
    "apple.restart",
    "apple.shutdown",
    "apple.lock",
    "apple.logout",
    "menu.minimize",
    "control.network",
    "control.on",
    "control.off",
    "control.not_connected",
    "control.unavailable",
    "control.dnd1",
    "control.dnd2",
    "control.display",
    "control.sound",
    "net.network",
    "net.wired",
    "net.wifi_networks",
    "net.searching",
    "net.no_networks",
    "net.wifi_off",
    "net.no_devices",
    "net.cable_unplugged",
    "net.connected",
    "net.connecting",
    "net.settings",
    "bt.my_devices",
    "bt.other_devices",
    "bt.searching",
    "bt.no_devices",
    "bt.off",
    "bt.disconnecting",
    "bt.connecting",
    "bt.pairing",
    "bt.connected",
    "bt.settings",
    "sound.title",
    "sound.off",
    "sound.apps",
    "sound.output",
    "sound.settings",
    "battery.title",
    "battery.fully_charged",
    "battery.charging_full",
    "battery.charging",
    "battery.on_battery_left",
    "battery.on_battery",
    "battery.plugged_not_charging",
    "battery.empty",
    "battery.unknown",
    "battery.time_hour",
    "battery.time_min",
    "battery.less_than_minute",
    "battery.power_settings",
    "panel.nothing_found",
    "wallpaper.title",
    "wallpaper.custom",
    "wallpaper.pick",
    "wallpaper.space",
    "wallpaper.waves",
    "wallpaper.aurora",
    "about.computer",
    "about.processor",
    "about.memory",
    "about.graphics",
    "about.disk",
    "about.shell",
    "about.screen",
    "about.kernel",
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
