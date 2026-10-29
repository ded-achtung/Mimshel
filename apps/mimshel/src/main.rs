//! Оболочка рабочего стола: служба, окна «Поиск» и «Программы», настройка
//! и проверочные команды.

mod about_ui;
mod config_store;
mod crash;
mod desktop_files;
mod desktop_ui;
mod desktop_wallpaper;
mod dock_ui;
mod folder_ui;
mod guard;
mod ipc;
mod lock_ui;
mod menubar_ui;
mod notify_ui;
mod settings_system;
mod settings_ui;
mod setup;
mod status;
mod term_ui;
mod ui;

use std::collections::HashMap;
use std::time::Instant;

use mimshel_core::search::{Query, best};

const HELP: &str = "\
Использование:
  mimshel search          открыть или закрыть окно «Поиск» (через службу, без неё — сам)
  mimshel apps            открыть или закрыть окно «Программы» (так же)
  mimshel daemon          служба: строка меню, док, уведомления, окна «Поиск» и «Программы»;
                        упадёт — сторож запустит заново (журнал: ~/.local/state/mimshel/daemon.log)
  mimshel snapshot search|apps ТЕКСТ ФАЙЛ.png
                        нарисовать окно с этим текстом в картинку
  mimshel --version       версия программы
  mimshel settings [РАЗДЕЛ] окно «Системные настройки» (appearance wallpaper menubar dock
                        lock apps terminal parts)
  mimshel snapshot settings РАЗДЕЛ ФАЙЛ.png
                        нарисовать окно настроек в картинку (проверка вида)
  mimshel terminal [ПАПКА] свой терминал (отдельное окно; оболочка — в папке);
                        mimshel terminal -e ПРОГРАММА … — запустить программу в нём
  mimshel folder [ПАПКА…] открыть папку в окне (по умолчанию — домашнюю; через службу,
                        без неё или при parts.folders = thunar — в Thunar)
  mimshel test-apps       список программ, которые увидит окно «Программы»
  mimshel test-search ТЕКСТ
                        что найдёт окно «Поиск» по этому тексту
  mimshel test-clipboard  какие файлы в буфере обмена (для «Вставить» на столе)
  mimshel watch           следить за ярлыками и печатать, что появилось и пропало
  mimshel doctor --report [ПАПКА]  собрать обезличенный отчёт (по умолчанию — дома)
  mimshel doctor [--fix]  проверить, что всё на месте; --fix — починить
  mimshel crashes [last]  отчёты о сбоях: время, часть, причина (новые первыми;
                        last — последний отчёт целиком)
  mimshel config          где файл настроек и что в нём можно менять
  mimshel set [--no-apply] КЛЮЧ ЗНАЧЕНИЕ
                        поменять настройку и применить (например: mimshel set dock.icon_size 56,
                        mimshel set parts.dock plank — вернуть Plank)
  mimshel apply [ЧАСТЬ…]  применить заново: menubar dock notify desktop folders
                        launchers lock power icons look wallpaper service
                        (launchers — сочетания клавиш окон «Поиск» и «Программы»;
                        без частей — все)
  mimshel apply greeter [--dry-run]
                        оформить экран входа в систему (LightDM): свои обои, тема,
                        курсор и шрифт (нужен пароль sudo; --dry-run — только показать)
  mimshel lock            заблокировать экран (свой экран блокировки; упадёт без пароля —
                        сторож запустит снова)
  mimshel snapshot lock ТЕКСТ ФАЙЛ.png
                        нарисовать экран блокировки в картинку (проверка вида)
  mimshel action ДЕЙСТВИЕ пункт меню яблока: about settings forcequit sleep restart shutdown lock logout
  mimshel backup [ПОМЕТКА] сохранить настройки Xfce, Plank и shell
  mimshel backups         список копий
  mimshel restore [--dry-run] [ИМЯ]
                        вернуть настройки из копии (по умолчанию — самой первой, «как было»);
                        --dry-run — только показать, что будет сделано";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Any part that fails must leave an understandable trace — install
    // the hook before anything else.
    crash::install(component(&args));
    match args.first().map(String::as_str) {
        Some("test-apps") => list_apps(),
        Some("test-clipboard") => match mimshel_x11::clipboard_files() {
            Some((cut, files)) => {
                println!(
                    "{}:",
                    if cut {
                        "вырезаны"
                    } else {
                        "скопированы"
                    }
                );
                for f in files {
                    println!("  {}", f.display());
                }
            }
            None => println!("в буфере обмена нет файлов"),
        },
        Some("test-search") if args.len() > 1 => search(&args[1..].join(" ")),
        Some("watch") => watch(),
        Some("daemon") if args.get(1).map(String::as_str) == Some("--child") => {
            if ui::daemon_running() {
                std::process::exit(guard::ALREADY_RUNNING);
            }
            guard::watch_parent();
            exit_on_error(ui::run(None))
        }
        Some("daemon") => guard::run(),
        Some("doctor") if (args.len() == 2 || args.len() == 3) && args[1] == "--report" => {
            let dir = args
                .get(2)
                .map(std::path::PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from));
            let result = dir
                .ok_or_else(|| std::io::Error::other("нет HOME: укажите папку"))
                .and_then(|dir| {
                    let path = ui::config_path();
                    let valid = |p: &std::path::Path| {
                        std::fs::read_to_string(p)
                            .ok()
                            .filter(|text| mimshel_core::config::toml_error(text).is_none())
                    };
                    // Read without migrating, creating or repairing the user's configuration.
                    let text = path.as_deref().and_then(valid).or_else(|| {
                        path.as_ref()
                            .and_then(|p| ui::config_store(p.clone()).good)
                            .as_deref()
                            .and_then(valid)
                    });
                    let cfg = text
                        .as_deref()
                        .map(mimshel_core::config::Config::from_toml)
                        .unwrap_or_default();
                    mimshel_setup::report::make(&dir, &cfg, path.as_deref())
                });
            match result {
                Ok(report) => {
                    println!(
                        "Отчёт: {}\nРазмер: {} байт\nВнутри:",
                        report.path.display(),
                        report.size
                    );
                    for file in report.files {
                        println!("  {file}");
                    }
                    println!(
                        "Перед отправкой кому-либо обязательно просмотрите содержимое архива!"
                    );
                }
                Err(e) => {
                    eprintln!("Не удалось собрать отчёт: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("doctor") if args.len() == 1 || (args.len() == 2 && args[1] == "--fix") => {
            std::process::exit(setup::doctor(args.len() == 2))
        }
        Some("crashes") => std::process::exit(crash::crashes_cmd(&args[1..])),
        Some("set") if args.len() >= 4 && args[1] == "--no-apply" => {
            std::process::exit(setup::set(&args[2], &args[3..].join(" "), false))
        }
        Some("set") if args.len() >= 3 => {
            std::process::exit(setup::set(&args[1], &args[2..].join(" "), true))
        }
        Some("apply") => std::process::exit(setup::apply_cmd(&args[1..])),
        Some("config") => std::process::exit(setup::show_config()),
        Some("reload") => {
            if ui::ask_daemon_reload() {
                println!("Служба перечитала настройки (без перезапуска)");
            } else {
                println!("Служба не запущена — настройки применятся при следующем запуске");
                std::process::exit(1);
            }
        }
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
            std::process::exit(mimshel_setup::lock::supervise(&guard::own_exe()))
        }
        Some("lock-screen") => std::process::exit(lock_ui::run()),
        Some("backup") if args.len() <= 2 => {
            std::process::exit(setup::backup(args.get(1).map(String::as_str)))
        }
        Some("backups") => std::process::exit(setup::backups()),
        Some("restore") => std::process::exit(setup::restore(&args[1..])),
        Some("--version" | "-V" | "version") => {
            println!("mimshel {}", env!("CARGO_PKG_VERSION"));
        }
        Some("settings") if args.len() <= 2 => {
            std::process::exit(settings_ui::run(args.get(1).map(String::as_str)))
        }
        Some("terminal") => {
            let (dir, command) = term_ui::parse_args(&args[1..]);
            std::process::exit(term_ui::run(dir, command))
        }
        Some("folder") => {
            for path in folder_ui::folder_args(&args[1..]) {
                if !ui::ask_daemon_folder(&path) {
                    folder_ui::open_elsewhere(&path);
                }
            }
        }
        // «Об этом компьютере» — своё окно в службе; без неё — окно zenity.
        Some("action") if args.len() == 2 && args[1] == "about" && ui::ask_daemon_about() => {}
        Some("action") if args.len() == 2 => {
            std::process::exit(mimshel_setup::actions::action(&args[1], &guard::own_exe()))
        }
        Some(cmd) if ui::Panel::parse(cmd).is_some() => {
            let panel = ui::Panel::parse(cmd).expect("проверено выше");
            if !ui::ask_daemon(panel) {
                exit_on_error(ui::run(Some(panel)));
            }
        }
        Some("snapshot") if args.len() == 4 && args[1] == "folder" => exit_on_error(
            folder_ui::snapshot(std::path::Path::new(&args[2]), &args[3]),
        ),
        Some("snapshot") if args.len() == 4 && args[1] == "settings" => {
            exit_on_error(settings_ui::snapshot(&args[2], &args[3]))
        }
        Some("snapshot") if args.len() == 4 && args[1] == "lock" => {
            exit_on_error(lock_ui::snapshot(&args[2], &args[3]))
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

fn exit_on_error(result: mimshel_x11::Result<()>) {
    if let Err(e) = result {
        eprintln!("Ошибка: {e}");
        std::process::exit(1);
    }
}

/// Which part runs — for the crash report (file name and header).
/// `mimshel daemon --child` is the service, the named commands are their
/// own parts, everything else is just a command.
fn component(args: &[String]) -> &'static str {
    match (
        args.first().map(String::as_str),
        args.get(1).map(String::as_str),
    ) {
        (Some("daemon"), Some("--child")) => "service",
        (Some("lock-screen"), _) => "lock-screen",
        (Some("terminal"), _) => "terminal",
        (Some("folder"), _) => "folder",
        (Some("settings"), _) => "settings",
        _ => "cli",
    }
}

fn load() -> Vec<mimshel_apps::App> {
    let start = Instant::now();
    let apps = mimshel_apps::load(&ui::app_settings(&ui::load_config()));
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
    let found = best(&query, &apps, mimshel_launcher::fields);
    eprintln!("Поиск занял {:?}", start.elapsed());
    for app in found.iter().take(10) {
        println!("{:<32} {}", app.name, app.id);
    }
}

fn watch() {
    let settings = ui::app_settings(&ui::load_config());
    let mut watcher = match mimshel_apps::Watcher::new(&settings) {
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
        let old_by_id: HashMap<&str, &mimshel_apps::App> =
            apps.iter().map(|a| (a.id.as_str(), a)).collect();
        let new_by_id: HashMap<&str, &mimshel_apps::App> =
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
