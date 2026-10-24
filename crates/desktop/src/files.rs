//! Что лежит на рабочем столе: файлы и папки из папки «Рабочий стол»
//! (`xdg-user-dir DESKTOP`) с названием и именем значка.
//!
//! Тип файла — по стандарту freedesktop (shared-mime-info): расширение →
//! тип по `globs2`, тип → значок (`image/png` → `image-png`, общий — из
//! `generic-icons`, например `image-x-generic`). Ярлык `.desktop` — значком
//! и названием своей программы.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use shell_apps::Locale;
use shell_apps::entry::DesktopEntry;

/// Что за значок.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Folder,
    File,
    /// Ярлык программы (`.desktop`).
    Launcher,
}

/// Значок на столе.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub path: PathBuf,
    /// Подпись: имя файла или название программы у ярлыка.
    pub name: String,
    pub kind: Kind,
    /// Имена значка в теме по порядку (первый найденный и берётся).
    pub icons: Vec<String>,
}

/// Типы файлов системы: расширение → тип и тип → общий значок.
#[derive(Default)]
pub struct MimeDb {
    /// `*.png` → `image/png` (с наибольшим весом; длинные расширения —
    /// `tar.gz` — тоже).
    by_ext: HashMap<String, String>,
    generic: HashMap<String, String>,
}

impl MimeDb {
    /// Из `/usr/share/mime` и `~/.local/share/mime`.
    pub fn load() -> MimeDb {
        let mut dirs = vec![PathBuf::from("/usr/share/mime")];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".local/share/mime"));
        }
        let mut db = MimeDb::default();
        for dir in dirs {
            if let Ok(text) = std::fs::read_to_string(dir.join("globs2")) {
                db.add_globs(&text);
            }
            if let Ok(text) = std::fs::read_to_string(dir.join("generic-icons")) {
                db.add_generic(&text);
            }
        }
        db
    }

    /// Строки `вес:тип:*.расш` — берём простые `*.расш` с наибольшим весом.
    fn add_globs(&mut self, text: &str) {
        let mut weights: HashMap<String, u32> = HashMap::new();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let mut parts = line.splitn(4, ':');
            let (Some(w), Some(mime), Some(glob)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Some(ext) = glob.strip_prefix("*.") else {
                continue;
            };
            if ext.contains(['*', '?', '[']) {
                continue;
            }
            let w: u32 = w.parse().unwrap_or(50);
            let ext = ext.to_lowercase();
            if weights.get(&ext).is_none_or(|&old| w > old) {
                weights.insert(ext.clone(), w);
                self.by_ext.insert(ext, mime.to_string());
            }
        }
    }

    fn add_generic(&mut self, text: &str) {
        for line in text.lines() {
            if let Some((mime, icon)) = line.split_once(':') {
                self.generic.insert(mime.to_string(), icon.to_string());
            }
        }
    }

    /// Тип файла по имени (`отчёт.tar.gz` → `application/x-compressed-tar`).
    pub fn mime(&self, name: &str) -> Option<&str> {
        let lower = name.to_lowercase();
        // Сначала самое длинное расширение: «tar.gz» раньше «gz».
        let mut rest = lower.as_str();
        while let Some((_, ext)) = rest.split_once('.') {
            if let Some(m) = self.by_ext.get(ext) {
                return Some(m);
            }
            rest = ext;
        }
        None
    }

    /// Имена значков для типа: свой, общий, общий по первой части, запасной.
    pub fn icons(&self, mime: Option<&str>) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(m) = mime {
            out.push(m.replace('/', "-"));
            if let Some(g) = self.generic.get(m) {
                out.push(g.clone());
            }
            if let Some((major, _)) = m.split_once('/') {
                out.push(format!("{major}-x-generic"));
            }
        }
        out.push("text-x-generic".into());
        out.push("unknown".into());
        out
    }
}

