//! Значки дока и чьё окно чьё.
//!
//! Урок Plank (CLAUDE.md, урок 6): связывать окно с ярлыком по пути .desktop
//! ненадёжно — копия ярлыка в другой папке давала дубли. Здесь окно
//! сопоставляется с программой по её идентификатору и признакам самого окна.

use std::path::PathBuf;

use mimshel_apps::App;
use mimshel_core::config::dock_entry_matches;
use mimshel_core::windows::WindowInfo;

/// Ключ значка корзины (в конце дока, за разделителем).
pub const TRASH: &str = "trash:";

/// Корзина как «программа»: имя и иконка (пустая или полная). Запускать её
/// нечего — открывает и очищает её служба (через Проводник).
#[allow(dead_code)]
pub(crate) fn trash_app(full: bool) -> App {
    trash_app_lang(full, mimshel_core::i18n::Language::Ru)
}

/// Trash as an app, with its display name in `lang`.
pub(crate) fn trash_app_lang(full: bool, lang: mimshel_core::i18n::Language) -> App {
    App {
        id: TRASH.into(),
        path: PathBuf::new(),
        name: mimshel_core::i18n::t(lang, "dock.trash").to_string(),
        name_untranslated: "Trash".into(),
        generic_name: None,
        comment: None,
        keywords: vec![],
        categories: vec![],
        icon: Some(
            if full {
                "user-trash-full"
            } else {
                "user-trash"
            }
            .into(),
        ),
        command: vec![],
        working_dir: None,
        terminal: false,
        startup_wm_class: None,
        no_display: false,
        prefers_dgpu: false,
    }
}

/// Значок в доке: программа (закреплённая или запущенная) либо окно без ярлыка.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Идентификатор ярлыка (`firefox.desktop`) или `window:<номер>`.
    pub key: String,
    pub app: Option<App>,
    pub name: String,
    pub pinned: bool,
    /// Окна программы, снизу вверх.
    pub windows: Vec<u32>,
    /// Перед этим значком — разделитель (между закреплёнными и остальными).
    pub separator_before: bool,
}

/// Программа, которой принадлежит окно (номер в `apps`), от надёжного признака
/// к запасному:
/// 1. ярлык, из которого запущен процесс окна (`hint`, имя файла);
/// 2. `StartupWMClass` ярлыка = класс окна;
/// 3. имя ярлыка = класс окна (`org.gnome.Meld` ↔ `Meld` — по последней части);
/// 4. имя команды = класс окна.
///
/// Регистр не важен. Среди равных первым идёт видимый в меню ярлык.
pub fn match_window(w: &WindowInfo, hint: Option<&str>, apps: &[App]) -> Option<usize> {
    let same = |a: &str, b: &str| !a.is_empty() && a.eq_ignore_ascii_case(b);
    let window_names = [w.instance.as_str(), w.class.as_str()];
    let names_match = |name: &str| window_names.iter().any(|n| same(n, name));

    let stem = |a: &App| a.id.trim_end_matches(".desktop").to_string();
    let program = |a: &App| {
        a.command
            .first()
            .map(|c| c.rsplit('/').next().unwrap_or(c).to_string())
            .unwrap_or_default()
    };

    let tiers: [&dyn Fn(&App) -> bool; 4] = [
        &|a| {
            hint.is_some_and(|h| {
                let file = h.rsplit('/').next().unwrap_or(h);
                file == a.id
            })
        },
        &|a| a.startup_wm_class.as_deref().is_some_and(names_match),
        &|a| {
            let s = stem(a);
            names_match(&s) || names_match(s.rsplit('.').next().unwrap_or(&s))
        },
        &|a| names_match(&program(a)),
    ];
    for tier in tiers {
        let mut found: Vec<usize> = (0..apps.len()).filter(|&i| tier(&apps[i])).collect();
        found.sort_by_key(|&i| apps[i].no_display);
        if let Some(&i) = found.first() {
            return Some(i);
        }
    }
    None
}

