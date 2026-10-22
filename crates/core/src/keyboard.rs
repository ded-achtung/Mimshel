//! Раскладки клавиатуры: как их назвать в строке меню.

/// Раскладки из имени набора символов XKB, по порядку групп:
/// `pc+us+ru:2+inet(evdev)+group(alt_shift_toggle)` → `["us", "ru"]`.
/// Первая раскладка идёт сразу после `pc`, остальные помечены номером
/// группы (`:2`); прочее (`inet`, `group(...)`) — не раскладки.
pub fn layouts_from_symbols(symbols: &str) -> Vec<String> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (i, part) in symbols.split('+').enumerate() {
        let (name, group) = match part.split_once(':') {
            Some((name, n)) => match n.parse::<usize>() {
                Ok(n) => (name, n),
                Err(_) => continue,
            },
            None if i == 1 => (part, 1),
            None => continue,
        };
        // «us(intl)» → «us».
        let code = name.split('(').next().unwrap_or(name);
        if !code.is_empty() {
            out.push((group, code.to_string()));
        }
    }
    out.sort_by_key(|(g, _)| *g);
    out.into_iter().map(|(_, c)| c).collect()
}

/// Короткая подпись раскладки: английские — «en», остальные — их код.
pub fn layout_label(code: &str) -> String {
    match code {
        "us" | "gb" => "en".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_symbols() {
        assert_eq!(
            layouts_from_symbols("pc+us+ru:2+inet(evdev)+group(alt_shift_toggle)"),
            ["us", "ru"]
        );
        assert_eq!(
            layouts_from_symbols("pc+de(nodeadkeys)+us:2+ua:3+inet(evdev)"),
            ["de", "us", "ua"]
        );
        assert_eq!(layouts_from_symbols("pc+us+inet(evdev)"), ["us"]);
        assert!(layouts_from_symbols("").is_empty());
        assert_eq!(layout_label("us"), "en");
        assert_eq!(layout_label("ru"), "ru");
    }
}
