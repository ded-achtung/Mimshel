//! Чтение и изменение настроек. Блокировка отдельного файла охватывает
//! чтение, правку и атомарную замену: док и CLI не теряют изменения друг друга.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use shell_core::config::{self, Config};

pub struct Store {
    pub path: PathBuf,
    pub good: Option<PathBuf>,
}

impl Store {
    pub fn load(&self, legacy: Option<&Config>) -> Config {
        match self.load_locked(legacy) {
            Ok(config) => config,
            Err(e) => {
                eprintln!(
                    "Не удалось загрузить {}: {e}; беру последние исправные настройки",
                    self.path.display()
                );
                self.fallback(legacy)
            }
        }
    }

    fn load_locked(&self, legacy: Option<&Config>) -> io::Result<Config> {
        let _lock = lock(&self.path)?;
        let mut text = match fs::read_to_string(&self.path) {
            Ok(text) => {
                validate(&text)?;
                text
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let text = self.fallback(legacy).to_toml();
                atomic_write(&self.path, &text)?;
                text
            }
            Err(e) => return Err(e),
        };
        if !config::has_parts(&text)
            && let Some(legacy) = legacy
        {
            for (name, old, _) in config::PARTS {
                let value = if legacy.part(name) == Some(true) {
                    "shell"
                } else {
                    old
                };
                text = config::set_value(&text, "parts", name, (*value).into())
                    .ok_or_else(|| io::Error::other("не удалось перенести настройки частей"))?;
            }
            atomic_write(&self.path, &text)?;
        }
        self.remember(&text);
        Ok(Config::from_toml(&text))
    }

    fn fallback(&self, legacy: Option<&Config>) -> Config {
        if let Some(text) = self
            .good
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .filter(|text| validate(text).is_ok())
        {
            let mut config = Config::from_toml(&text);
            if !config::has_parts(&text)
                && let Some(legacy) = legacy
            {
                for (name, ..) in config::PARTS {
                    config.set_part(name, legacy.part(name) == Some(true));
                }
            }
            config
        } else {
            legacy.cloned().unwrap_or_default()
        }
    }

    /// Правка получает актуальный текст после получения межпроцессной блокировки.
    /// Нечитаемый или повреждённый файл не перезаписывается.
    pub fn edit(&self, edit: impl FnOnce(&str) -> io::Result<String>) -> io::Result<()> {
        let _lock = lock(&self.path)?;
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => self.fallback(None).to_toml(),
            Err(e) => return Err(e),
        };
        validate(&text)?;
        let next = edit(&text)?;
        validate(&next)?;
        if next != text || !self.path.exists() {
            atomic_write(&self.path, &next)?;
        }
        self.remember(&next);
        Ok(())
    }

    fn remember(&self, text: &str) {
        let Some(path) = &self.good else { return };
        let save = || -> io::Result<()> {
            let _lock = lock(path)?;
            if fs::read_to_string(path).ok().as_deref() != Some(text) {
                atomic_write(path, text)?;
            }
            Ok(())
        };
        if let Err(e) = save() {
            eprintln!(
                "Не удалось сохранить копию настроек {}: {e}",
                path.display()
            );
        }
    }
}

