//! Места в боковой панели окна папки, как «Избранное» в Finder: домашняя
//! папка, папки пользователя по стандарту freedesktop (`user-dirs.dirs`:
//! `XDG_DESKTOP_DIR="$HOME/Рабочий стол"` и т. д.), корзина и весь диск.

use std::path::{Path, PathBuf};

/// Место в боковой панели.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
    /// Одноцветный значок темы.
    pub icon: &'static str,
}

/// Папки пользователя по порядку панели: (ключ в `user-dirs.dirs`,
/// название по умолчанию, значок).
const USER_DIRS: &[(&str, &str, &str)] = &[
    ("DESKTOP", "Рабочий стол", "user-desktop-symbolic"),
    ("DOCUMENTS", "Документы", "folder-documents-symbolic"),
    ("DOWNLOAD", "Загрузки", "folder-download-symbolic"),
    ("PICTURES", "Изображения", "folder-pictures-symbolic"),
    ("MUSIC", "Музыка", "folder-music-symbolic"),
    ("VIDEOS", "Видео", "folder-videos-symbolic"),
];

/// Папка с файлами корзины (freedesktop Trash).
pub fn trash_dir(home: &Path) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"))
        .join("Trash/files")
}

/// Места этого пользователя (несуществующие папки пропускаются).
pub fn places() -> Vec<Place> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    let mut list = from_user_dirs(&text, &home);
    list.retain(|p| p.path.is_dir());
    list.push(Place {
        name: "Корзина".into(),
        path: trash_dir(&home),
        icon: "user-trash-symbolic",
    });
    list.push(Place {
        name: "Компьютер".into(),
        path: PathBuf::from("/"),
        icon: "drive-harddisk-symbolic",
    });
    list
}

/// Домашняя папка и папки пользователя из текста `user-dirs.dirs`. Папка,
/// равная домашней (так отключают папку), не показывается.
fn from_user_dirs(text: &str, home: &Path) -> Vec<Place> {
    let name = home
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Домашняя папка".into());
    let mut list = vec![Place {
        name,
        path: home.to_path_buf(),
        icon: "user-home-symbolic",
    }];
    for (key, default, icon) in USER_DIRS {
        let var = format!("XDG_{key}_DIR=");
        let Some(value) = text
            .lines()
            .map(str::trim)
            .find_map(|l| l.strip_prefix(&var))
        else {
            continue;
        };
        let value = value.trim_matches('"');
        let path = match value.strip_prefix("$HOME") {
            Some(rest) => home.join(rest.trim_start_matches('/')),
            None => PathBuf::from(value),
        };
        if path == home || !path.is_absolute() {
            continue;
        }
        // Название — как у папки на диске (в русской системе — по-русски).
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| (*default).into());
        list.push(Place { name, path, icon });
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_user_dirs() {
        let text = "# комментарий\nXDG_DESKTOP_DIR=\"$HOME/Рабочий стол\"\n\
                    XDG_MUSIC_DIR=\"$HOME/\"\nXDG_DOWNLOAD_DIR=\"/data/Загрузки\"\n";
        let list = from_user_dirs(text, Path::new("/home/ded"));
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["ded", "Рабочий стол", "Загрузки"]);
        assert_eq!(list[1].path, Path::new("/home/ded/Рабочий стол"));
        assert_eq!(list[2].path, Path::new("/data/Загрузки"));
        assert_eq!(
            trash_dir(Path::new("/h")).file_name().unwrap(),
            "files",
            "файлы корзины — в Trash/files"
        );
    }
}
