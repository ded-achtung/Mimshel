//! Файлы на рабочем столе: слежение за папкой «Рабочий стол» (inotify —
//! ядро само сообщает о переменах) и действия со значками: открыть,
//! переименовать, в корзину, новая папка, переложить в папку, вставить из
//! буфера обмена, терминал в этой папке; места значков в файле. Всё, что может ждать (окна zenity, копирование),
//! — в отдельных потоках; перемены в папке служба увидит через слежение.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

use inotify::{Inotify, WatchMask};
use std::collections::HashMap;

use mimshel_apps::entry::DesktopEntry;
use mimshel_desktop::files::{Item, Kind, MimeDb, free_name, scan, trusted_launcher};
use mimshel_desktop::icon_grid::Slot;

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
        let locale = mimshel_apps::Locale::from_env();
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
        Kind::Launcher if trusted_launcher(&item.path) => spawn(&["gio", "launch", &path], None),
        Kind::Launcher => launch_untrusted(item.path.clone()),
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

/// Ярлык без права на выполнение: сначала спросить, показав настоящую
/// команду. Согласились — пометить его выполняемым (больше не спросим, как
/// «Пометить исполняемым» в Xfce) и запустить.
fn launch_untrusted(path: PathBuf) {
    std::thread::spawn(move || {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let exec = DesktopEntry::parse(&text)
            .string("Exec")
            .unwrap_or_default();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let lang = mimshel_core::i18n::language_from_config();
        let question = mimshel_core::i18n::t(lang, "desktop.launch_text")
            .replace("{name}", &pango_escape(&name))
            .replace("{exec}", &pango_escape(&exec));
        if !mimshel_setup::actions::confirm(
            mimshel_core::i18n::t(lang, "desktop.launch_title"),
            &question,
            mimshel_core::i18n::t(lang, "desktop.launch_ok"),
            "dialog-warning",
        ) {
            return;
        }
        if let Err(e) = mark_executable(&path) {
            error(
                &mimshel_core::i18n::t(lang, "desktop.launcher_not_executable")
                    .replace("{e}", &e.to_string()),
            );
            return;
        }
        spawn(&["gio", "launch", &path.display().to_string()], None);
    });
}

/// Добавить право на выполнение тем, кому разрешено чтение (`chmod +x`).
fn mark_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)?.permissions().mode();
    let mode = mode | ((mode & 0o444) >> 2);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Текст для разметки Pango (окна zenity): `<`, `>`, `&` — не теги.
fn pango_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Whether `path` lies on the same file system as the folder `into` (a
/// move there is a rename). When either cannot be read, assume it does.
pub fn same_device(path: &Path, into: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::symlink_metadata(path), std::fs::metadata(into)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev(),
        _ => true,
    }
}

/// В корзину (её видит и корзина в доке). One `gio trash` per batch of
/// files, not one process per file: deleting 2000 files started 2000
/// processes at once.
pub fn trash<'a>(paths: impl IntoIterator<Item = &'a Path>) {
    for args in trash_commands(paths) {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        spawn(&args, None);
    }
}

/// Files per `gio trash` call: keeps the command line far below the
/// kernel's limit even with long paths.
const TRASH_BATCH: usize = 200;

fn trash_commands<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<Vec<String>> {
    let paths: Vec<String> = paths.into_iter().map(|p| p.display().to_string()).collect();
    paths
        .chunks(TRASH_BATCH)
        .map(|chunk| {
            ["gio", "trash", "--"]
                .into_iter()
                .map(String::from)
                .chain(chunk.iter().cloned())
                .collect()
        })
        .collect()
}