/// Значки для папки `dir`: скрытые (с точки) не показываются; папки — первыми,
/// дальше по имени без учёта регистра.
pub fn scan(dir: &Path, mime: &MimeDb, locale: Option<&Locale>) -> Vec<Item> {
    let mut items: Vec<Item> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let file_name = e.file_name().to_string_lossy().into_owned();
            if file_name.starts_with('.') {
                return None;
            }
            let path = e.path();
            if path.is_dir() {
                return Some(Item {
                    path,
                    name: file_name,
                    kind: Kind::Folder,
                    icons: vec!["folder".into()],
                });
            }
            if file_name.ends_with(".desktop")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                let entry = DesktopEntry::parse(&text);
                let name = entry
                    .localized("Name", locale)
                    .unwrap_or_else(|| file_name.trim_end_matches(".desktop").to_string());
                let mut icons: Vec<String> = entry.string("Icon").into_iter().collect();
                icons.push("application-x-executable".into());
                return Some(Item {
                    path,
                    name,
                    kind: Kind::Launcher,
                    icons,
                });
            }
            let icons = mime.icons(mime.mime(&file_name));
            Some(Item {
                path,
                name: file_name,
                kind: Kind::File,
                icons,
            })
        })
        .collect();
    items.sort_by(|a, b| {
        (a.kind != Kind::Folder)
            .cmp(&(b.kind != Kind::Folder))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    items
}

/// Свободное имя в папке `dir`: `имя`, `имя 2`, `имя 3`… (у файла — перед
/// расширением: `фото 2.jpg`).
/// Свободное имя с учётом уже назначенных имён в пакетной операции.
/// Проверяем и битые символьные ссылки: Path::exists их пропускает.
pub fn free_name_reserved(dir: &Path, name: &str, reserved: &HashSet<PathBuf>) -> PathBuf {
    let available = |p: &Path| {
        !reserved.contains(p)
            && matches!(
                std::fs::symlink_metadata(p),
                Err(ref e) if e.kind() == std::io::ErrorKind::NotFound
            )
    };
    let first = dir.join(name);
    if available(&first) {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} {n}{ext}")))
        .find(|p| available(p))
        .expect("свободное имя найдётся")
}

pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    free_name_reserved(dir, name, &HashSet::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> MimeDb {
        let mut db = MimeDb::default();
        db.add_globs(
            "# comment\n50:image/png:*.png\n40:image/apng:*.png\n50:application/x-compressed-tar:*.tar.gz\n\
             50:application/gzip:*.gz\n50:text/plain:*.txt\n50:application/pdf:*.pdf\n",
        );
        db.add_generic("application/pdf:x-office-document\n");
        db
    }

    #[test]
    fn mime_and_icons() {
        let db = db();
        assert_eq!(
            db.mime("Фото.PNG"),
            Some("image/png"),
            "больший вес, регистр не важен"
        );
        assert_eq!(
            db.mime("архив.tar.gz"),
            Some("application/x-compressed-tar")
        );
        assert_eq!(db.mime("просто.gz"), Some("application/gzip"));
        assert_eq!(db.mime("без-расширения"), None);
        assert_eq!(
            db.icons(Some("application/pdf"))[..3],
            [
                "application-pdf",
                "x-office-document",
                "application-x-generic"
            ]
        );
        assert_eq!(db.icons(None), ["text-x-generic", "unknown"]);
    }

    #[test]
    fn scan_sorts_folders_first_and_reads_launchers() {
        let dir = std::env::temp_dir().join(format!("shell-desktop-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Проекты")).unwrap();
        std::fs::write(dir.join("заметка.txt"), "x").unwrap();
        std::fs::write(dir.join("Анкета.pdf"), "x").unwrap();
        std::fs::write(dir.join(".скрытый"), "x").unwrap();
        std::fs::write(
            dir.join("vlc.desktop"),
            "[Desktop Entry]\nName=VLC\nName[ru]=Медиаплеер\nIcon=vlc\nExec=vlc\n",
        )
        .unwrap();
        let ru = Locale::parse("ru_RU.UTF-8");
        let items = scan(&dir, &db(), ru.as_ref());
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            ["Проекты", "Анкета.pdf", "заметка.txt", "Медиаплеер"]
        );
        assert_eq!(items[0].kind, Kind::Folder);
        assert_eq!(items[3].icons[0], "vlc");
        assert_eq!(free_name(&dir, "Проекты"), dir.join("Проекты 2"));
        assert_eq!(free_name(&dir, "заметка.txt"), dir.join("заметка 2.txt"));
        assert_eq!(free_name(&dir, "новый.txt"), dir.join("новый.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn reserved_names_and_dangling_symlinks_are_occupied() {
        use std::os::unix::fs::symlink;

        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("shell-safe-names-{}-{id}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("документ.txt"), "старый").unwrap();
        let mut reserved = HashSet::new();
        let first = free_name_reserved(&dir, "документ.txt", &reserved);
        assert_eq!(first.file_name().unwrap(), "документ 2.txt");
        reserved.insert(first);
        let second = free_name_reserved(&dir, "документ.txt", &reserved);
        assert_eq!(second.file_name().unwrap(), "документ 3.txt");

        symlink(dir.join("нет-файла"), dir.join("битая ссылка")).unwrap();
        assert_eq!(free_name(&dir, "битая ссылка"), dir.join("битая ссылка 2"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
