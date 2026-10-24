//! Управление ссылками GTK4: не заменяем и не удаляем пользовательские файлы.
//! Манифест запоминает имя и точную цель только для ссылок, созданных Shell.
//! Ссылки старых установок без манифеста оставляем нетронутыми.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::sys::home;

const NAMES: [&str; 3] = ["gtk.css", "gtk-dark.css", "assets"];

fn manifest_path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".local/state"));
    state.join("shell/gtk4-links")
}

fn read_entries(manifest: &Path) -> BTreeMap<&'static str, PathBuf> {
    let mut entries = BTreeMap::new();
    if let Ok(content) = std::fs::read_to_string(manifest) {
        for line in content.lines() {
            if let Some((name, path)) = line.split_once('\t')
                && let Some(&known) = NAMES.iter().find(|&&n| n == name)
                && Path::new(path).is_absolute()
            {
                entries.insert(known, PathBuf::from(path));
            }
        }
    }
    entries
}

fn save_entries(manifest: &Path, entries: &BTreeMap<&str, PathBuf>) -> io::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    if let Some(parent) = manifest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = entries
        .iter()
        .map(|(name, target)| format!("{name}\t{}\n", target.display()))
        .collect::<String>();
    let tmp = manifest.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, manifest)
}

fn apply_in(theme_dir: &Path, gtk4: &Path, manifest: &Path) -> io::Result<()> {
    if gtk4.is_symlink() {
        eprintln!(
            "GTK4: {} — чужая ссылка на каталог, не трогаю",
            gtk4.display()
        );
        return Ok(());
    }
    let mut owned = read_entries(manifest);
    for name in NAMES {
        let target = theme_dir.join("gtk-4.0").join(name);
        if !target.exists() {
            continue;
        }
        let link = gtk4.join(name);
        let actual = std::fs::read_link(&link).ok();
        // Ссылки прежних версий Shell (до списка) ведут в файл темы
        // `…/gtk-4.0/<имя>` — признаём их своими, иначе смена темы окон
        // на уже установленных системах не дошла бы до программ на GTK4.
        let ours = actual
            .as_ref()
            .is_some_and(|a| owned.get(name) == Some(a) || looks_like_theme_link(a, name));
        if actual.as_deref() == Some(target.as_path()) {
            if ours {
                owned.insert(name, target);
            }
            continue;
        }
        match std::fs::symlink_metadata(&link) {
            Ok(_) if !ours => {
                eprintln!(
                    "GTK4: {} уже существует и не принадлежит Shell",
                    link.display()
                );
                continue;
            }
            Ok(_) => std::fs::remove_file(&link)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        std::fs::create_dir_all(gtk4)?;
        std::os::unix::fs::symlink(&target, &link)?;
        owned.insert(name, target);
    }
    save_entries(manifest, &owned)
}

/// Ссылка ведёт в файл темы с тем же именем: `…/gtk-4.0/gtk.css` и т. п.
fn looks_like_theme_link(target: &Path, name: &str) -> bool {
    target.file_name().is_some_and(|f| f == name)
        && target
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|d| d == "gtk-4.0")
}

fn owned_links_in(gtk4: &Path, manifest: &Path) -> Vec<PathBuf> {
    read_entries(manifest)
        .into_iter()
        .filter_map(|(name, target)| {
            let link = gtk4.join(name);
            (std::fs::read_link(&link).ok().as_deref() == Some(target.as_path())).then_some(link)
        })
        .collect()
}

