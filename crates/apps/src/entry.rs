//! Чтение одного файла .desktop: группа `[Desktop Entry]`, переводы ключей
//! (`Name[ru]`), списки через `;` и экранирование (`\s`, `\n`, `\\`).

use std::collections::HashMap;

/// Язык пользователя, разобранный на части: `ru_RU.UTF-8@mod` → `ru`, `RU`, `mod`.
/// Кодировка (`.UTF-8`) для выбора перевода не нужна и отбрасывается.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locale {
    lang: String,
    country: Option<String>,
    modifier: Option<String>,
}

impl Locale {
    /// Разбирает строку вида `ru_RU.UTF-8`. Для `C` и `POSIX` переводов нет — `None`.
    pub fn parse(s: &str) -> Option<Locale> {
        let (rest, modifier) = match s.split_once('@') {
            Some((rest, m)) => (rest, Some(m.to_string())),
            None => (s, None),
        };
        let rest = rest.split('.').next().unwrap_or("");
        let (lang, country) = match rest.split_once('_') {
            Some((l, c)) => (l, Some(c.to_string())),
            None => (rest, None),
        };
        if lang.is_empty() || lang == "C" || lang == "POSIX" {
            return None;
        }
        Some(Locale {
            lang: lang.to_string(),
            country,
            modifier,
        })
    }

    /// Язык сеанса: первая непустая из LC_ALL, LC_MESSAGES, LANG.
    pub fn from_env() -> Option<Locale> {
        for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(v) = std::env::var(var)
                && !v.is_empty()
            {
                return Locale::parse(&v);
            }
        }
        None
    }

    /// Какие пометки `[..]` у ключа подходят, от лучшей к худшей (порядок из стандарта).
    fn variants(&self) -> Vec<String> {
        let lang = &self.lang;
        let mut v = Vec::new();
        if let Some(c) = &self.country {
            if let Some(m) = &self.modifier {
                v.push(format!("{lang}_{c}@{m}"));
            }
            v.push(format!("{lang}_{c}"));
        }
        if let Some(m) = &self.modifier {
            v.push(format!("{lang}@{m}"));
        }
        v.push(lang.clone());
        v
    }
}

/// Ключи группы `[Desktop Entry]` одного файла, значения пока «как в файле».
#[derive(Debug, Default)]
pub struct DesktopEntry {
    keys: HashMap<String, String>,
}

impl DesktopEntry {
    /// Разбирает текст файла. Остальные группы (например, `[Desktop Action …]`) пока не нужны.
    pub fn parse(text: &str) -> DesktopEntry {
        let mut keys = HashMap::new();
        let mut in_main = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                in_main = line == "[Desktop Entry]";
                continue;
            }
            if !in_main {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                // Если ключ повторяется, берём первое значение.
                keys.entry(key.trim().to_string())
                    .or_insert_with(|| value.trim().to_string());
            }
        }
        DesktopEntry { keys }
    }

    /// Строка без перевода.
    pub fn string(&self, key: &str) -> Option<String> {
        self.keys.get(key).map(|v| unescape(v))
    }

    /// Строка на языке пользователя, а если перевода нет — без перевода.
    pub fn localized(&self, key: &str, locale: Option<&Locale>) -> Option<String> {
        self.raw_localized(key, locale).map(unescape)
    }

    /// Логическое значение: только `true` считается «да».
    pub fn boolean(&self, key: &str) -> bool {
        self.keys.get(key).is_some_and(|v| v == "true")
    }

    /// Список через `;` без перевода.
    pub fn list(&self, key: &str) -> Vec<String> {
        self.keys
            .get(key)
            .map(|v| split_list(v))
            .unwrap_or_default()
    }

    /// Список через `;` на языке пользователя.
    pub fn localized_list(&self, key: &str, locale: Option<&Locale>) -> Vec<String> {
        self.raw_localized(key, locale)
            .map(split_list)
            .unwrap_or_default()
    }

    fn raw_localized(&self, key: &str, locale: Option<&Locale>) -> Option<&str> {
        if let Some(locale) = locale {
            for variant in locale.variants() {
                if let Some(v) = self.keys.get(&format!("{key}[{variant}]")) {
                    return Some(v);
                }
            }
        }
        self.keys.get(key).map(String::as_str)
    }
}

/// Раскрывает `\s \n \t \r \\` (и `\;` внутри списков).
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(';') => out.push(';'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Делит список по `;`, не трогая `\;`. Пустые элементы (в том числе после
/// завершающей `;`) отбрасываются.
fn split_list(s: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                cur.push(c);
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            ';' => items.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    items.push(cur);
    items
        .iter()
        .map(|i| unescape(i.trim()))
        .filter(|i| !i.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
# комментарий
[Desktop Entry]
Name=Terminal
Name[ru]=Терминал
Name[ru_RU@test]=Особый
Comment=Line\\sone\\nline two
Keywords=shell;prompt;command\\;line;
Keywords[ru]=оболочка;консоль
NoDisplay=true
Hidden=false

[Desktop Action new]
Name=New Window
";

    #[test]
    fn locale_parsing() {
        let l = Locale::parse("ru_RU.UTF-8").unwrap();
        assert_eq!(l.variants(), ["ru_RU", "ru"]);
        let l = Locale::parse("sr_RS@latin").unwrap();
        assert_eq!(l.variants(), ["sr_RS@latin", "sr_RS", "sr@latin", "sr"]);
        assert_eq!(Locale::parse("C"), None);
        assert_eq!(Locale::parse("POSIX"), None);
    }

    #[test]
    fn translations() {
        let e = DesktopEntry::parse(FILE);
        let ru = Locale::parse("ru_RU.UTF-8");
        assert_eq!(e.localized("Name", ru.as_ref()).unwrap(), "Терминал");
        let special = Locale::parse("ru_RU@test");
        assert_eq!(e.localized("Name", special.as_ref()).unwrap(), "Особый");
        let de = Locale::parse("de_DE.UTF-8");
        assert_eq!(e.localized("Name", de.as_ref()).unwrap(), "Terminal");
        assert_eq!(e.localized("Name", None).unwrap(), "Terminal");
    }

    #[test]
    fn values() {
        let e = DesktopEntry::parse(FILE);
        assert_eq!(e.string("Comment").unwrap(), "Line one\nline two");
        assert_eq!(e.list("Keywords"), ["shell", "prompt", "command;line"]);
        let ru = Locale::parse("ru_RU.UTF-8");
        assert_eq!(
            e.localized_list("Keywords", ru.as_ref()),
            ["оболочка", "консоль"]
        );
        assert!(e.boolean("NoDisplay"));
        assert!(!e.boolean("Hidden"));
        assert!(!e.boolean("Terminal"));
    }

    #[test]
    fn other_groups_ignored() {
        let e = DesktopEntry::parse(FILE);
        // «New Window» из группы действия не должен перебить имя программы.
        assert_eq!(e.string("Name").unwrap(), "Terminal");
    }
}