/// Новая папка «Новая папка» (или «Новая папка 2»…). Возвращает путь.
pub fn new_folder(dir: &Path) -> Option<PathBuf> {
    let path = free_name(
        dir,
        mimshel_core::i18n::t(
            mimshel_core::i18n::language_from_config(),
            "folder.new_folder",
        ),
    );
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
    let path = free_name(
        dir,
        mimshel_core::i18n::t(
            mimshel_core::i18n::language_from_config(),
            "desktop.new_text_file_name",
        ),
    );
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
        return Err(mimshel_core::i18n::t(
            mimshel_core::i18n::language_from_config(),
            "desktop.rename_invalid",
        )
        .into());
    }
    let dir = path.parent().ok_or(mimshel_core::i18n::t(
        mimshel_core::i18n::language_from_config(),
        "desktop.rename_no_dir",
    ))?;
    let target = dir.join(new);
    if target == path {
        return Ok(target);
    }
    match std::fs::symlink_metadata(&target) {
        Ok(_) => {
            return Err(mimshel_core::i18n::t(
                mimshel_core::i18n::language_from_config(),
                "desktop.rename_exists",
            )
            .replace("{new}", new));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(mimshel_core::i18n::t(
                mimshel_core::i18n::language_from_config(),
                "desktop.rename_check_failed",
            )
            .replace("{e}", &e.to_string()));
        }
    }
    // std::fs::rename может заменить файл, созданный между проверкой и переименованием.
    // GNU mv с none-fail не заменяет назначения даже при гонке.
    let status = Command::new("mv")
        .args(["--no-clobber", "-T", "--"])
        .arg(path)
        .arg(&target)
        .status()
        .map_err(|e| {
            mimshel_core::i18n::t(
                mimshel_core::i18n::language_from_config(),
                "desktop.rename_failed",
            )
            .replace("{e}", &e.to_string())
        })?;
    if !status.success()
        || std::fs::symlink_metadata(path).is_ok()
        || std::fs::symlink_metadata(&target).is_err()
    {
        return Err(mimshel_core::i18n::t(
            mimshel_core::i18n::language_from_config(),
            "desktop.rename_not_done",
        )
        .into());
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
        for message in transfer(&[(path, dest)], true) {
            error(&message);
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
        // symlink_metadata учитывает битые ссылки. Нельзя считать ошибки
        // доступа «свободным именем», а отсутствие источника — успехом.
        match std::fs::symlink_metadata(dest) {
            Ok(_) => {
                errors.push(
                    mimshel_core::i18n::t(
                        mimshel_core::i18n::language_from_config(),
                        "desktop.transfer_exists",
                    )
                    .replace("{dest}", &dest.display().to_string()),
                );
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                errors.push(
                    mimshel_core::i18n::t(
                        mimshel_core::i18n::language_from_config(),
                        "desktop.transfer_check_failed",
                    )
                    .replace("{dest}", &dest.display().to_string())
                    .replace("{e}", &e.to_string()),
                );
                continue;
            }
        }
        if let Err(e) = std::fs::symlink_metadata(src) {
            errors.push(
                mimshel_core::i18n::t(
                    mimshel_core::i18n::language_from_config(),
                    "desktop.transfer_read_failed",
                )
                .replace("{src}", &src.display().to_string())
                .replace("{e}", &e.to_string()),
            );
            continue;
        }
        let mut cmd = Command::new(if moving { "mv" } else { "cp" });
        cmd.args(["--no-clobber", "-T"]);
        if !moving {
            // Phones (MTP over gvfs) do not keep owners and permissions:
            // `cp -a` would copy the file and still report a failure.
            let phone = dest.starts_with(mimshel_folder::gvfs_dir());
            cmd.arg(if phone { "-r" } else { "-a" });
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
            errors.push(
                mimshel_core::i18n::t(
                    mimshel_core::i18n::language_from_config(),
                    "desktop.transfer_failed",
                )
                .replace("{name}", &name),
            );
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
    mimshel_core::names::dir_in(&state).join("desktop-icons")
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
        let Some((cut, files)) = mimshel_x11::clipboard_files() else {
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
            // Единый путь проверки конфликтов и результатов для вставки,
            // XDND и переноса в папку. Не полагаемся на код возврата cp -n.
            for message in transfer(&[(src, dest)], cut) {
                error(&message);
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
    // Our own terminal (`mimshel terminal …`) takes the folder as an argument.
    if terminal.get(1).map(String::as_str) == Some("terminal") {
        let dir = dir.display().to_string();
        spawn(&[program, "terminal", &dir], Some(Path::new(&dir)));
        return;
    }
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
            &format!(
                "--title={}",
                mimshel_core::i18n::t(
                    mimshel_core::i18n::language_from_config(),
                    "desktop.error_title"
                )
            ),
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
    cmd.args(rest);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    if let Err(e) = crate::ui::spawn_detached(cmd) {
        eprintln!("Не запустить {program}: {e}");
    }
}

#[cfg(test)]
mod transfer_tests {
    use super::*;

    #[test]
    fn same_device_for_files_in_one_folder() {
        let dir = std::env::temp_dir().join(format!("mimshel-dev-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("f"), b"x").unwrap();
        assert!(same_device(&dir.join("f"), &dir.join("sub")));
        // /proc is always its own file system.
        assert!(!same_device(&dir.join("f"), Path::new("/proc")));
        assert!(same_device(&dir.join("missing"), &dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn trash_goes_in_batches() {
        let paths: Vec<PathBuf> = (0..450)
            .map(|i| PathBuf::from(format!("/t/f{i}")))
            .collect();
        let calls = trash_commands(paths.iter().map(PathBuf::as_path));
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0][..4], ["gio", "trash", "--", "/t/f0"]);
        assert_eq!(calls[2].len(), 3 + 50);
        assert!(trash_commands(std::iter::empty()).is_empty());
    }

    #[test]
    fn untrusted_launcher_becomes_trusted_after_consent() {
        use std::os::unix::fs::PermissionsExt;
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mimshel-launcher-{}-{id}.desktop",
            std::process::id()
        ));
        std::fs::write(&path, "[Desktop Entry]\nExec=true\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(!trusted_launcher(&path));
        mark_executable(&path).unwrap();
        // Право на выполнение — только тем, кто может читать.
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert!(trusted_launcher(&path));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            pango_escape("sh -c 'a <b> && c'"),
            "sh -c 'a &lt;b&gt; &amp;&amp; c'"
        );
    }

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

        // Обычный перенос действительно освобождает источник.
        let moved = root.join("moved.txt");
        assert!(transfer(&[(source.clone(), moved.clone())], true).is_empty());
        assert!(!source.exists());
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "new");

        // Битая символическая ссылка — тоже занятое имя.
        let dangling = root.join("dangling");
        std::os::unix::fs::symlink(root.join("missing"), &dangling).unwrap();
        assert!(!transfer(&[(moved.clone(), dangling)], false).is_empty());
        assert!(moved.exists());

        // Переименовать файл в себя можно; чужое имя не перезаписывается.
        assert_eq!(rename(&moved, "moved.txt").unwrap(), moved);
        assert!(rename(&moved, "destination.txt").is_err());
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "original");

        std::fs::remove_dir_all(root).unwrap();
    }
}
