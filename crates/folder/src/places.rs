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
        // Upstream default (Russian); app layer uses title_lang().
        self.title_lang(mimshel_core::i18n::Language::Ru)
    }

    pub fn title_lang(self, lang: mimshel_core::i18n::Language) -> &'static str {
        match self {
            Section::Places => mimshel_core::i18n::t(lang, "folder.places"),
            Section::Devices => mimshel_core::i18n::t(lang, "folder.devices"),
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
    /// gvfs location of a phone or camera (`mtp://…/`). Such a place with
    /// `removable == false` is not mounted yet: a click mounts it first.
    pub remote: Option<String>,
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
/// подключённые диски. Upstream default language is Russian; the app
/// layer calls `places_lang(config.language)` instead.
pub fn places() -> Vec<Place> {
    places_lang(mimshel_core::i18n::Language::Ru)
}

/// Same as [`places`], with sidebar names in `lang`.
pub fn places_lang(lang: mimshel_core::i18n::Language) -> Vec<Place> {
    let home = home();
    let mut list = from_user_dirs(&user_dirs_text(&home), &home);
    list.retain(|p| p.path.is_dir());
    // Корзина — после Рабочего стола, как в Thunar.
    let at = list.iter().position(|p| p.icon == "user-desktop-symbolic");
    list.insert(
        at.map_or(list.len(), |i| i + 1),
        Place {
            name: mimshel_core::i18n::t(lang, "dock.trash").to_string(),
            path: trash_dir(&home),
            icon: "user-trash-symbolic",
            section: Section::Places,
            removable: false,
            remote: None,
        },
    );
    list.push(Place {
        name: mimshel_core::i18n::t(lang, "folder.filesystem").to_string(),
        path: PathBuf::from("/"),
        icon: "drive-harddisk-symbolic",
        section: Section::Devices,
        removable: false,
        remote: None,
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
        .unwrap_or_else(|| {
            mimshel_core::i18n::t(mimshel_core::i18n::Language::Ru, "app.home").to_string()
        });
    let mut list = vec![Place {
        name,
        path: home.to_path_buf(),
        icon: "user-home-symbolic",
        section: Section::Places,
        removable: false,
        remote: None,
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
            remote: None,
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
                remote: None,
            }
        })
        .collect()
}

/// gvfs schemes of phones and cameras, with their sidebar icons.
const PHONE_SCHEMES: &[(&str, &str)] = &[
    ("mtp", "phone-symbolic"),
    ("afc", "phone-apple-iphone-symbolic"),
    ("gphoto2", "camera-photo-symbolic"),
];

/// Folder where gvfs shows its mounts as ordinary files (FUSE).
pub fn gvfs_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/run/user").join(uid()))
        .join("gvfs")
}

fn uid() -> String {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|v| v.split_whitespace().next())
        .unwrap_or("0")
        .to_string()
}

