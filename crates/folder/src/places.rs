//! Места в боковой панели окна папки, как в Thunar: «Места» — домашняя
//! папка, папки пользователя по стандарту freedesktop (`user-dirs.dirs`:
//! `XDG_DESKTOP_DIR="$HOME/Рабочий стол"` и т. д.) и корзина; «Устройства» —
//! файловая система и подключённые диски и флешки (из `/proc/self/mounts`).
//! Особые папки (Документы, Загрузки…) и в сетке значков — своими значками.

use std::path::{Path, PathBuf};

/// Раздел боковой панели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Places,
    Devices,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Places => "Места",
            Section::Devices => "Устройства",
        }
    }
}

/// Место в боковой панели.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
    /// Одноцветный значок темы.
    pub icon: &'static str,
    pub section: Section,
    /// Подключённый диск, который можно извлечь (кнопка ⏏).
    pub removable: bool,
}

/// Папки пользователя: (ключ в `user-dirs.dirs`, значок в боковой панели,
/// значок папки в сетке). Порядок — как в боковой панели Thunar.
const USER_DIRS: &[(&str, &str, &str)] = &[
    ("DESKTOP", "user-desktop-symbolic", "folder-desktop"),
    ("DOCUMENTS", "folder-documents-symbolic", "folder-documents"),
    ("MUSIC", "folder-music-symbolic", "folder-music"),
    ("PICTURES", "folder-pictures-symbolic", "folder-pictures"),
    ("VIDEOS", "folder-videos-symbolic", "folder-videos"),
    ("DOWNLOAD", "folder-download-symbolic", "folder-download"),
    ("PUBLICSHARE", "", "folder-publicshare"),
    ("TEMPLATES", "", "folder-templates"),
    ("PROJECTS", "", "folder-projects"),
];

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn user_dirs_text(home: &Path) -> String {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    std::fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default()
}

/// Папка с файлами корзины (freedesktop Trash).
pub fn trash_dir(home: &Path) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"))
        .join("Trash/files")
}

/// Места этого пользователя (несуществующие папки пропускаются) и
/// подключённые диски.
pub fn places() -> Vec<Place> {
    let home = home();
    let mut list = from_user_dirs(&user_dirs_text(&home), &home);
    list.retain(|p| p.path.is_dir());
    // Корзина — после Рабочего стола, как в Thunar.
    let at = list.iter().position(|p| p.icon == "user-desktop-symbolic");
    list.insert(
        at.map_or(list.len(), |i| i + 1),
        Place {
            name: "Корзина".into(),
            path: trash_dir(&home),
            icon: "user-trash-symbolic",
            section: Section::Places,
            removable: false,
        },
    );
    list.push(Place {
        name: "Файловая система".into(),
        path: PathBuf::from("/"),
        icon: "drive-harddisk-symbolic",
        section: Section::Devices,
        removable: false,
    });
    let user = home
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    list.extend(removable(&mounts, &user));
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
        section: Section::Places,
        removable: false,
    }];
    for (path, side, _) in user_dirs(text, home) {
        if side.is_empty() {
            continue;
        }
        // Название — как у папки на диске (в русской системе — по-русски).
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        list.push(Place {
            name,
            path,
            icon: side,
            section: Section::Places,
            removable: false,
        });
    }
    list
}

/// Папки пользователя: (путь, значок боковой панели, значок в сетке).
fn user_dirs(text: &str, home: &Path) -> Vec<(PathBuf, &'static str, &'static str)> {
    USER_DIRS
        .iter()
        .filter_map(|&(key, side, grid)| {
            let var = format!("XDG_{key}_DIR=");
            let value = text
                .lines()
                .map(str::trim)
                .find_map(|l| l.strip_prefix(&var))?
                .trim_matches('"');
            let path = match value.strip_prefix("$HOME") {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None => PathBuf::from(value),
            };
            (path != home && path.is_absolute()).then_some((path, side, grid))
        })
        .collect()
}

/// Особые папки и их значки в сетке (Документы — папка с листком и т. д.).
pub fn special_folders() -> Vec<(PathBuf, &'static str)> {
    let home = home();
    let mut list: Vec<(PathBuf, &'static str)> = user_dirs(&user_dirs_text(&home), &home)
        .into_iter()
        .map(|(path, _, grid)| (path, grid))
        .collect();
    list.push((home, "user-home"));
    list
}

/// Подключённые диски и флешки: точки подключения в `/run/media/ПОЛЬЗОВАТЕЛЬ`,
/// `/media` и `/mnt`.
fn removable(mounts: &str, user: &str) -> Vec<Place> {
    let run_media = format!("/run/media/{user}/");
    mounts
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(unescape)
        .filter(|m| m.starts_with(&run_media) || m.starts_with("/media/") || m.starts_with("/mnt/"))
        .map(|m| {
            let path = PathBuf::from(&m);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(m);
            Place {
                name,
                path,
                icon: "drive-removable-media-symbolic",
                section: Section::Devices,
                removable: true,
            }
        })
        .collect()
}

/// В `/proc/mounts` пробел и другие знаки записаны как `\040`.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 4], 8)
        {
            out.push(v);
            i += 4;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_user_dirs() {
        let text = "# комментарий\nXDG_DESKTOP_DIR=\"$HOME/Рабочий стол\"\n\
                    XDG_MUSIC_DIR=\"$HOME/\"\nXDG_DOWNLOAD_DIR=\"/data/Загрузки\"\n\
                    XDG_TEMPLATES_DIR=\"$HOME/Шаблоны\"\n";
        let list = from_user_dirs(text, Path::new("/home/ded"));
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["ded", "Рабочий стол", "Загрузки"],
            "шаблонов в панели нет"
        );
        assert_eq!(list[1].path, Path::new("/home/ded/Рабочий стол"));
        assert_eq!(list[2].path, Path::new("/data/Загрузки"));
        let special = user_dirs(text, Path::new("/home/ded"));
        assert!(
            special
                .iter()
                .any(|(p, _, g)| p == Path::new("/home/ded/Шаблоны") && *g == "folder-templates")
        );
        assert_eq!(trash_dir(Path::new("/h")).file_name().unwrap(), "files");
    }

    #[test]
    fn removable_mounts() {
        let mounts = "/dev/sda2 / ext4 rw 0 0\n\
                      /dev/sr0 /run/media/ded/EOS_202608 iso9660 ro 0 0\n\
                      /dev/sdb1 /media/Моя\\040флешка vfat rw 0 0\n\
                      tmpfs /run/user/1000 tmpfs rw 0 0\n";
        let list = removable(mounts, "ded");
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["EOS_202608", "Моя флешка"]);
        assert!(
            list.iter()
                .all(|p| p.removable && p.section == Section::Devices)
        );
    }
}
