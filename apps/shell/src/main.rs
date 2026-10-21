//! Оболочка рабочего стола. Пока здесь только проверочные команды без окон.

use std::time::Instant;

use shell_core::search::{Fields, Query, best};

const HELP: &str = "\
Использование:
  shell apps            список программ, которые увидит Launchpad
  shell search ТЕКСТ    что найдёт Spotlight по этому тексту";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("apps") => list_apps(),
        Some("search") if args.len() > 1 => search(&args[1..].join(" ")),
        _ => {
            println!("{HELP}");
            std::process::exit(2);
        }
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
    let found = best(&query, &apps, |a| Fields {
        names: vec![a.name.as_str(), a.name_untranslated.as_str()],
        keywords: a.keywords.iter().map(String::as_str).collect(),
        others: a
            .generic_name
            .iter()
            .chain(&a.comment)
            .map(String::as_str)
            .collect(),
    });
    eprintln!("Поиск занял {:?}", start.elapsed());
    for app in found.iter().take(10) {
        println!("{:<32} {}", app.name, app.id);
    }
}
