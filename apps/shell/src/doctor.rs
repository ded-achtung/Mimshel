//! `shell doctor [--fix]`: напечатать проверки и, если просят, починить
//! найденное. Чинит пока macos-look (`apply` нужных частей).

use std::process::Command;

use shell_setup::doctor::{Check, checks, fix_parts};
use shell_setup::look::Look;

/// Код выхода: 0 — всё в порядке, 1 — есть проблемы.
pub fn run(fix: bool) -> i32 {
    let look = Look::load();
    let config = crate::ui::config_path();
    let found = checks(&look, config.as_deref());
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
    let macos_look = shell_setup::look::Paths::new().tool;
    let mut steps: Vec<Vec<&str>> = Vec::new();
    if parts.contains(&"install") {
        // Сначала пакеты и темы, потом заново всё оформление.
        steps.push(vec!["install"]);
        steps.push(vec!["apply"]);
    } else {
        let mut apply = vec!["apply"];
        apply.extend(&parts);
        steps.push(apply);
    }
    for args in steps {
        println!("▶ macos-look {}", args.join(" "));
        match Command::new(&macos_look).args(&args).status() {
            Ok(s) if s.success() => {}
            Ok(s) => println!("✘ macos-look {} завершился с ошибкой ({s})", args.join(" ")),
            Err(e) => {
                println!("✘ Не удалось запустить {}: {e}", macos_look.display());
                return 1;
            }
        }
    }
    // Службе и панели нужно время, чтобы подняться.
    std::thread::sleep(std::time::Duration::from_secs(3));
    let left: Vec<Check> = checks(&look, config.as_deref())
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