fn cleanup_in(gtk4: &Path, manifest: &Path) -> io::Result<()> {
    for link in owned_links_in(gtk4, manifest) {
        std::fs::remove_file(link)?;
    }
    match std::fs::remove_file(manifest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Применить новую тему, сохранив чужие файлы и ссылки.
pub fn apply(theme_dir: &Path) {
    let gtk4 = home().join(".config/gtk-4.0");
    if let Err(e) = apply_in(theme_dir, &gtk4, &manifest_path()) {
        eprintln!("GTK4: не удалось применить тему: {e}");
    }
}

/// Ссылки, созданные Shell и до сих пор указывающие на назначенные файлы.
pub fn owned_links() -> Vec<PathBuf> {
    owned_links_in(&home().join(".config/gtk-4.0"), &manifest_path())
}

/// Удалить только ссылки, оставшиеся нашими; директорию GTK4 не трогать.
pub fn cleanup() -> io::Result<()> {
    cleanup_in(&home().join(".config/gtk-4.0"), &manifest_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_replace_user_files_and_restores_only_owned_links() {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("shell-gtk-links-{}-{id}", std::process::id()));
        let gtk4 = root.join("config/gtk-4.0");
        let manifest = root.join("state/shell/gtk4-links");
        let theme1 = root.join("theme1");
        let theme2 = root.join("theme2");
        for theme in [&theme1, &theme2] {
            let t = theme.join("gtk-4.0");
            std::fs::create_dir_all(&t).unwrap();
            for name in NAMES {
                std::fs::write(t.join(name), theme.display().to_string()).unwrap();
            }
        }
        std::fs::create_dir_all(&gtk4).unwrap();
        std::fs::write(gtk4.join("gtk-dark.css"), "user").unwrap();
        let foreign = root.join("foreign");
        std::fs::write(&foreign, "foreign").unwrap();
        std::os::unix::fs::symlink(&foreign, gtk4.join("assets")).unwrap();

        apply_in(&theme1, &gtk4, &manifest).unwrap();
        assert_eq!(
            std::fs::read_link(gtk4.join("gtk.css")).unwrap(),
            theme1.join("gtk-4.0/gtk.css")
        );
        assert_eq!(
            std::fs::read_to_string(gtk4.join("gtk-dark.css")).unwrap(),
            "user"
        );
        assert_eq!(std::fs::read_link(gtk4.join("assets")).unwrap(), foreign);

        apply_in(&theme2, &gtk4, &manifest).unwrap();
        assert_eq!(
            std::fs::read_link(gtk4.join("gtk.css")).unwrap(),
            theme2.join("gtk-4.0/gtk.css")
        );
        cleanup_in(&gtk4, &manifest).unwrap();
        assert!(!gtk4.join("gtk.css").exists());
        assert_eq!(
            std::fs::read_to_string(gtk4.join("gtk-dark.css")).unwrap(),
            "user"
        );
        assert_eq!(std::fs::read_link(gtk4.join("assets")).unwrap(), foreign);
        assert!(gtk4.is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn links_of_older_shell_are_adopted() {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("shell-gtk-adopt-{}-{id}", std::process::id()));
        let gtk4 = root.join("gtk4");
        let manifest = root.join("links");
        let (old, new) = (root.join("Old"), root.join("New"));
        for theme in [&old, &new] {
            std::fs::create_dir_all(theme.join("gtk-4.0")).unwrap();
            std::fs::write(theme.join("gtk-4.0/gtk.css"), "x").unwrap();
        }
        // Так ссылку делала прежняя версия — без списка.
        std::fs::create_dir_all(&gtk4).unwrap();
        std::os::unix::fs::symlink(old.join("gtk-4.0/gtk.css"), gtk4.join("gtk.css")).unwrap();
        apply_in(&new, &gtk4, &manifest).unwrap();
        assert_eq!(
            std::fs::read_link(gtk4.join("gtk.css")).unwrap(),
            new.join("gtk-4.0/gtk.css"),
            "тема сменилась"
        );
        assert_eq!(owned_links_in(&gtk4, &manifest).len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changed_owned_link_is_preserved_during_cleanup() {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("shell-gtk-restore-{}-{id}", std::process::id()));
        let gtk4 = root.join("gtk4");
        let marker = root.join("links");
        let theme = root.join("theme");
        std::fs::create_dir_all(theme.join("gtk-4.0")).unwrap();
        std::fs::write(theme.join("gtk-4.0/gtk.css"), "theme").unwrap();
        apply_in(&theme, &gtk4, &marker).unwrap();
        std::fs::remove_file(gtk4.join("gtk.css")).unwrap();
        std::fs::write(gtk4.join("gtk.css"), "user").unwrap();
        cleanup_in(&gtk4, &marker).unwrap();
        assert_eq!(
            std::fs::read_to_string(gtk4.join("gtk.css")).unwrap(),
            "user"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
