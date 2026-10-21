//! Поиск по мере ввода, как в Spotlight: насколько хорошо строка подходит к
//! названию программы и к её описанию.
//!
//! Порядок важности: название целиком → начало названия → начало слова в
//! названии → инициалы («vsc» → «Visual Studio Code») → кусок названия →
//! ключевые слова → описание. Регистр и «ё/е» не важны.
//! Если текст набран не в той раскладке («ntvbyfk» вместо «терминал»),
//! он тоже находится, но ниже правильно набранного.

/// Запрос пользователя, подготовленный для сравнения.
#[derive(Debug, Clone)]
pub struct Query {
    /// Сам запрос и, если отличается, он же в другой раскладке.
    variants: Vec<String>,
}

impl Query {
    pub fn new(text: &str) -> Query {
        let q = normalize(text);
        let swapped = swap_layout(&q);
        let mut variants = vec![q];
        if swapped != variants[0] {
            variants.push(swapped);
        }
        Query { variants }
    }

    pub fn is_empty(&self) -> bool {
        self.variants[0].is_empty()
    }

    /// Оценка совпадения: больше — лучше, `None` — не подходит.
    pub fn score(&self, fields: &Fields) -> Option<u32> {
        let norm = |v: &[&str]| v.iter().map(|s| normalize(s)).collect::<Vec<_>>();
        let f = [
            norm(&fields.names),
            norm(&fields.keywords),
            norm(&fields.others),
        ];
        let direct = score_one(&self.variants[0], &f);
        let swapped = self
            .variants
            .get(1)
            .and_then(|q| score_one(q, &f))
            .map(|s| s / 2);
        direct.max(swapped)
    }
}

/// Где искать, от важного к менее важному.
#[derive(Debug, Default)]
pub struct Fields<'a> {
    /// Названия: переведённое и исходное.
    pub names: Vec<&'a str>,
    /// Слова, которые автор программы написал специально для поиска.
    pub keywords: Vec<&'a str>,
    /// Остальное: описание и т. п.
    pub others: Vec<&'a str>,
}

/// Отбирает подходящие элементы и ставит лучшие вперёд. При равной оценке
/// сохраняется исходный порядок (например, алфавитный).
pub fn best<'a, T>(query: &Query, items: &'a [T], fields: impl Fn(&T) -> Fields) -> Vec<&'a T> {
    let mut found: Vec<(u32, &T)> = items
        .iter()
        .filter_map(|item| query.score(&fields(item)).map(|s| (s, item)))
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, item)| item).collect()
}

fn score_one(q: &str, [names, keywords, others]: &[Vec<String>; 3]) -> Option<u32> {
    if q.is_empty() {
        return None;
    }
    let q_words: Vec<&str> = words(q).collect();
    let word_start = |f: &String| f.starts_with(q) || words(f).any(|w| w.starts_with(q));

    if let Some(best) = names
        .iter()
        .filter_map(|n| score_name(q, &q_words, n))
        .max()
    {
        return Some(best);
    }
    if keywords.iter().any(word_start) {
        return Some(450);
    }
    if others.iter().any(word_start) {
        return Some(400);
    }
    let everywhere = || {
        names
            .iter()
            .chain(keywords)
            .chain(others)
            .map(String::as_str)
    };
    if q_words
        .iter()
        .all(|qw| everywhere().any(|f| words(f).any(|w| w.starts_with(qw))))
    {
        return Some(300);
    }
    if q.chars().count() >= 3 && keywords.iter().chain(others).any(|o| o.contains(q)) {
        return Some(200);
    }
    None
}

/// Совпадение с одним названием (оценки от 500 до 1000).
fn score_name(q: &str, q_words: &[&str], name: &str) -> Option<u32> {
    let long_enough = q.chars().count() >= 2;
    if name == q {
        return Some(1000);
    }
    if name.starts_with(q) {
        return Some(900);
    }
    if words(name).any(|w| w.starts_with(q)) {
        return Some(800);
    }
    if q_words
        .iter()
        .all(|qw| words(name).any(|w| w.starts_with(qw)))
    {
        return Some(700);
    }
    if long_enough && initials(name).starts_with(q) {
        return Some(600);
    }
    if long_enough && name.contains(q) {
        return Some(500);
    }
    None
}

