//! Список программ: обход папок `applications`, выбор ярлыка по его
//! идентификатору и отбор тех, что нужно показывать.

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::entry::{DesktopEntry, Locale};
use crate::exec;

/// Одна программа, готовая к показу и запуску.
#[derive(Debug, Clone, PartialEq)]
pub struct App {
    /// Идентификатор по стандарту: путь внутри `applications` с `-` вместо `/`,
    /// например `org.xfce.mousepad.desktop`.
    pub id: String,
    /// Откуда взят ярлык.
    pub path: PathBuf,
    /// Название на языке пользователя.
    pub name: String,
    /// Название без перевода (обычно английское) — чтобы находилось и по нему.
    pub name_untranslated: String,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    /// Слова для поиска: переведённые и исходные вместе.
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    /// Имя иконки в теме или путь к файлу.
    pub icon: Option<String>,
    /// Команда запуска, уже разобранная на аргументы.
    pub command: Vec<String>,
    /// Рабочая папка (`Path=`).
    pub working_dir: Option<PathBuf>,
    /// Запускать в терминале.
    pub terminal: bool,
    /// Класс окна программы — пригодится доку, чтобы узнавать её окна.
    pub startup_wm_class: Option<String>,
    /// `NoDisplay=true`: не показывать в Spotlight и Launchpad. Доку такие
    /// ярлыки всё равно нужны (значок Launchpad, окна служебных программ).
    pub no_display: bool,
}

/// Где искать ярлыки и для кого.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Папки `applications` в порядке важности: первая перекрывает остальные.
    pub dirs: Vec<PathBuf>,
    pub locale: Option<Locale>,
    /// Имена рабочего стола из XDG_CURRENT_DESKTOP (для OnlyShowIn/NotShowIn).
    pub desktops: Vec<String>,
    /// Папки, где ищутся программы из `TryExec`.
    pub path: Vec<PathBuf>,
}

impl Settings {
    /// Всё из переменных окружения сеанса (XDG_DATA_HOME, XDG_DATA_DIRS и т. д.).
    pub fn from_env() -> Settings {
        let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());

        let mut data = Vec::new();
        match var("XDG_DATA_HOME") {
            Some(d) => data.push(PathBuf::from(d)),
            None => {
                if let Some(home) = var("HOME") {
                    data.push(Path::new(&home).join(".local/share"));
                }
            }
        }
        let system = var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        data.extend(
            system
                .split(':')
                .filter(|d| !d.is_empty())
                .map(PathBuf::from),
        );

        Settings {
            dirs: data.into_iter().map(|d| d.join("applications")).collect(),
            locale: Locale::from_env(),
            desktops: var("XDG_CURRENT_DESKTOP")
                .map(|d| d.split(':').map(String::from).collect())
                .unwrap_or_default(),
            path: var("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
        }
    }
}

/// Читает все ярлыки и возвращает видимые программы, по алфавиту.
pub fn load(settings: &Settings) -> Vec<App> {
    let mut apps = load_all(settings);
    apps.retain(|a| !a.no_display);
    apps
}

/// То же, но вместе с ярлыками `NoDisplay=true` (для дока).
pub fn load_all(settings: &Settings) -> Vec<App> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();

    for dir in &settings.dirs {
        let mut files = Vec::new();
        collect_files(dir, dir, &mut files);
        files.sort();
        for (id, path) in files {
            // Ярлык с тем же идентификатором из более важной папки уже решил судьбу
            // программы — даже если он её скрыл.
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(app) = make_app(id, path, &DesktopEntry::parse(&text), settings) {
                apps.push(app);
            }
        }
    }

    apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.id.clone()));
    apps
}

/// Все `*.desktop` в папке и вложенных папках вместе с идентификаторами.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for item in read.flatten() {
        let path = item.path();
        // metadata (а не file_type) — чтобы проходить и по символическим ссылкам.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            collect_files(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "desktop")
            && let Ok(rel) = path.strip_prefix(root)
        {
            let id = rel.to_string_lossy().replace('/', "-");
            out.push((id, path));
        }
    }
}

