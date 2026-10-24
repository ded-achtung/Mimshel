//! Файлы на рабочем столе: слежение за папкой «Рабочий стол» (inotify —
//! ядро само сообщает о переменах) и действия со значками: открыть,
//! переименовать, в корзину, новая папка, вставить из буфера обмена,
//! терминал в этой папке. Всё, что может ждать (окна zenity, копирование),
//! — в отдельных потоках; перемены в папке служба увидит через слежение.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Duration;

use inotify::{Inotify, WatchMask};
use shell_desktop::files::{Item, Kind, MimeDb, free_name, scan};

use crate::desktop_ui::DesktopMsg;
use crate::ui::Msg;

/// После события в папке — подождать, пока перемены затихнут (копирование
/// пачки файлов), и тогда перечитать.
const QUIET: Duration = Duration::from_millis(200);

/// Папка «Рабочий стол» (по `xdg-user-dir DESKTOP`, иначе `~/Desktop`).
pub fn desktop_dir() -> PathBuf {
    Command::new("xdg-user-dir")
        .arg("DESKTOP")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Desktop")
        })
}

/// Следить за папкой: сразу и после каждой перемены присылать список значков.
pub fn watch(dir: PathBuf, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let mime = MimeDb::load();
        let locale = shell_apps::Locale::from_env();
        let send = |tx: &Sender<Msg>| {
            let items = scan(&dir, &mime, locale.as_ref());
            tx.send(Msg::Desktop(Box::new(DesktopMsg::Files(items))))
                .is_ok()
        };
        let _ = std::fs::create_dir_all(&dir);
        if !send(&tx) {
            return;
        }
        let Ok(mut inotify) = Inotify::init() else {
            eprintln!("Стол: не следить за {} (inotify)", dir.display());
            return;
        };
        let mask = WatchMask::CREATE
            | WatchMask::DELETE
            | WatchMask::MOVED_FROM
            | WatchMask::MOVED_TO
            | WatchMask::CLOSE_WRITE
            | WatchMask::ATTRIB;
        if inotify.watches().add(&dir, mask).is_err() {
            eprintln!("Стол: не следить за {}", dir.display());
            return;
        }
        let mut buffer = vec![0; 4096];
        loop {
            if inotify.read_events_blocking(&mut buffer).is_err() {
                return;
            }
            // Ждём тишины: пока приходят события, перечитывать рано.
            loop {
                std::thread::sleep(QUIET);
                let more = inotify
                    .read_events(&mut buffer)
                    .is_ok_and(|mut events| events.next().is_some());
                if !more {
                    break;
                }
            }
            if !send(&tx) {
                return;
            }
        }
    });
}

/// Открыть: ярлык — запустить программу, остальное — программой по умолчанию.
pub fn open(item: &Item) {
    let path = item.path.display().to_string();
    match item.kind {
        Kind::Launcher => spawn(&["gio", "launch", &path], None),
        // gio не нашёл программу (бывает, когда список «тип → программа» в
        // ~/.local/share/applications устарел) — пробуем xdg-open.
        _ => spawn(
            &[
                "sh",
                "-c",
                "gio open \"$1\" || xdg-open \"$1\"",
                "open",
                &path,
            ],
            None,
        ),
    }
}

/// В корзину (её видит и корзина в доке).
pub fn trash(item: &Item) {
    spawn(&["gio", "trash", &item.path.display().to_string()], None);
}

/// Новая папка «Новая папка» (или «Новая папка 2»…). Возвращает путь.
pub fn new_folder(dir: &Path) -> Option<PathBuf> {
    let path = free_name(dir, "Новая папка");
    match std::fs::create_dir(&path) {
        Ok(()) => Some(path),
        Err(e) => {
            eprintln!("Стол: папка не создана: {e}");
            None
        }
    }
}

/// Переименовать: окно с новым именем (zenity). Выбранное имя — сообщением
/// `Select`, чтобы выделение перешло на переименованный файл.
pub fn rename(item: &Item, tx: Sender<Msg>) {
    let path = item.path.clone();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    std::thread::spawn(move || {
        let out = Command::new("zenity")
            .args([
                "--entry",
                "--title=Переименовать",
                "--text=Новое имя:",
                &format!("--entry-text={name}"),
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let Some(new) = out
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && *s != name)
        else {
            return;
        };
        if new.contains('/') {
            error("В имени не может быть знака «/».");
            return;
        }
        let Some(dir) = path.parent() else {
            return;
        };
        let target = dir.join(&new);
        if target.exists() {
            error(&format!("«{new}» уже есть на рабочем столе."));
            return;
        }
        match std::fs::rename(&path, &target) {
            Ok(()) => {
                let _ = tx.send(Msg::Desktop(Box::new(DesktopMsg::Select(target))));
            }
            Err(e) => error(&format!("Не удалось переименовать: {e}")),
        }
    });
}

/// Вставить файлы из буфера обмена (скопированные или вырезанные в
/// Проводнике): копировать или переместить в папку стола. Имя занято —
/// «имя 2».
pub fn paste(dir: PathBuf) {
    std::thread::spawn(move || {
        let Some((cut, files)) = shell_x11::clipboard_files() else {
            return;
        };
        for src in files {
            let Some(name) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            // Скопировать файл туда же, где он лежит, — нет смысла (Mac так же
            // делает копию, но рядом с исходным): на столе — копия «имя 2».
            let dest = free_name(&dir, &name);
            let (program, verb) = if cut {
                ("mv", "переместить")
            } else {
                ("cp", "скопировать")
            };
            let mut cmd = Command::new(program);
            if !cut {
                cmd.arg("-a");
            }
            let ok = cmd
                .arg("--")
                .arg(&src)
                .arg(&dest)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                error(&format!("Не удалось {verb} «{name}»."));
            }
        }
    });
}

/// Терминал в папке стола.
pub fn terminal(terminal: &[String], dir: &Path) {
    let Some(program) = terminal.first() else {
        return;
    };
    // Уже запущенный xfce4-terminal (и похожие) открывает окно в своей
    // папке, а не в нашей, — папку им передаём явно.
    let name = program.rsplit('/').next().unwrap_or(program);
    let flag = match name {
        "xfce4-terminal" | "gnome-terminal" | "mate-terminal" | "lxterminal" => {
            Some(format!("--working-directory={}", dir.display()))
        }
        "konsole" => Some(format!("--workdir={}", dir.display())),
        _ => None,
    };
    match &flag {
        Some(flag) => spawn(&[program, flag], Some(dir)),
        None => spawn(&[program], Some(dir)),
    }
}

/// Сообщение об ошибке (окно zenity).
fn error(text: &str) {
    spawn(
        &[
            "zenity",
            "--error",
            "--title=Рабочий стол",
            &format!("--text={text}"),
        ],
        None,
    );
}

/// Запустить программу отдельно от службы (в своей группе процессов).
pub fn spawn(args: &[&str], dir: Option<&Path>) {
    let Some((program, rest)) = args.split_first() else {
        return;
    };
    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    match cmd.spawn() {
        // Дожидаемся в отдельном потоке, чтобы не копились «зомби».
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => eprintln!("Не запустить {program}: {e}"),
    }
}
