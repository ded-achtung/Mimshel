//! Оболочка рабочего стола: Spotlight, Launchpad и проверочные команды.

mod config_store;
mod dock_ui;
mod guard;
mod ipc;
mod menubar_ui;
mod notify_ui;
mod setup;
mod status;
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
                        нарисовать окно с этим текстом в картинку
  shell doctor [--fix]  проверить, что всё на месте; --fix — починить
  shell config          где файл настроек и что в нём можно менять
  shell set [--no-apply] КЛЮЧ ЗНАЧЕНИЕ
                        поменять настройку и применить (например: shell set dock.icon_size 56,
                        shell set parts.dock plank — вернуть Plank)
  shell apply [ЧАСТЬ…]  применить заново: menubar dock notify launchers lock power service
                        (без частей — все)
  shell lock            заблокировать экран (i3lock-color; упадёт без пароля — запустится снова)
  shell action ДЕЙСТВИЕ пункт меню яблока: about settings forcequit sleep restart shutdown lock logout
  shell backup [ПОМЕТКА] сохранить настройки Xfce, Plank и shell
  shell backups         список копий
  shell restore [--dry-run] [ИМЯ]
                        вернуть настройки из копии (по умолчанию — самой первой, «как было»);
                        --dry-run — только показать, что будет сделано";

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
        Some("doctor") if args.len() == 1 || (args.len() == 2 && args[1] == "--fix") => {
            std::process::exit(setup::doctor(args.len() == 2))
        }
        Some("set") if args.len() >= 4 && args[1] == "--no-apply" => {
            std::process::exit(setup::set(&args[2], &args[3..].join(" "), false))
        }
        Some("set") if args.len() >= 3 => {
            std::process::exit(setup::set(&args[1], &args[2..].join(" "), true))
        }
        Some("apply") => std::process::exit(setup::apply_cmd(&args[1..])),
        Some("config") => std::process::exit(setup::show_config()),
        Some("lock") if args.len() == 1 => {
            // Меню и xflock4 ждут, пока команда вернётся: сторож уходит в фон.
            let exe = guard::own_exe();
            let exe = exe.to_string_lossy();
            let _ = std::process::Command::new("setsid")
                .args(["-f", &exe, "lock", "--nofork"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        Some("lock") if args.len() == 2 && args[1] == "--nofork" => {
            let look = shell_setup::look::Look::load();
            let wallpaper = ui::load_config().wallpaper;
            std::process::exit(shell_setup::lock::supervise(&look, &wallpaper))
        }
        Some("backup") if args.len() <= 2 => {
            std::process::exit(setup::backup(args.get(1).map(String::as_str)))
        }
        Some("backups") => std::process::exit(setup::backups()),
        Some("restore") => std::process::exit(setup::restore(&args[1..])),
        Some("action") if args.len() == 2 => {
            std::process::exit(shell_setup::actions::action(&args[1], &guard::own_exe()))
        }
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
