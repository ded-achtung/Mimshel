//! Меню яблока. Пункты описывает macos-look: файл меню по стандарту
//! freedesktop (`apple.menu`: папка ярлыков, порядок и разделители) и ярлыки
//! `.desktop` с названием и командой. Читаем их, чтобы пункты были в одном
//! месте — в macos-look.

use std::path::{Path, PathBuf};

use shell_apps::Locale;
use shell_apps::entry::DesktopEntry;
use shell_apps::exec::{self, Context};

/// Пункт меню яблока.
#[derive(Debug, Clone, PartialEq)]
pub struct AppleItem {
    pub label: String,
    /// Команда, уже разобранная на аргументы.
    pub command: Vec<String>,
    /// Перед пунктом — разделитель.
    pub separator_before: bool,
}

/// Пункты меню из файла `menu_file`. Нет файла или пунктов — пусто.
pub fn apple_menu(menu_file: &Path) -> Vec<AppleItem> {
    let Ok(xml) = std::fs::read_to_string(menu_file) else {
        return Vec::new();
    };
    parse_menu(&xml, |p| std::fs::read_to_string(p).ok())
}

/// Разбор файла меню; `read` читает ярлык по пути. Понимаем только то, что
/// пишет macos-look: `<AppDir>`, `<Filename>` и `<Separator/>` в `<Layout>`.
pub fn parse_menu(xml: &str, read: impl Fn(&Path) -> Option<String>) -> Vec<AppleItem> {
    let tag = |line: &str, name: &str| {
        line.trim()
            .strip_prefix(&format!("<{name}>"))
            .and_then(|l| l.strip_suffix(&format!("</{name}>")))
            .map(str::to_string)
    };
    let dir = xml
        .lines()
        .find_map(|l| tag(l, "AppDir"))
        .map(PathBuf::from)
        .unwrap_or_default();
    let locale = Locale::from_env();
    let mut out = Vec::new();
    let mut separator = false;
    for line in xml.lines() {
        if line.trim() == "<Separator/>" {
            separator = !out.is_empty();
            continue;
        }
        let Some(file) = tag(line, "Filename") else {
            continue;
        };
        let path = dir.join(&file);
        let Some(entry) = read(&path).map(|t| DesktopEntry::parse(&t)) else {
            continue;
        };
        let Some(label) = entry.localized("Name", locale.as_ref()) else {
            continue;
        };
        let ctx = Context {
            name: &label,
            icon: None,
            file: path.to_str().unwrap_or_default(),
        };
        let Some(command) = entry
            .string("Exec")
            .and_then(|e| exec::parse(&e, &ctx).ok())
        else {
            continue;
        };
        out.push(AppleItem {
            label,
            command,
            separator_before: separator,
        });
        separator = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_items_and_separators() {
        let xml = "<Menu>\n  <AppDir>/m</AppDir>\n  <Layout>\n    <Separator/>\n    \
                   <Filename>a.desktop</Filename>\n    <Separator/>\n    \
                   <Filename>b.desktop</Filename>\n    <Filename>nope.desktop</Filename>\n    \
                   <Filename>c.desktop</Filename>\n  </Layout>\n</Menu>\n";
        let read = |p: &Path| match p.to_str()? {
            "/m/a.desktop" => {
                Some("[Desktop Entry]\nName=Об этом компьютере\nExec=look action about\n".into())
            }
            "/m/b.desktop" => Some("[Desktop Entry]\nName=Сон\nExec=look action sleep\n".into()),
            "/m/c.desktop" => {
                Some("[Desktop Entry]\nName=Выйти\nExec=look \"action\" logout\n".into())
            }
            _ => None,
        };
        let items = parse_menu(xml, read);
        let short: Vec<_> = items
            .iter()
            .map(|i| (i.label.as_str(), i.separator_before))
            .collect();
        assert_eq!(
            short,
            [
                ("Об этом компьютере", false),
                ("Сон", true),
                ("Выйти", false)
            ]
        );
        assert_eq!(items[2].command, ["look", "action", "logout"]);
    }
}
