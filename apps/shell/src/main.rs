//! Оболочка рабочего стола: Spotlight, Launchpad и проверочные команды.

mod dock_ui;
mod guard;
mod ui;

use std::collections::HashMap;
use std::time::Instant;

use shell_core::search::{Query, best};

const HELP: &str = "\
Использование:
  shell apps            список программ, которые увидит Launchpad
  shell search ТЕКСТ    что найдёт Spotlight по этому тексту
  shell watch           следить за ярлыками и печатать, что появилось и пропало
  shell daemon          служба: держит Spotlight и Launchpad наготове, рисует док;
                        упадёт — сторож запустит заново (журнал: ~/.local/state/shell/daemon.log)
  shell spotlight       открыть или закрыть Spotlight (через службу, без неё — сам)
  shell launchpad       открыть или закрыть Launchpad (через службу, без неё — сам)
  shell snapshot spotlight|launchpad ТЕКСТ ФАЙЛ.png
                        нарисовать окно с этим текстом в картинку";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("apps") => list_apps(),
        Some("search") if args.len() > 1 => search(&args[1..].join(" ")),
        Some("watch") => watch(),
        Some("daemon") if args.get(1).map(String::as_str) == Some("--child") => {
            if ui::daemon_running() {
                std::process::exit(guard::ALREADY_RUNNING);
            }
            guard::watch_parent();
            exit_on_error(ui::run(None))
        }
        Some("daemon") => guard::run(),
        Some(cmd) if ui::Panel::parse(cmd).is_some() => {
            let panel = ui::Panel::parse(cmd).expect("проверено выше");
            if !ui::ask_daemon(panel) {
                exit_on_error(ui::run(Some(panel)));
            }
        }
        Some("snapshot") if args.len() == 4 && ui::Panel::parse(&args[1]).is_some() => {
            let panel = ui::Panel::parse(&args[1]).expect("проверено выше");
            exit_on_error(ui::snapshot(panel, &args[2], &args[3]))
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
