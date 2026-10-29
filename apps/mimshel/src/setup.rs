//! Команды настройки системы: `mimshel doctor [--fix]`, `mimshel set`,
//! `mimshel apply`, `mimshel config`. Сама работа — в `crates/setup`.

use mimshel_core::config::{self, PARTS, SETTINGS, set_value, setting_value};
use mimshel_setup::doctor::{Check, checks, fix_parts};
use mimshel_setup::parts::{APPLY_PARTS, apply};

use crate::ui::{config_path, config_store, load_config};

/// `mimshel doctor`. Код выхода: 0 — всё в порядке, 1 — есть проблемы.
pub fn doctor(fix: bool) -> i32 {
    let config = load_config();
    let path = config_path();
    let found = checks(&config, path.as_deref());
    for c in &found {
        print_check(c);
    }
    let Some(parts) = fix_parts(&found) else {
        if found.iter().all(|c| c.ok) {
            println!("✔ Проблем не найдено");
            return 0;
        }
        if found.iter().any(|c| !c.ok && c.part == "env") {
            println!("✘ Нет графического сеанса — чинить нечего, пока вы не вошли в систему");
        } else {
            println!(
                "⚠ Ошибку в файле настроек исправьте сами — служба пока работает по последним рабочим настройкам"
            );
        }
        return 1;
    };
    if !fix {
        println!("⚠ Чтобы починить: mimshel doctor --fix");
        return 1;
    }
    // Пакеты ставит установщик (нужен пароль sudo) — его не запускаем сами.
    if parts.contains(&"install") {
        println!("⚠ Не хватает пакетов или оформления — запустите ./install.sh в папке проекта");
    }
    let mine: Vec<&str> = parts
        .iter()
        .filter_map(|p| match *p {
            "install" => None,
            "menu" => Some("launchers"),
            p => APPLY_PARTS.iter().copied().find(|a| *a == p),
        })
        .collect();
    if !mine.is_empty() {
        run_apply(&config, &mine);
    }
    // Службе и панели нужно время, чтобы подняться.
    std::thread::sleep(std::time::Duration::from_secs(3));
    let left: Vec<Check> = checks(&config, path.as_deref())
        .into_iter()
        .filter(|c| !c.ok)
        .collect();
    for c in &left {
        print!("всё ещё не так: ");
        print_check(c);
    }
    if left.is_empty() {
        println!("✔ Всё починено");
        0
    } else {
        1
    }
}

fn print_check(c: &Check) {
    let mark = if c.ok { "✔" } else { "✘" };
    let hint = if c.ok || c.hint.is_empty() {
        String::new()
    } else {
        format!("  → {}", c.hint)
    };
    println!("{mark} [{}] {}{hint}", c.part, c.what);
}

/// `mimshel set раздел.ключ значение`: записать в файл настроек и применить.
/// `apply` = false (`mimshel set --no-apply`) — только записать: установщик
/// пишет несколько настроек и применяет всё разом.
pub fn set(key: &str, input: &str, apply: bool) -> i32 {
    let key = match write_setting(key, input) {
        Ok(key) => key,
        Err(SetError::Value(key, e)) => {
            println!("✘ {key}: {e}");
            if !SETTINGS.iter().any(|s| s.0 == key) {
                print_settings();
            }
            return 2;
        }
        Err(SetError::Write(e)) => {
            println!("✘ {e}");
            return 1;
        }
    };
    println!("✔ {key} = {}", input.trim());
    if !apply {
        return 0;
    }
    apply_setting(&key)
}

/// Why a setting was not written.
pub enum SetError {
    /// The value does not fit the key (or there is no such key).
    Value(String, String),
    /// The config file could not be written.
    Write(String),
}