/// Значки: закреплённые по порядку настроек, затем остальные запущенные
/// программы и окна без ярлыка — в порядке появления окон; в конце корзина
/// (`trash` — полна ли она; `None` — без корзины).
pub(crate) fn build(
    apps: &[App],
    pinned: &[String],
    windows: &[(WindowInfo, Option<usize>)],
    trash: Option<bool>,
    lang: mimshel_core::i18n::Language,
) -> Vec<Item> {
    let mut items: Vec<Item> = pinned
        .iter()
        .filter_map(|entry| apps.iter().find(|a| dock_entry_matches(entry, &a.id)))
        .map(|app| Item {
            key: app.id.clone(),
            app: Some(app.clone()),
            name: app.name.clone(),
            pinned: true,
            windows: Vec::new(),
            separator_before: false,
        })
        .collect();

    // Окна в порядке появления: номера окон X растут со временем создания.
    let mut by_age: Vec<&(WindowInfo, Option<usize>)> = windows.iter().collect();
    by_age.sort_by_key(|(w, _)| w.id);
    for (w, app) in by_age {
        let key = match app {
            Some(i) => apps[*i].id.clone(),
            None => format!("window:{}", w.id),
        };
        if !items.iter().any(|it| it.key == key) {
            items.push(Item {
                key: key.clone(),
                app: app.map(|i| apps[i].clone()),
                name: match app {
                    Some(i) => apps[*i].name.clone(),
                    None if !w.title.is_empty() => w.title.clone(),
                    None => w.class.clone(),
                },
                pinned: false,
                windows: Vec::new(),
                separator_before: false,
            });
        }
    }
    // Окна раскладываем по значкам в порядке стопки (снизу вверх).
    for (w, app) in windows {
        let key = match app {
            Some(i) => apps[*i].id.clone(),
            None => format!("window:{}", w.id),
        };
        if let Some(item) = items.iter_mut().find(|it| it.key == key) {
            item.windows.push(w.id);
        }
    }
    if let Some(full) = trash {
        let app = trash_app_lang(full, lang);
        items.push(Item {
            key: TRASH.into(),
            name: app.name.clone(),
            app: Some(app),
            pinned: false,
            windows: Vec::new(),
            separator_before: false,
        });
    }
    if let Some(first) = items.iter().position(|it| !it.pinned)
        && first > 0
    {
        items[first].separator_before = true;
    }
    let n = items.len();
    if n > 1
        && let Some(last) = items.last_mut()
        && last.key == TRASH
    {
        last.separator_before = true;
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{app, window};

    fn apps() -> Vec<App> {
        let mut meld = app("org.gnome.Meld", "Meld");
        meld.command = vec!["/usr/bin/meld".into()];
        let mut code = app("code-oss", "Code");
        code.startup_wm_class = Some("code-oss-dev".into());
        let mut hidden = app("xfce4-terminal-emulator", "Терминал");
        hidden.no_display = true;
        hidden.command = vec!["xfce4-terminal".into()];
        vec![
            hidden,
            app("xfce4-terminal", "Терминал"),
            meld,
            code,
            app("thunar", "Проводник"),
        ]
    }

    fn found(w: &WindowInfo, hint: Option<&str>) -> Option<String> {
        let apps = apps();
        match_window(w, hint, &apps).map(|i| apps[i].id.clone())
    }

    #[test]
    fn tiers() {
        // Подсказка из окружения процесса важнее всего — даже если класс другой.
        let w = window(1, "xfce4-terminal", "Xfce4-terminal");
        assert_eq!(
            found(
                &w,
                Some("/home/ded/.local/share/applications/thunar.desktop")
            )
            .as_deref(),
            Some("thunar.desktop")
        );
        assert_eq!(found(&w, None).as_deref(), Some("xfce4-terminal.desktop"));
        assert_eq!(
            found(&window(2, "code-oss-dev", "Code-oss-dev"), None).as_deref(),
            Some("code-oss.desktop")
        );
        assert_eq!(
            found(&window(3, "meld", "Meld"), None).as_deref(),
            Some("org.gnome.Meld.desktop")
        );
        assert_eq!(found(&window(4, "xeyes", "XEyes"), None), None);
    }

    #[test]
    fn visible_shortcut_preferred() {
        // По команде подходят оба «Терминала» — берётся видимый в меню.
        let mut w = window(5, "term-x", "Term-x");
        w.instance = "xfce4-terminal".into();
        assert_eq!(found(&w, None).as_deref(), Some("xfce4-terminal.desktop"));
    }

    #[test]
    fn separator_and_grouping() {
        let apps = apps();
        let windows = vec![
            (window(30, "meld", "Meld"), Some(2)),
            (window(10, "thunar", "Thunar"), Some(4)),
            (window(31, "meld", "Meld"), Some(2)),
        ];
        let items = build(
            &apps,
            &["thunar".into(), "nope".into()],
            &windows,
            None,
            mimshel_core::i18n::Language::Ru,
        );
        let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["thunar.desktop", "org.gnome.Meld.desktop"]);
        assert!(items[1].separator_before && !items[0].separator_before);
        assert_eq!(items[1].windows, [30, 31]);
        assert_eq!(items[0].windows, [10]);
    }
}
