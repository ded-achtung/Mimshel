//! Поиск по мере ввода, как в окне «Поиск»: насколько хорошо строка подходит к
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
        self.score_prepared(&Prepared::new(fields))
    }

    /// То же, что [`score`](Query::score), но по полям, подготовленным
    /// заранее ([`Prepared`]) — для поиска, который выполняется на каждый
    /// символ ввода.
    pub fn score_prepared(&self, p: &Prepared) -> Option<u32> {
        let direct = score_one(&self.variants[0], p);
        // Лучшая оценка другой раскладки — 1000/2 = 500: если прямой поиск
        // дал не меньше, второй вариант можно не считать.
        if direct.is_some_and(|d| d >= 500) {
            return direct;
        }
        let swapped = self
            .variants
            .get(1)
            .and_then(|q| score_one(q, p))
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

/// Поля, подготовленные для поиска один раз — при загрузке списка программ.
/// Без подготовки каждый символ ввода заново нормализовал бы все поля всех
/// программ: тысячи мелких строк на каждое нажатие клавиши.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prepared {
    /// Нормализованные названия.
    pub names: Vec<String>,
    /// Инициалы каждого названия («visual studio code» → «vsc»);
    /// параллельны `names`.
    pub initials: Vec<String>,
    /// Нормализованные ключевые слова.
    pub keywords: Vec<String>,
    /// Нормализованное остальное: описание и т. п.
    pub others: Vec<String>,
}

impl Prepared {
    /// Нормализует поля и считает инициалы названий.
    pub fn new(f: &Fields) -> Prepared {
        let names: Vec<String> = f.names.iter().map(|s| normalize(s)).collect();
        let initials = names.iter().map(|n| initials(n)).collect();
        Prepared {
            names,
            initials,
            keywords: f.keywords.iter().map(|s| normalize(s)).collect(),
            others: f.others.iter().map(|s| normalize(s)).collect(),
        }
    }
}

/// Отбирает подходящие элементы и ставит лучшие вперёд. При равной оценке
/// сохраняется исходный порядок (например, алфавитный).
pub fn best<'a, T>(
    query: &Query,
    items: &'a [T],
    fields: impl Fn(&'a T) -> Fields<'a>,
) -> Vec<&'a T> {
    let mut found: Vec<(u32, &T)> = items
        .iter()
        .filter_map(|item| query.score(&fields(item)).map(|s| (s, item)))
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, item)| item).collect()
}

/// То же, что [`best`], но по заранее подготовленным полям ([`Prepared`]).
pub fn best_prepared<'a, T>(
    query: &Query,
    items: &'a [T],
    prepared: impl Fn(&'a T) -> &'a Prepared,
) -> Vec<&'a T> {
    let mut found: Vec<(u32, &T)> = items
        .iter()
        .filter_map(|item| query.score_prepared(prepared(item)).map(|s| (s, item)))
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, item)| item).collect()
}

fn score_one(q: &str, p: &Prepared) -> Option<u32> {
    if q.is_empty() {
        return None;
    }
    let q_words: Vec<&str> = words(q).collect();
    let word_start = |v: &[String]| {
        v.iter()
            .any(|f| f.starts_with(q) || words(f).any(|w| w.starts_with(q)))
    };

    if let Some(best) = p
        .names
        .iter()
        .zip(&p.initials)
        .filter_map(|(n, ini)| score_name(q, &q_words, n, ini))
        .max()
    {
        return Some(best);
    }
    if word_start(&p.keywords) {
        return Some(450);
    }
    if word_start(&p.others) {
        return Some(400);
    }
    let everywhere = || {
        p.names
            .iter()
            .chain(&p.keywords)
            .chain(&p.others)
            .map(String::as_str)
    };
    if q_words
        .iter()
        .all(|qw| everywhere().any(|f| words(f).any(|w| w.starts_with(qw))))
    {
        return Some(300);
    }
    if q.chars().count() >= 3 && p.keywords.iter().chain(&p.others).any(|o| o.contains(q)) {
        return Some(200);
    }
    None
}

/// Совпадение с одним названием (оценки от 500 до 1000). `ini` — первые
/// буквы слов названия, посчитаны при подготовке.
fn score_name(q: &str, q_words: &[&str], name: &str, ini: &str) -> Option<u32> {
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
    if long_enough && ini.starts_with(q) {
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

    #[test]
    fn prepared_matches_fields() {
        // Подготовленные поля обязаны давать те же оценки, что и разбор на
        // месте, — включая путь, где второй вариант раскладки пропускается.
        let cases: &[(&str, Fields<'static>)] = &[
            ("fire", firefox()),
            ("Firefox", firefox()),
            ("fox", firefox()),
            ("x", firefox()),
            ("inter", firefox()),
            ("браузер", firefox()),
            ("fire веб", firefox()),
            ("узер", firefox()),
            ("tern", firefox()),
            (
                "ntrc",
                Fields {
                    names: vec!["Текстовый редактор"],
                    ..Fields::default()
                },
            ),
            (
                "vsc",
                Fields {
                    names: vec!["Visual Studio Code"],
                    ..Fields::default()
                },
            ),
            (
                "аштв",
                Fields {
                    names: vec!["Find Files"],
                    ..Fields::default()
                },
            ),
            (
                "ЁЛКА",
                Fields {
                    names: vec!["елка"],
                    ..Fields::default()
                },
            ),
        ];
        for (q, f) in cases {
            let p = Prepared::new(f);
            assert_eq!(
                Query::new(q).score(f),
                Query::new(q).score_prepared(&p),
                "запрос {q:?}"
            );
        }
        // Инициалы считаются при подготовке, один раз.
        let p = Prepared::new(&Fields {
            names: vec!["Visual Studio Code"],
            ..Fields::default()
        });
        assert_eq!(p.initials, ["vsc"]);
        assert_eq!(p.names, ["visual studio code"]);
    }

    /// Замер: поиск по большому списку на каждое нажатие. Пропущен в обычном
    /// прогоне; запуск: `cargo test -p mimshel-core search_timing -- --ignored --nocapture`
    #[test]
    #[ignore = "замер скорости"]
    fn search_timing() {
        // Название, ключевые слова и описание — как в настоящих .desktop.
        let apps: Vec<(
            &'static str,
            &'static [&'static str],
            &'static [&'static str],
        )> = (0..500)
            .map(|i| {
                (
                    if i % 2 == 0 {
                        "Текстовый редактор"
                    } else {
                        "Visual Studio Code"
                    },
                    &["internet", "web", "разработка"][..],
                    &["Какое-то описание программы на двух языках"][..],
                )
            })
            .collect();
        let prepared: Vec<Prepared> = apps
            .iter()
            .map(|(n, k, o)| {
                Prepared::new(&Fields {
                    names: vec![n],
                    keywords: k.to_vec(),
                    others: o.to_vec(),
                })
            })
            .collect();
        let indices: Vec<usize> = (0..apps.len()).collect();
        let q = Query::new("тек ред");
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(best(&q, &indices, |&i| {
                let (n, k, o) = apps[i];
                Fields {
                    names: vec![n],
                    keywords: k.to_vec(),
                    others: o.to_vec(),
                }
            }));
        }
        let old = start.elapsed();
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(best_prepared(&q, &indices, |&i| &prepared[i]));
        }
        let new = start.elapsed();
        println!("1000 запросов × 500 программ: было {old:?}, стало {new:?}");
        assert!(new < old, "подготовка полей обязана ускорять поиск");
    }
}