/// Write `key = input` into the config file, as `mimshel set --no-apply`
/// does; the key as it is called now (`language` → `look.language`).
pub fn write_setting(key: &str, input: &str) -> Result<String, SetError> {
    // Bare `mimshel set language en` is an alias for `look.language`.
    let key = if key == "language" {
        "look.language".to_string()
    } else {
        config::current_key(key).to_string()
    };
    let value = setting_value(&key, input).map_err(|e| SetError::Value(key.clone(), e))?;
    // Создаст файл, если его ещё нет.
    load_config();
    let Some(path) = config_path() else {
        return Err(SetError::Write("Не найти папку настроек (нет HOME)".into()));
    };
    let (section, name) = key.split_once('.').expect("ключи из SETTINGS с точкой");
    config_store(path.clone())
        .edit(|text| {
            let text = set_value(text, section, name, value)
                .ok_or_else(|| std::io::Error::other("не удалось изменить настройки"))?;
            if key == "parts.folders" {
                let shell = config::Config::from_toml(&text).folders;
                return swap_file_manager(&text, shell)
                    .ok_or_else(|| std::io::Error::other("не удалось изменить настройки"));
            }
            Ok(text)
        })
        .map_err(|e| SetError::Write(format!("Не записать {}: {e}", path.display())))?;
    Ok(key)
}

/// Whether applying `key` restarts the service (everything but the hot
/// reload of the language and the wallpaper, and the terminal's settings).
pub fn restarts_service(key: &str) -> bool {
    !matches!(key, "look.language" | "look.wallpaper") && !key.starts_with("terminal.")
}

/// Apply a setting already written by [`write_setting`]; the exit code
/// of `mimshel set`.
pub fn apply_setting(key: &str) -> i32 {
    match apply_setting_checked(key) {
        Ok(()) => 0,
        Err(e) => {
            println!("✘ {e}");
            1
        }
    }
}

/// The same, with what went wrong (the settings window shows it).
pub fn apply_setting_checked(key: &str) -> Result<(), String> {
    let (section, name) = key.split_once('.').unwrap_or((key, ""));
    // Hot reload (v2): language and wallpaper switch without restarting
    // the daemon, the session or the machine. If no daemon is running,
    // the setting simply takes effect on the next login. The wallpaper
    // also goes to xfdesktop and the lock screen first, as `apply
    // wallpaper` does.
    if section == "look" && (name == "language" || name == "wallpaper") {
        if name == "wallpaper" {
            mimshel_setup::parts::apply_wallpaper(&load_config());
        }
        if crate::ui::ask_daemon_reload() {
            println!("Служба перечитала настройки (без перезапуска)");
        } else {
            println!("Служба не запущена — настройки применятся при следующем запуске");
        }
        return Ok(());
    }
    let config = load_config();
    // Что применить: часть, которую переключили, или прежнюю программу, чьи
    // настройки поменялись (Plank, панель Xfce). Служба перезапускается всегда.
    let parts: Vec<&str> = match section {
        "parts" => {
            vec![if name == "search_panel" || name == "apps_panel" {
                "launchers"
            } else {
                name
            }]
        }
        "dock" if !config.dock => vec!["dock"],
        // Тема значков — для всех программ (xsettings), не только для Mimshel.
        "look" if name == "icon_theme" || name == "png_icons" => vec!["icons"],
        "look" if name == "gtk_theme" || name == "cursor_theme" => vec!["look"],
        // Строка меню: и панель Xfce, и знак меню ставит apply menubar.
        "menubar" => vec!["menubar"],
        // The terminal is a separate process: it reads the setting when a
        // window opens, so there is nothing to restart.
        "terminal" => {
            println!(
                "Новые окна терминала возьмут настройку сами — уже открытые нужно открыть заново."
            );
            return Ok(());
        }
        _ => vec!["service"],
    };
    try_apply(&config, &parts)?;
    println!("✔ Применено");
    Ok(())
}

