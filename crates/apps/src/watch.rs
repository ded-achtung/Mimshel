//! Слежение за папками с ярлыками: ядро Linux (inotify) само сообщает, когда
//! ярлык появился, изменился или пропал — например, после установки программы
//! через pacman. Тогда список программ пора перечитать.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};

use crate::Settings;

/// Сколько ждать тишины после последнего события: pacman ставит программу
/// пачкой файлов, и перечитывать список после каждого незачем.
const QUIET: Duration = Duration::from_millis(300);
const TICK: Duration = Duration::from_millis(50);

/// Что именно наблюдаем по данному номеру.
enum Watched {
    /// Папка с ярлыками (или вложенная в неё).
    Apps,
    /// Родитель ещё не созданной папки с ярлыками: важно только появление
    /// папки с этим именем (например, `applications` в `~/.local/share`).
    ParentOf(OsString),
}

pub struct Watcher {
    inotify: Inotify,
    roots: Vec<PathBuf>,
    watches: HashMap<WatchDescriptor, Watched>,
    buffer: Vec<u8>,
}

impl Watcher {
    /// Начинает следить за всеми папками из настроек.
    pub fn new(settings: &Settings) -> io::Result<Watcher> {
        let mut w = Watcher {
            inotify: Inotify::init()?,
            roots: settings.dirs.clone(),
            watches: HashMap::new(),
            buffer: vec![0; 4096],
        };
        w.rewatch();
        Ok(w)
    }

    /// Ждёт, пока в ярлыках что-то изменится и всё затихнет. Когда функция
    /// вернулась — список программ пора перечитать.
    pub fn wait(&mut self) -> io::Result<()> {
        while !self.read(true)? {}
        let mut quiet = Duration::ZERO;
        while quiet < QUIET {
            std::thread::sleep(TICK);
            if self.read(false)? {
                quiet = Duration::ZERO;
            } else {
                quiet += TICK;
            }
        }
        // Могли появиться новые папки — следим и за ними.
        self.rewatch();
        Ok(())
    }

    /// Читает накопившиеся события. `true` — среди них есть важные.
    fn read(&mut self, block: bool) -> io::Result<bool> {
        let events = if block {
            self.inotify.read_events_blocking(&mut self.buffer)
        } else {
            self.inotify.read_events(&mut self.buffer)
        };
        let events = match events {
            Ok(events) => events,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) => return Err(e),
        };
        let mut important = false;
        for e in events {
            important |= is_important(self.watches.get(&e.wd), e.mask, e.name);
        }
        Ok(important)
    }

    /// Ставит наблюдение на все существующие папки с ярлыками, а для
    /// несуществующих — на их родителя. Повторная постановка безвредна.
    fn rewatch(&mut self) {
        let apps_mask = WatchMask::CREATE
            | WatchMask::DELETE
            | WatchMask::CLOSE_WRITE
            | WatchMask::MOVED_FROM
            | WatchMask::MOVED_TO
            | WatchMask::DELETE_SELF
            | WatchMask::MOVE_SELF;
        let parent_mask = WatchMask::CREATE | WatchMask::MOVED_TO | WatchMask::ONLYDIR;

        for root in self.roots.clone() {
            if root.is_dir() {
                let mut dirs = Vec::new();
                collect_dirs(&root, &mut dirs);
                for dir in dirs {
                    // Нет прав или папка уже пропала — просто пропускаем.
                    if let Ok(wd) = self.inotify.watches().add(&dir, apps_mask) {
                        self.watches.insert(wd, Watched::Apps);
                    }
                }
            } else if let (Some(parent), Some(name)) = (root.parent(), root.file_name())
                && let Ok(wd) = self.inotify.watches().add(parent, parent_mask)
            {
                self.watches.insert(wd, Watched::ParentOf(name.to_owned()));
            }
        }
    }
}

fn is_important(watched: Option<&Watched>, mask: EventMask, name: Option<&OsStr>) -> bool {
    if mask.contains(EventMask::Q_OVERFLOW) {
        // Событий было так много, что часть потерялась: перечитать на всякий случай.
        return true;
    }
    match watched {
        Some(Watched::Apps) => {
            mask.intersects(EventMask::ISDIR | EventMask::DELETE_SELF | EventMask::MOVE_SELF)
                || name.is_some_and(|n| Path::new(n).extension() == Some(OsStr::new("desktop")))
        }
        Some(Watched::ParentOf(wanted)) => name == Some(wanted.as_os_str()),
        None => false,
    }
}

/// Папка и все вложенные папки.
fn collect_dirs(dir: &Path, out: &mut Vec<PathBuf>) {
    out.push(dir.to_path_buf());
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for item in read.flatten() {
        let path = item.path();
        if path.is_dir() {
            collect_dirs(&path, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use std::sync::mpsc;

    /// Запускает ожидание в отдельном потоке; сообщит, когда оно закончится.
    fn start(dirs: Vec<PathBuf>) -> mpsc::Receiver<()> {
        let settings = Settings {
            dirs,
            locale: None,
            desktops: vec![],
            path: vec![],
            hide: vec![],
            rename: vec![],
        };
        let mut w = Watcher::new(&settings).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while w.wait().is_ok() {
                if tx.send(()).is_err() {
                    break;
                }
            }
        });
        rx
    }

    const SOON: Duration = Duration::from_secs(3);
    const NOT_YET: Duration = Duration::from_millis(700);

    #[test]
    fn new_desktop_file() {
        let dir = TempDir::new("watch-new");
        let rx = start(vec![dir.0.clone()]);
        dir.write("app.desktop", "[Desktop Entry]\n");
        assert!(rx.recv_timeout(SOON).is_ok());
    }

    #[test]
    fn other_files_ignored() {
        let dir = TempDir::new("watch-other");
        let rx = start(vec![dir.0.clone()]);
        dir.write("mimeinfo.cache", "x");
        assert!(rx.recv_timeout(NOT_YET).is_err());
    }

    #[test]
    fn subfolder_and_late_folder() {
        let base = TempDir::new("watch-late");
        let apps = base.0.join("applications");
        let rx = start(vec![apps.clone()]);

        // Папки с ярлыками ещё нет — её появление тоже повод перечитать.
        base.write("unrelated.txt", "x");
        assert!(rx.recv_timeout(NOT_YET).is_err());
        std::fs::create_dir(&apps).unwrap();
        assert!(rx.recv_timeout(SOON).is_ok());

        // После этого следим уже за ней самой и за вложенными папками.
        base.write("applications/kde/konsole.desktop", "[Desktop Entry]\n");
        assert!(rx.recv_timeout(SOON).is_ok());
        base.write("applications/kde/more.desktop", "[Desktop Entry]\n");
        assert!(rx.recv_timeout(SOON).is_ok());
    }
}