/// Connected phones and cameras (MTP, iPhone, PTP) as gvfs sees them.
/// Slow — `gio mount -li` takes about half a second, so call it from a
/// background thread.
pub fn phones() -> Vec<Place> {
    let Ok(out) = std::process::Command::new("gio")
        .args(["mount", "-li"])
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let gvfs = gvfs_dir();
    let entries: Vec<String> = std::fs::read_dir(&gvfs)
        .map(|dir| {
            dir.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let mut list = phones_from(&String::from_utf8_lossy(&out.stdout), &gvfs, &entries);
    // A phone usually has one storage («Internal shared storage») — open it
    // right away instead of a folder with a single folder inside.
    for p in list.iter_mut().filter(|p| p.removable) {
        if let Some(only) = single_subdir(&p.path) {
            p.path = only;
        }
    }
    list
}

fn single_subdir(dir: &Path) -> Option<PathBuf> {
    let mut entries = std::fs::read_dir(dir).ok()?.flatten();
    let first = entries.next()?;
    (entries.next().is_none() && first.file_type().ok()?.is_dir()).then(|| first.path())
}

/// Phones from the `gio mount -li` text. `entries` — names in the gvfs
/// FUSE folder `gvfs`: a mounted phone has its folder there
/// (`mtp:host=Xiaomi_…`).
fn phones_from(gio: &str, gvfs: &Path, entries: &[String]) -> Vec<Place> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut volume = String::new();
    for line in gio.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("Volume(") {
            volume = rest
                .split_once("): ")
                .map(|(_, n)| n.to_string())
                .unwrap_or_default();
            continue;
        }
        let entry = if let Some(uri) = line.strip_prefix("activation_root=") {
            Some((volume.clone(), uri.to_string()))
        } else if let Some(rest) = line.strip_prefix("Mount(") {
            // A mount without a volume (or one gvfs mounted by itself).
            rest.split_once("): ")
                .and_then(|(_, m)| m.split_once(" -> "))
                .map(|(n, u)| (n.to_string(), u.to_string()))
        } else {
            None
        };
        if let Some((name, uri)) = entry
            && location(&uri).is_some()
            && !found.iter().any(|(_, u)| *u == uri)
        {
            found.push((name, uri));
        }
    }
    found
        .into_iter()
        .filter_map(|(name, uri)| {
            let (scheme, host) = location(&uri)?;
            let icon = PHONE_SCHEMES.iter().find(|(s, _)| *s == scheme)?.1;
            let mounted = entries.iter().find(|e| {
                e.split_once(":host=").is_some_and(|(s, h)| {
                    s == scheme && percent_decode(h.split(',').next().unwrap_or(h)) == host
                })
            });
            let path = match mounted {
                Some(e) => gvfs.join(e),
                None => gvfs.join(format!("{scheme}:host={host}")),
            };
            Some(Place {
                name: if name.is_empty() { host } else { name },
                path,
                icon,
                section: Section::Devices,
                removable: mounted.is_some(),
                remote: Some(uri),
            })
        })
        .collect()
}

/// Scheme and host of a phone location: `mtp://Xiaomi_…/` → (`mtp`, `Xiaomi_…`).
fn location(uri: &str) -> Option<(&str, String)> {
    let (scheme, rest) = uri.split_once("://")?;
    PHONE_SCHEMES
        .iter()
        .any(|(s, _)| *s == scheme)
        .then_some(())?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some((scheme, percent_decode(host)))
}

/// `%5B` → `[` (gvfs escapes special characters in hosts).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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

    #[test]
    fn phones_from_gio() {
        let gio = "Drive(0): KINGSTON\n  Volume(0): Том 917 Гб\n    uuid=6C1E\n\
                   Volume(0): Xiaomi 13T Pro\n  Type: GProxyVolume (GProxyVolumeMonitorMTP)\n\
                   \x20 activation_root=mtp://Xiaomi_13T_ABC/\n\
                   \x20 Mount(0): Xiaomi 13T Pro -> mtp://Xiaomi_13T_ABC/\n\
                   Mount(1): mtp -> mtp://Xiaomi_13T_ABC/\n\
                   Volume(1): Canon\n  activation_root=gphoto2://%5Busb%3A001%2C007%5D/\n\
                   Mount(2): Общий -> smb://nas/share/\n";
        let gvfs = Path::new("/run/user/1000/gvfs");
        // Not mounted yet: a click mounts it.
        let list = phones_from(gio, gvfs, &[]);
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Xiaomi 13T Pro", "Canon"], "no network shares");
        assert!(!list[0].removable);
        assert_eq!(list[0].remote.as_deref(), Some("mtp://Xiaomi_13T_ABC/"));
        assert_eq!(list[0].icon, "phone-symbolic");
        assert_eq!(list[1].icon, "camera-photo-symbolic");
        // Mounted: its FUSE folder is there.
        let entries = [
            "mtp:host=Xiaomi_13T_ABC".to_string(),
            "gphoto2:host=%5Busb%3A001%2C007%5D".to_string(),
        ];
        let list = phones_from(gio, gvfs, &entries);
        assert!(list.iter().all(|p| p.removable));
        assert_eq!(list[0].path, gvfs.join("mtp:host=Xiaomi_13T_ABC"));
        assert_eq!(
            list[1].path,
            gvfs.join("gphoto2:host=%5Busb%3A001%2C007%5D")
        );
    }
}