/// Свой Проводник включили (`mimshel`) или вернули Thunar: значок в доке —
/// того, кто открывает папки, а Thunar при своём Проводнике не показывается
/// в окнах «Программы» и «Поиск» (иначе там два «Проводника»).
fn swap_file_manager(text: &str, shell: bool) -> Option<String> {
    let (from, to) = if shell {
        ("thunar", crate::folder_ui::DESKTOP_ID)
    } else {
        (crate::folder_ui::DESKTOP_ID, "thunar")
    };
    // Записи — как написаны в файле: чтение настроек их нормализует, а
    // менять можно только одну.
    let raw = |section: &str, key: &str| -> Vec<String> {
        let doc: toml_edit::DocumentMut = text.parse().ok().unwrap_or_default();
        doc.get(section)
            .and_then(|t| t.get(key))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut dock = raw("dock", "apps");
    if dock.iter().any(|a| config::dock_entry(a) == to) {
        dock.retain(|a| config::dock_entry(a) != from);
    } else {
        for app in &mut dock {
            if config::dock_entry(app) == from {
                *app = to.to_string();
            }
        }
    }
    let text = config::set_dock_apps(text, &dock)?;
    let mut hide: Vec<String> = raw("apps", "hide")
        .into_iter()
        .filter(|h| h != "thunar")
        .collect();
    if shell {
        hide.push("thunar".into());
    }
    let hide: toml_edit::Array = hide.iter().map(String::as_str).collect();
    set_value(&text, "apps", "hide", toml_edit::Value::Array(hide))
}

/// `mimshel apply [части]`. `greeter` (the login screen) is handled apart:
/// it asks for the sudo password and never restarts the service, so it is
/// not part of the plain apply-all — `mimshel apply greeter [--dry-run]`.
pub fn apply_cmd(args: &[String]) -> i32 {
    let mut dry = false;
    let mut greeter = false;
    let mut rest: Vec<String> = Vec::new();
    for a in args {
        match a.as_str() {
            "--dry-run" => dry = true,
            "greeter" => greeter = true,
            other => rest.push(other.to_string()),
        }
    }
    if greeter {
        let mut code = match mimshel_setup::greeter::apply(&load_config(), dry) {
            Ok(()) => 0,
            Err(e) => {
                println!("✘ {e}");
                1
            }
        };
        if code == 0 && !rest.is_empty() {
            code = apply_parts(&rest);
        }
        return code;
    }
    if dry {
        println!("✘ --dry-run умеет только: mimshel apply greeter --dry-run");
        return 2;
    }
    apply_parts(args)
}

/// Parts listed in `APPLY_PARTS` (the plain `mimshel apply [части…]`).
fn apply_parts(parts: &[String]) -> i32 {
    let mut list = Vec::new();
    for p in parts {
        let p = match p.as_str() {
            "search" | "apps" | "spotlight" | "launchpad" => "launchers",
            p => p,
        };
        let Some(&known) = APPLY_PARTS.iter().find(|&&a| a == p) else {
            println!("✘ Нет части {p}. Можно: {}", APPLY_PARTS.join(", "));
            return 2;
        };
        list.push(known);
    }
    run_apply(&load_config(), &list)
}

fn run_apply(config: &mimshel_core::config::Config, parts: &[&str]) -> i32 {
    match try_apply(config, parts) {
        Ok(()) => {
            println!("✔ Применено");
            0
        }
        Err(e) => {
            println!("✘ {e}");
            1
        }
    }
}

/// `mimshel apply PARTS…`; what went wrong, for the settings window.
fn try_apply(config: &mimshel_core::config::Config, parts: &[&str]) -> Result<(), String> {
    let mut updated = config.clone();
    if (parts.is_empty() || parts.contains(&"launchers"))
        && let Some(path) = config_path()
    {
        let store = config_store(path);
        store
            .edit(|text| {
                swap_settings(text)
                    .ok_or_else(|| std::io::Error::other("не удалось изменить настройки"))
            })
            .map_err(|e| format!("Не записать настройки: {e}"))?;
        updated = store.load();
    }
    apply(&updated, parts)
}

/// Replace only an existing settings pin, preserving its position.
fn swap_settings(text: &str) -> Option<String> {
    let mut doc: toml_edit::DocumentMut = text.parse().ok()?;
    let Some(apps) = doc
        .get_mut("dock")
        .and_then(|t| t.get_mut("apps"))
        .and_then(|v| v.as_array_mut())
    else {
        return Some(text.to_string());
    };
    let mut seen = false;
    let mut remove = Vec::new();
    for (index, value) in apps.iter_mut().enumerate() {
        let Some(app) = value.as_str() else {
            continue;
        };
        if matches!(
            config::dock_entry(app),
            "xfce-settings-manager" | "mimshel-settings"
        ) {
            if seen {
                remove.push(index);
            } else {
                seen = true;
                if config::dock_entry(app) == "xfce-settings-manager" {
                    *value = toml_edit::Value::from("mimshel-settings");
                }
            }
        }
    }
    for index in remove.into_iter().rev() {
        apps.remove(index);
    }
    Some(doc.to_string())
}

/// `mimshel config`: где файл, что в нём и что можно менять.
pub fn show_config() -> i32 {
    let config = load_config();
    let Some(path) = config_path() else {
        return 1;
    };
    println!("Файл настроек: {}\n", path.display());
    for (name, old, what) in PARTS {
        let who = if config.part(name) == Some(true) {
            "shell"
        } else {
            old
        };
        println!("  {what}: {who}");
    }
    println!();
    print_settings();
    0
}

fn print_settings() {
    println!("Настройки (mimshel set КЛЮЧ ЗНАЧЕНИЕ):");
    for (key, what) in SETTINGS {
        println!("  {key:<22} {what}");
    }
}

/// `mimshel backup [ПОМЕТКА]`.
pub fn backup(label: Option<&str>) -> i32 {
    match mimshel_setup::backup::make(label.unwrap_or("manual")) {
        Ok(dest) => {
            println!("✔ Копия сохранена: {}", dest.display());
            0
        }
        Err(e) => {
            println!("✘ Не удалось сохранить копию: {e}");
            1
        }
    }
}

/// `mimshel backups`.
pub fn backups() -> i32 {
    let all = mimshel_setup::backup::list();
    if all.is_empty() {
        println!("Резервных копий нет");
    }
    for b in all {
        println!("  {}", b.file_name().unwrap_or_default().to_string_lossy());
    }
    0
}

/// `mimshel restore [--dry-run] [ИМЯ]` — по умолчанию самая первая копия.
pub fn restore(args: &[String]) -> i32 {
    let dry = args.iter().any(|a| a == "--dry-run");
    let name = args.iter().find(|a| *a != "--dry-run").map(String::as_str);
    let Some(src) = mimshel_setup::backup::find(name) else {
        println!("✘ Нет такой копии. Список: mimshel backups");
        return 1;
    };
    if dry {
        println!("Откат к {} (ничего не меняется):", src.display());
    }
    match mimshel_setup::backup::restore(&src, dry) {
        Ok(()) if dry => 0,
        Ok(()) => {
            println!("✔ Готово. Если что-то выглядит странно — выйдите из системы и зайдите снова");
            0
        }
        Err(e) => {
            println!("✘ Откат прерван: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_swap_preserves_position_and_is_idempotent() {
        let text = "[dock]\napps = [\"firefox\", \"xfce-settings-manager.desktop\", \"thunar\", \"mimshel-settings\"]\n";
        let once = swap_settings(text).unwrap();
        assert_eq!(
            config::Config::from_toml(&once).dock_apps,
            ["firefox", "mimshel-settings", "thunar"]
        );
        assert_eq!(swap_settings(&once).unwrap(), once);
        let absent = "[dock]\napps = [\"firefox\"]\n";
        assert_eq!(swap_settings(absent).unwrap(), absent);
        assert_eq!(swap_settings("# empty\n").unwrap(), "# empty\n");
    }

    #[test]
    fn file_manager_swap_keeps_other_entries() {
        let text =
            "[dock]\napps = [\"thunar\", \"org.telegram.desktop\"]\n\n[apps]\nhide = [\"rofi\"]\n";
        let on = swap_file_manager(text, true).unwrap();
        assert!(
            on.contains("[\"mimshel-folder\", \"org.telegram.desktop\"]"),
            "{on}"
        );
        assert!(on.contains("[\"rofi\", \"thunar\"]"), "{on}");
        let off = swap_file_manager(&on, false).unwrap();
        assert!(
            off.contains("[\"thunar\", \"org.telegram.desktop\"]"),
            "{off}"
        );
        assert!(off.contains("hide = [\"rofi\"]"), "{off}");
    }
}
