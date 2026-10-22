//! Команды настройки системы: `shell doctor [--fix]`, `shell set`,
//! `shell apply`, `shell config`. Сама работа — в `crates/setup`; то, что
//! ещё не перенесено (тема, обои, блокировка, меню яблока), делает macos-look.

use std::process::Command;

use shell_core::config::{PARTS, SETTINGS, set_value, setting_value, toml_error};
use shell_setup::doctor::{Check, checks, fix_parts};
use shell_setup::look::{Look, Paths, set_macos_look_engine};
use shell_setup::parts::{APPLY_PARTS, apply};

use crate::ui::{config_path, load_config, write_file};

/// Части, которые чинит shell; остальные — macos-look.
const OWN: &[&str] = &[
    "menubar",
    "dock",
    "notify",
    "launchers",
    "lock",
    "power",
    "service",
];

/// `shell doctor`. Код выхода: 0 — всё в порядке, 1 — есть проблемы.
pub fn doctor(fix: bool) -> i32 {
    let look = Look::load();
    let config = load_config();
    let path = config_path();
    let found = checks(&look, &config, path.as_deref());
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
        println!("⚠ Чтобы починить: shell doctor --fix");
        return 1;
    }
    let theirs: Vec<&str> = parts.iter().copied().filter(|p| !OWN.contains(p)).collect();
    let mine: Vec<&str> = parts.iter().copied().filter(|p| OWN.contains(p)).collect();
    if !theirs.is_empty() {
        let mut steps: Vec<Vec<&str>> = Vec::new();
        if theirs.contains(&"install") {
            // Сначала пакеты и темы, потом заново всё оформление.
            steps.push(vec!["install"]);
            steps.push(vec!["apply"]);
        } else {
            let mut args = vec!["apply"];
            args.extend(&theirs);
            steps.push(args);
        }
        let macos_look = Paths::new().tool;
        for args in steps {
            println!("▶ macos-look {}", args.join(" "));
            match Command::new(&macos_look).args(&args).status() {
                Ok(s) if s.success() => {}
                Ok(s) => println!("✘ macos-look {} завершился с ошибкой ({s})", args.join(" ")),
                Err(e) => println!("✘ Не удалось запустить {}: {e}", macos_look.display()),
            }
        }
    }
    if !mine.is_empty()
        && let Err(e) = apply(&config, &look, &mine)
    {
        println!("✘ {e}");
    }
    // Службе и панели нужно время, чтобы подняться.
    std::thread::sleep(std::time::Duration::from_secs(3));
    let left: Vec<Check> = checks(&look, &config, path.as_deref())
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

/// `shell set раздел.ключ значение`: записать в файл настроек и применить.
pub fn set(key: &str, input: &str) -> i32 {
    let value = match setting_value(key, input) {
        Ok(v) => v,
        Err(e) => {
            println!("✘ {key}: {e}");
            if !SETTINGS.iter().any(|s| s.0 == key) {
                print_settings();
            }
            return 2;
        }
    };
    // Создаст файл, если его ещё нет.
    load_config();
    let Some(path) = config_path() else {
        println!("✘ Не найти папку настроек (нет HOME)");
        return 1;
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    if let Some(e) = toml_error(&text) {
        println!("✘ В {} ошибка — исправьте её сначала:\n{e}", path.display());
        return 1;
    }
    let (section, name) = key.split_once('.').expect("ключи из SETTINGS с точкой");
    let Some(text) = set_value(&text, section, name, value) else {
        return 1;
    };
    if let Err(e) = write_file(&path, &text) {
        println!("✘ Не записать {}: {e}", path.display());
        return 1;
    }
    println!("✔ {key} = {}", input.trim());
    let config = load_config();
    // Что применить: часть, которую переключили, или прежнюю программу, чьи
    // настройки поменялись (Plank, панель Xfce). Служба перезапускается всегда.
    let parts: Vec<&str> = match section {
        "parts" => {
            if let Err(e) = set_macos_look_engine(name, input.trim()) {
                println!("⚠ Не записать в конфиг macos-look: {e}");
            }
            vec![if name == "spotlight" || name == "launchpad" {
                "launchers"
            } else {
                name
            }]
        }
        "dock" if !config.dock => vec!["dock"],
        "menubar" if !config.menubar => vec!["menubar"],
        _ => vec!["service"],
    };
    run_apply(&config, &parts)
}

/// `shell apply [части]`.
pub fn apply_cmd(parts: &[String]) -> i32 {
    let mut list = Vec::new();
    for p in parts {
        let p = match p.as_str() {
            "spotlight" | "launchpad" => "launchers",
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

fn run_apply(config: &shell_core::config::Config, parts: &[&str]) -> i32 {
    match apply(config, &Look::load(), parts) {
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

/// `shell config`: где файл, что в нём и что можно менять.
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
    println!("Настройки (shell set КЛЮЧ ЗНАЧЕНИЕ):");
    for (key, what) in SETTINGS {
        println!("  {key:<22} {what}");
    }
}