fn validate(text: &str) -> io::Result<()> {
    match config::toml_error(text) {
        Some(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        None => Ok(()),
    }
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn lock(path: &Path) -> io::Result<File> {
    fs::create_dir_all(parent(path))?;
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(PathBuf::from(name))?;
    file.lock()?;
    // Файл .lock нельзя удалять: другие процессы могут ждать на его inode.
    // При закрытии File ядро освобождает блокировку, в том числе после падения.
    Ok(file)
}

fn atomic_write(path: &Path, text: &str) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (tmp, mut file) = loop {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(
            ".{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let tmp = PathBuf::from(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
        {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        File::open(parent(path))?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "shell-config-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn store(&self) -> Store {
            Store {
                path: self.0.join("config.toml"),
                good: Some(self.0.join("good.toml")),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn read_errors_and_invalid_values_restore_good_copy_without_overwriting_source() {
        let f = Fixture::new();
        let store = f.store();
        let good = "[look]\nfont = \"Сохранённый\"\n[parts]\ndock = \"shell\"\n";
        fs::write(store.good.as_ref().unwrap(), good).unwrap();
        // Каталог вместо файла вызывает ошибку чтения и у root, в отличие от chmod 000.
        fs::create_dir(&store.path).unwrap();
        assert_eq!(store.load(None).font, "Сохранённый");
        assert!(store.path.is_dir());
        assert!(store.edit(|_| Ok(Config::default().to_toml())).is_err());
        fs::remove_dir(&store.path).unwrap();
        for bad in ["[dock]\nzoom = 9999", "[broken", "[parts]\ndock = \"shel\""] {
            fs::write(&store.path, bad).unwrap();
            assert!(store.load(None).dock);
            assert_eq!(fs::read_to_string(&store.path).unwrap(), bad);
            assert_eq!(
                fs::read_to_string(store.good.as_ref().unwrap()).unwrap(),
                good
            );
        }
    }

    #[test]
    fn missing_config_uses_backup_and_bad_backup_uses_legacy() {
        let f = Fixture::new();
        let store = f.store();
        fs::write(store.good.as_ref().unwrap(), "[look]\nfont = \"Резерв\"").unwrap();
        assert_eq!(store.load(None).font, "Резерв");
        assert_eq!(
            Config::from_toml(&fs::read_to_string(&store.path).unwrap()).font,
            "Резерв"
        );
        fs::write(&store.path, "bad [").unwrap();
        fs::write(store.good.as_ref().unwrap(), "bad [").unwrap();
        let legacy = Config {
            font: "Старый".into(),
            ..Config::default()
        };
        assert_eq!(store.load(Some(&legacy)).font, "Старый");
    }

    #[test]
    fn migration_and_edits_preserve_comments_and_backup() {
        let f = Fixture::new();
        let store = f.store();
        fs::write(&store.path, "# пояснение\n[dock]\napps = [\"one\"]\n").unwrap();
        let legacy = Config {
            dock: true,
            ..Config::default()
        };
        assert!(store.load(Some(&legacy)).dock);
        store
            .edit(|text| Ok(config::set_dock_apps(text, &["two".into()]).unwrap()))
            .unwrap();
        let text = fs::read_to_string(&store.path).unwrap();
        assert!(text.starts_with("# пояснение"));
        assert_eq!(Config::from_toml(&text).dock_apps, ["two"]);
        assert!(Config::from_toml(&text).dock);
        assert_eq!(fs::read_to_string(store.good.unwrap()).unwrap(), text);
    }

    #[test]
    fn process_edit_helper() {
        let Some(dir) = std::env::var_os("SHELL_TEST_EDIT_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let store = Store {
            path: dir.join("config.toml"),
            good: Some(dir.join("good.toml")),
        };
        for _ in 0..10 {
            store
                .edit(|text| {
                    let size = Config::from_toml(text).dock_icon;
                    std::thread::sleep(Duration::from_millis(2));
                    Ok(
                        config::set_value(text, "dock", "icon_size", i64::from(size + 1).into())
                            .unwrap(),
                    )
                })
                .unwrap();
        }
    }

    #[test]
    fn concurrent_processes_do_not_lose_updates() {
        let f = Fixture::new();
        let store = f.store();
        store.load(None);
        let mut children: Vec<_> = (0..4)
            .map(|_| {
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "config_store::tests::process_edit_helper"])
                    .env("SHELL_TEST_EDIT_DIR", &f.0)
                    .stdout(Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        assert_eq!(store.load(None).dock_icon, 88);
        assert_eq!(
            fs::read_to_string(&store.path).unwrap(),
            fs::read_to_string(store.good.unwrap()).unwrap()
        );
        assert!(
            !fs::read_dir(&f.0)
                .unwrap()
                .flatten()
                .any(|e| e.path().extension().is_some_and(|s| s == "tmp"))
        );
    }
}
