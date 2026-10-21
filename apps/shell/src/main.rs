//! Оболочка рабочего стола: Spotlight и проверочные команды.

mod spotlight;

use std::collections::HashMap;
use std::time::Instant;

use shell_core::search::{Query, best};

const HELP: &str = "\
Использование:
  shell apps            список программ, которые увидит Launchpad
  shell search ТЕКСТ    что найдёт Spotlight по этому тексту
  shell watch           следить за ярлыками и печатать, что появилось и пропало
  shell daemon          служба: держит Spotlight наготове
  shell spotlight       открыть или закрыть Spotlight (через службу, без неё — сам)
  shell snapshot ТЕКСТ ФАЙЛ.png
                        нарисовать Spotlight с этим текстом в картинку (без экрана)";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("apps") => list_apps(),
        Some("search") if args.len() > 1 => search(&args[1..].join(" ")),
        Some("watch") => watch(),
        Some("daemon") => exit_on_error(spotlight::run(true)),
        Some("spotlight") => {
            if !spotlight::ask_daemon() {
                exit_on_error(spotlight::run(false));
            }
        }
        Some("snapshot") if args.len() == 3 => {
            exit_on_error(spotlight::snapshot(&args[1], &args[2]))
        }
        _ => {
            println!("{HELP}");
            std::process::exit(2);
        }
    }
}

fn exit_on_error(result: shell_x11::Result<()>) {
    if let Err(e) = result {
        eprintln!("Ошибка: {e}");
        std::process::exit(1);
    }
}

fn load() -> Vec<shell_apps::App> {
    let start = Instant::now();
    let apps = shell_apps::load(&shell_apps::Settings::from_env());
    eprintln!(
        "Прочитано программ: {} за {:?}",
        apps.len(),
        start.elapsed()
    );
    apps
}

fn list_apps() {
    for app in load() {
        println!("{:<32} {:<40} {}", app.name, app.id, app.command.join(" "));
    }
}

fn search(text: &str) {
    let apps = load();
    let start = Instant::now();
    let query = Query::new(text);
    let found = best(&query, &apps, shell_launcher::fields);
    eprintln!("Поиск занял {:?}", start.elapsed());
    for app in found.iter().take(10) {
        println!("{:<32} {}", app.name, app.id);
    }
}

fn watch() {
    let settings = shell_apps::Settings::from_env();
    let mut watcher = match shell_apps::Watcher::new(&settings) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Не удалось следить за ярлыками: {e}");
            std::process::exit(1);
        }
    };
    let mut apps = load();
    eprintln!("Слежу за ярлыками. Остановить — Ctrl+C.");
    loop {
        if let Err(e) = watcher.wait() {
            eprintln!("Слежение прервалось: {e}");
            std::process::exit(1);
        }
        let new = load();
        let old_by_id: HashMap<&str, &shell_apps::App> =
            apps.iter().map(|a| (a.id.as_str(), a)).collect();
        let new_by_id: HashMap<&str, &shell_apps::App> =
            new.iter().map(|a| (a.id.as_str(), a)).collect();
        for app in &new {
            match old_by_id.get(app.id.as_str()) {
                None => println!("+ {:<32} {}", app.name, app.id),
                Some(old) if old.name != app.name => {
                    println!("~ {:<32} {} (было: {})", app.name, app.id, old.name)
                }
                Some(_) => {}
            }
        }
        for app in &apps {
            if !new_by_id.contains_key(app.id.as_str()) {
                println!("- {:<32} {}", app.name, app.id);
            }
        }
        apps = new;
    }
}
