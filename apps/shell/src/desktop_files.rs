//! Файлы на рабочем столе: слежение за папкой «Рабочий стол» (inotify —
//! ядро само сообщает о переменах) и действия со значками: открыть,
//! переименовать, в корзину, новая папка, переложить в папку, вставить из
//! буфера обмена, терминал в этой папке; места значков в файле. Всё, что может ждать (окна zenity, копирование),
//! — в отдельных потоках; перемены в папке служба увидит через слежение.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Duration;

use inotify::{Inotify, WatchMask};
use std::collections::HashMap;

use shell_desktop::files::{Item, Kind, MimeDb, free_name, scan};
use shell_desktop::icon_grid::Slot;

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

/// Новый пустой текстовый файл «Новый документ.txt» (или «… 2.txt»).
pub fn new_text_file(dir: &Path) -> Option<PathBuf> {
    let path = free_name(dir, "Новый документ.txt");
    match std::fs::File::create_new(&path) {
        Ok(_) => Some(path),
        Err(e) => {
            eprintln!("Стол: файл не создан: {e}");
            None
        }
    }
}

/// Переименовать файл в той же папке. Ошибка — словами для окна.
pub fn rename(path: &Path, new: &str) -> Result<PathBuf, String> {
    if new.is_empty() || matches!(new, "." | "..") || new.contains('/') {
        return Err("Недопустимое имя файла.".into());
    }
    let dir = path.parent().ok_or("нет папки")?;
    let target = dir.join(new);
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(format!("«{new}» уже есть в этой папке."));
    }
    if target == path {
        return Ok(target);
    }
    // std::fs::rename может заменить файл, созданный между проверкой и переименованием.
    // GNU mv с none-fail не заменяет назначения даже при гонке.
    let status = Command::new("mv")
        .args(["--no-clobber", "-T", "--"])
        .arg(path)
        .arg(&target)
        .status()
        .map_err(|e| format!("Не удалось переименовать: {e}"))?;
    if !status.success()
        || std::fs::symlink_metadata(path).is_ok()
        || std::fs::symlink_metadata(&target).is_err()
    {
        return Err("Переименование не выполнено: имя занято или файл недоступен.".into());
    }
    Ok(target)
}

/// Переложить файл в папку (перетащили значок на папку). Имя там занято —
/// «имя 2».
pub fn move_into(path: PathBuf, folder: PathBuf) {
    std::thread::spawn(move || {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            return;
        };
        let dest = free_name(&folder, &name);
        let ok = Command::new("mv")
            .args(["--no-clobber", "-T", "--"])
            .arg(&path)
            .arg(&dest)
            .status()
            .is_ok_and(|s| s.success());
        if !ok
            || std::fs::symlink_metadata(&path).is_ok()
            || std::fs::symlink_metadata(&dest).is_err()
        {
            error(&format!("Не удалось переложить «{name}» в папку."));
        }
    });
}

/// Файловая операция выполняется последовательно для всей группы.
/// Ошибки возвращаются вызывающему: тесты не открывают окна zenity.
fn transfer(pairs: &[(PathBuf, PathBuf)], moving: bool) -> Vec<String> {
    let mut errors = Vec::new();
    for (src, dest) in pairs {
        // GNU coreutils на Ubuntu CI может быть старше 9.5: --update=none-fail
        // недоступен. Проверка препятствует ложному успеху для уже занятых путей,
        // а --no-clobber защищает назначение от конкурентной записи.
        if std::fs::symlink_metadata(dest).is_ok() {
            errors.push(format!(
                "Файл назначения {} уже существует.",
                dest.display()
            ));
            continue;
        }
        let mut cmd = Command::new(if moving { "mv" } else { "cp" });
        cmd.args(["--no-clobber", "-T"]);
        if !moving {
            cmd.arg("-a");
        }
        let ok = cmd
            .arg("--")
            .arg(src)
            .arg(dest)
            .status()
            .is_ok_and(|s| s.success());
        // GNU cp/mv --no-clobber может возвращать код 0, пропустив
        // существующий файл. Проверяем фактическое состояние после команды.
        let actually_done = std::fs::symlink_metadata(dest).is_ok()
            && (!moving || std::fs::symlink_metadata(src).is_err());
        if !ok || !actually_done {
            let name = src.file_name().unwrap_or_default().to_string_lossy();
            errors.push(format!(
                "Не удалось перенести «{name}»: файл назначения занят или недоступен."
            ));
        }
    }
    errors
}

/// Перенести или скопировать и отправить событие XDND только после операции.
pub fn bring(pairs: Vec<(PathBuf, PathBuf)>, moving: bool, ticket: u64, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let errors = transfer(&pairs, moving);
        let success = errors.is_empty();
        for message in errors {
            error(&message);
        }
        let _ = tx.send(Msg::DropFinished { ticket, success });
    });
}

/// Перенести или скопировать в фоне (перетащили внутри окна папки).
pub fn transfer_async(pairs: Vec<(PathBuf, PathBuf)>, moving: bool) {
    if pairs.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for message in transfer(&pairs, moving) {
            error(&message);
        }
    });
}

/// Где запомнены места значков: строки «столбец строка<Tab>имя файла».
fn places_file() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_default();
    state.join("shell/desktop-icons")
}

pub fn load_places() -> HashMap<String, Slot> {
    std::fs::read_to_string(places_file())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (pos, name) = l.split_once('\t')?;
            let (c, r) = pos.split_once(' ')?;
            Some((name.to_string(), (c.parse().ok()?, r.parse().ok()?)))
        })
        .collect()
}

pub fn save_places(places: &HashMap<String, Slot>) {
    let mut lines: Vec<String> = places
        .iter()
        .map(|(name, (c, r))| format!("{c} {r}\t{name}"))
        .collect();
    lines.sort();
    let path = places_file();
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, lines.join("\n") + "\n")?;
        std::fs::rename(&tmp, &path)
    };
    if let Err(e) = write() {
        eprintln!("Стол: места значков не сохранены: {e}");
    }
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
            // Вырезанный со стола обратно на стол — он уже здесь, ничего не
            // делать; скопированный — копия рядом, «имя 2» (как на Mac).
            if cut && src.parent() == Some(dir.as_path()) {
                continue;
            }
            let dest = free_name(&dir, &name);
            let (program, verb) = if cut {
                ("mv", "переместить")
            } else {
                ("cp", "скопировать")
            };
            let mut cmd = Command::new(program);
            cmd.args(["--no-clobber", "-T"]);
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
pub fn error(text: &str) {
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

#[cfg(test)]
mod transfer_tests {
    use super::*;

    #[test]
    fn move_and_copy_never_replace_an_existing_file() {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("shell-transfer-{}-{id}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.txt");
        let existing = root.join("destination.txt");
        let new = root.join("new.txt");
        std::fs::write(&source, "new").unwrap();
        std::fs::write(&existing, "original").unwrap();

        for moving in [false, true] {
            assert!(!transfer(&[(source.clone(), existing.clone())], moving).is_empty());
            assert_eq!(std::fs::read_to_string(&existing).unwrap(), "original");
            assert_eq!(std::fs::read_to_string(&source).unwrap(), "new");
        }
        assert!(transfer(&[(source.clone(), new.clone())], false).is_empty());
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "new");
        assert!(source.exists());

        std::fs::remove_dir_all(root).unwrap();
    }
}