fn make_app(id: String, path: PathBuf, e: &DesktopEntry, s: &Settings) -> Option<App> {
    if e.string("Type").as_deref() != Some("Application") || e.boolean("Hidden") {
        return None;
    }

    let only = e.list("OnlyShowIn");
    if !only.is_empty() && !only.iter().any(|d| s.desktops.contains(d)) {
        return None;
    }
    if e.list("NotShowIn").iter().any(|d| s.desktops.contains(d)) {
        return None;
    }
    if let Some(try_exec) = e.string("TryExec")
        && !find_executable(&try_exec, &s.path)
    {
        return None;
    }

    let locale = s.locale.as_ref();
    let name = e.localized("Name", locale)?;
    let icon = e.string("Icon").filter(|i| !i.is_empty());
    let file = path.to_string_lossy();
    let ctx = exec::Context {
        name: &name,
        icon: icon.as_deref(),
        file: &file,
    };
    let command = exec::parse(&e.string("Exec")?, &ctx).ok()?;

    let mut keywords = e.localized_list("Keywords", locale);
    for k in e.list("Keywords") {
        if !keywords.contains(&k) {
            keywords.push(k);
        }
    }

    Some(App {
        name_untranslated: e.string("Name").unwrap_or_else(|| name.clone()),
        name,
        generic_name: e.localized("GenericName", locale),
        comment: e.localized("Comment", locale),
        keywords,
        categories: e.list("Categories"),
        icon,
        command,
        working_dir: e
            .string("Path")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
        terminal: e.boolean("Terminal"),
        startup_wm_class: e.string("StartupWMClass"),
        no_display: e.boolean("NoDisplay"),
        id,
        path,
    })
}

/// Есть ли такая программа: полный путь проверяем сразу, короткое имя ищем в PATH.
fn find_executable(name: &str, path: &[PathBuf]) -> bool {
    let is_exec = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if name.contains('/') {
        return is_exec(Path::new(name));
    }
    path.iter().any(|dir| is_exec(&dir.join(name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn app(name: &str, extra: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName={name}\nExec={name} %U\n{extra}\n")
    }

    fn settings(dirs: &[&TempDir]) -> Settings {
        Settings {
            dirs: dirs.iter().map(|d| d.0.clone()).collect(),
            locale: Locale::parse("ru_RU.UTF-8"),
            desktops: vec!["XFCE".into()],
            path: vec!["/usr/bin".into(), "/bin".into()],
        }
    }

    fn names(apps: &[App]) -> Vec<&str> {
        apps.iter().map(|a| a.name.as_str()).collect()
    }

    #[test]
    fn user_overrides_system() {
        let user = TempDir::new("user");
        let system = TempDir::new("system");
        system.write("editor.desktop", &app("Editor", ""));
        system.write("plank.desktop", &app("Plank", ""));
        system.write("files.desktop", &app("Files", ""));
        // Пользователь переименовал редактор и спрятал Plank — как делает macos-look.
        user.write("editor.desktop", &app("Мой редактор", ""));
        user.write("plank.desktop", &app("Plank", "NoDisplay=true"));

        let apps = load(&settings(&[&user, &system]));
        assert_eq!(names(&apps), ["Files", "Мой редактор"]);
        // Для дока скрытый Plank есть, но с пометкой.
        let all = load_all(&settings(&[&user, &system]));
        assert_eq!(names(&all), ["Files", "Plank", "Мой редактор"]);
        assert!(all[1].no_display);
        assert_eq!(apps[1].path, user.0.join("editor.desktop"));
        assert_eq!(apps[0].command, ["Files"]);
    }

    #[test]
    fn subfolder_id() {
        let dir = TempDir::new("sub");
        dir.write("kde/konsole.desktop", &app("Konsole", ""));
        let apps = load(&settings(&[&dir]));
        assert_eq!(apps[0].id, "kde-konsole.desktop");
    }

    #[test]
    fn filters() {
        let dir = TempDir::new("filters");
        dir.write("a.desktop", &app("Visible", ""));
        dir.write("b.desktop", &app("Hidden", "Hidden=true"));
        dir.write("c.desktop", &app("Gnome only", "OnlyShowIn=GNOME;KDE;"));
        dir.write("d.desktop", &app("Xfce only", "OnlyShowIn=XFCE;"));
        dir.write("e.desktop", &app("Not xfce", "NotShowIn=XFCE;"));
        dir.write("f.desktop", &app("Missing", "TryExec=no-such-program-xyz"));
        dir.write("g.desktop", &app("Present", "TryExec=sh"));
        dir.write(
            "h.desktop",
            "[Desktop Entry]\nType=Link\nName=Link\nURL=x\n",
        );
        dir.write(
            "i.desktop",
            "[Desktop Entry]\nType=Application\nName=No exec\n",
        );
        dir.write("readme.txt", "not a desktop file");

        let apps = load(&settings(&[&dir]));
        assert_eq!(names(&apps), ["Present", "Visible", "Xfce only"]);
    }

    #[test]
    fn translated_fields() {
        let dir = TempDir::new("ru");
        dir.write(
            "t.desktop",
            "[Desktop Entry]\nType=Application\nName=Terminal\nName[ru]=Терминал\n\
             Exec=xterm\nKeywords=shell;prompt;\nKeywords[ru]=оболочка;shell;\nIcon=utilities-terminal\n",
        );
        let a = &load(&settings(&[&dir]))[0];
        assert_eq!(a.name, "Терминал");
        assert_eq!(a.name_untranslated, "Terminal");
        assert_eq!(a.keywords, ["оболочка", "shell", "prompt"]);
        assert_eq!(a.icon.as_deref(), Some("utilities-terminal"));
    }
}