/// Нижний регистр, «ё» → «е», лишние пробелы убраны.
fn normalize(s: &str) -> String {
    let lower = s.to_lowercase().replace('ё', "е");
    lower.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Слова: всё, что разделено не-буквами и не-цифрами.
fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
}

/// Первые буквы слов: «visual studio code» → «vsc».
fn initials(s: &str) -> String {
    words(s).filter_map(|w| w.chars().next()).collect()
}

/// Латиница ↔ кириллица по расположению клавиш (раскладки us и ru).
fn swap_layout(s: &str) -> String {
    const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`";
    const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбюё";
    s.chars()
        .map(|c| {
            if let Some(i) = EN.chars().position(|e| e == c) {
                RU.chars().nth(i).unwrap_or(c)
            } else if let Some(i) = RU.chars().position(|r| r == c) {
                EN.chars().nth(i).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(q: &str, name: &str) -> Option<u32> {
        Query::new(q).score(&Fields {
            names: vec![name],
            ..Fields::default()
        })
    }

    fn firefox() -> Fields<'static> {
        Fields {
            names: vec!["Firefox"],
            keywords: vec!["internet", "web"],
            others: vec!["Веб-браузер"],
        }
    }

    #[test]
    fn tiers() {
        assert_eq!(s("Firefox", "firefox"), Some(1000));
        assert_eq!(s("fire", "Firefox"), Some(900));
        assert_eq!(s("ред", "Текстовый редактор"), Some(800));
        assert_eq!(s("тек ред", "Текстовый редактор"), Some(700));
        assert_eq!(s("vsc", "Visual Studio Code"), Some(600));
        assert_eq!(s("fox", "Firefox"), Some(500));
        assert_eq!(s("x", "Firefox"), None);
        assert_eq!(s("chrome", "Firefox"), None);
    }

    #[test]
    fn other_fields() {
        let score = |q: &str| Query::new(q).score(&firefox());
        assert_eq!(score("inter"), Some(450));
        assert_eq!(score("браузер"), Some(400));
        assert_eq!(score("fire веб"), Some(300));
        assert_eq!(score("узер"), Some(200));
        assert_eq!(score("tern"), Some(200));
    }

    #[test]
    fn yo_and_case() {
        assert_eq!(s("ЁЛКА", "елка"), Some(1000));
        assert_eq!(s("  тек   ред ", "Текстовый редактор"), Some(700));
    }

    #[test]
    fn wrong_layout() {
        // «ntrc» — это «текс», набранное в английской раскладке.
        assert_eq!(s("ntrc", "Текстовый редактор"), Some(450));
        // «аштв» — это «find».
        assert_eq!(s("аштв", "Find Files"), Some(450));
    }

    #[test]
    fn untranslated_name_counts_as_name() {
        let q = Query::new("term");
        let terminal = Fields {
            names: vec!["Терминал", "Terminal"],
            ..Fields::default()
        };
        assert_eq!(q.score(&terminal), Some(900));
        // Ключевое слово важнее описания другой программы.
        let renamed = Fields {
            names: vec!["Терминал"],
            keywords: vec!["terminal", "shell"],
            ..Fields::default()
        };
        let other = Fields {
            names: vec!["eos-update"],
            others: vec!["Run in terminal"],
            ..Fields::default()
        };
        assert!(q.score(&renamed) > q.score(&other));
    }

    #[test]
    fn ordering() {
        let apps = [
            "Firefox",
            "Терминал",
            "Files",
            "Mousepad",
            "Профиль Firefox",
        ];
        let q = Query::new("fi");
        let found = best(&q, &apps, |a| Fields {
            names: vec![*a],
            ..Fields::default()
        });
        assert_eq!(found, [&"Firefox", &"Files", &"Профиль Firefox"]);
    }
}
