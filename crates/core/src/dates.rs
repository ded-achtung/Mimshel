//! Weekday and month names in the interface language, for the clock, the
//! calendar and the lock screen.
//!
//! `date` still does the time zone and the numbers; the names come from
//! here, so they follow `look.language` and not the session locale — and
//! work when the system has no locale for that language (the maintainer's
//! laptop has only en_US and ru_RU). The English and Russian names are the
//! ones glibc prints, so nothing changes for those.
//!
//! [`prepare`] turns the name directives of a `date` format into markers
//! that carry the number (`%u`, `%m`); [`fill`] puts the names in place of
//! the markers in `date`'s output.

use crate::i18n::Language;

/// Which form of a month name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Month {
    /// `%b`: «окт», "Oct".
    Short,
    /// `%B`, inside a date: «29 октября», "October 29".
    InDate,
    /// `%OB`, on its own (a calendar header): «Октябрь».
    Alone,
}

const EN_DAYS: [(&str, &str); 7] = [
    ("Mon", "Monday"),
    ("Tue", "Tuesday"),
    ("Wed", "Wednesday"),
    ("Thu", "Thursday"),
    ("Fri", "Friday"),
    ("Sat", "Saturday"),
    ("Sun", "Sunday"),
];
const RU_DAYS: [(&str, &str); 7] = [
    ("Пн", "Понедельник"),
    ("Вт", "Вторник"),
    ("Ср", "Среда"),
    ("Чт", "Четверг"),
    ("Пт", "Пятница"),
    ("Сб", "Суббота"),
    ("Вс", "Воскресенье"),
];
const ZH_DAYS: [(&str, &str); 7] = [
    ("周一", "星期一"),
    ("周二", "星期二"),
    ("周三", "星期三"),
    ("周四", "星期四"),
    ("周五", "星期五"),
    ("周六", "星期六"),
    ("周日", "星期日"),
];
const ZH_TW_DAYS: [(&str, &str); 7] = [
    ("週一", "星期一"),
    ("週二", "星期二"),
    ("週三", "星期三"),
    ("週四", "星期四"),
    ("週五", "星期五"),
    ("週六", "星期六"),
    ("週日", "星期日"),
];

/// `(short, in a date, on its own)`.
const EN_MONTHS: [(&str, &str, &str); 12] = [
    ("Jan", "January", "January"),
    ("Feb", "February", "February"),
    ("Mar", "March", "March"),
    ("Apr", "April", "April"),
    ("May", "May", "May"),
    ("Jun", "June", "June"),
    ("Jul", "July", "July"),
    ("Aug", "August", "August"),
    ("Sep", "September", "September"),
    ("Oct", "October", "October"),
    ("Nov", "November", "November"),
    ("Dec", "December", "December"),
];
const RU_MONTHS: [(&str, &str, &str); 12] = [
    ("янв", "января", "Январь"),
    ("фев", "февраля", "Февраль"),
    ("мар", "марта", "Март"),
    ("апр", "апреля", "Апрель"),
    ("мая", "мая", "Май"),
    ("июн", "июня", "Июнь"),
    ("июл", "июля", "Июль"),
    ("авг", "августа", "Август"),
    ("сен", "сентября", "Сентябрь"),
    ("окт", "октября", "Октябрь"),
    ("ноя", "ноября", "Ноябрь"),
    ("дек", "декабря", "Декабрь"),
];
/// The same in Simplified and Traditional Chinese.
const ZH_MONTHS: [(&str, &str, &str); 12] = [
    ("1月", "一月", "一月"),
    ("2月", "二月", "二月"),
    ("3月", "三月", "三月"),
    ("4月", "四月", "四月"),
    ("5月", "五月", "五月"),
    ("6月", "六月", "六月"),
    ("7月", "七月", "七月"),
    ("8月", "八月", "八月"),
    ("9月", "九月", "九月"),
    ("10月", "十月", "十月"),
    ("11月", "十一月", "十一月"),
    ("12月", "十二月", "十二月"),
];

/// The weekday `n` (1 = Monday … 7 = Sunday, as `date +%u`).
pub fn weekday(lang: Language, n: u32, full: bool) -> &'static str {
    let days = match lang.resolve() {
        Language::Ru => &RU_DAYS,
        Language::Zh => &ZH_DAYS,
        Language::ZhTw => &ZH_TW_DAYS,
        _ => &EN_DAYS,
    };
    let (short, long) = days[(n.clamp(1, 7) - 1) as usize];
    if full { long } else { short }
}

/// The month `n` (1 = January).
pub fn month(lang: Language, n: u32, form: Month) -> &'static str {
    let months = match lang.resolve() {
        Language::Ru => &RU_MONTHS,
        Language::Zh | Language::ZhTw => &ZH_MONTHS,
        _ => &EN_MONTHS,
    };
    let (short, in_date, alone) = months[(n.clamp(1, 12) - 1) as usize];
    match form {
        Month::Short => short,
        Month::InDate => in_date,
        Month::Alone => alone,
    }
}

/// `date` format of today in words, as the calendar and the lock screen
/// show it: «Четверг, 29 октября», "Thursday, October 29", 10月29日 星期四.
pub fn today_format(lang: Language) -> &'static str {
    match lang.resolve() {
        Language::Ru => "%A, %-d %B",
        Language::Zh | Language::ZhTw => "%-m月%-d日 %A",
        _ => "%A, %B %-d",
    }
}

/// Marker bounds in `date`'s output (characters no format contains).
const OPEN: char = '\u{1}';
const CLOSE: char = '\u{2}';

/// `format` for `date` with the weekday and month names (`%a %A %b %h %B
/// %OB`) replaced by markers holding their numbers. Directives with flags
/// or a width (`%^a`, `%10B`) and everything else pass through unchanged.
pub fn prepare(format: &str) -> String {
    let mut out = String::with_capacity(format.len() + 16);
    let mut rest = format.chars().peekable();
    while let Some(c) = rest.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let marker = |kind: char, number: &str| format!("{OPEN}{kind}{number}{CLOSE}");
        match rest.peek().copied() {
            Some('a') => out.push_str(&marker('a', "%u")),
            Some('A') => out.push_str(&marker('A', "%u")),
            Some('b' | 'h') => out.push_str(&marker('b', "%m")),
            Some('B') => out.push_str(&marker('B', "%m")),
            Some('O') => {
                rest.next();
                match rest.peek().copied() {
                    Some('B') => out.push_str(&marker('O', "%m")),
                    Some('b' | 'h') => out.push_str(&marker('b', "%m")),
                    _ => {
                        out.push_str("%O");
                        continue;
                    }
                }
            }
            // `%%` stays as it is, and so does the next character: it is
            // not the start of a directive.
            Some('%') => out.push_str("%%"),
            _ => {
                out.push('%');
                continue;
            }
        }
        rest.next();
    }
    out
}

/// `date`'s output of a [`prepare`]d format with the names put in.
pub fn fill(out: &str, lang: Language) -> String {
    let mut text = String::with_capacity(out.len());
    let mut rest = out;
    while let Some(start) = rest.find(OPEN) {
        text.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len_utf8()..];
        let Some(end) = after.find(CLOSE) else {
            rest = after;
            break;
        };
        let mut body = after[..end].chars();
        let kind = body.next();
        let n: Option<u32> = body.as_str().parse().ok();
        let name = match (kind, n) {
            (Some('a'), Some(n)) => weekday(lang, n, false),
            (Some('A'), Some(n)) => weekday(lang, n, true),
            (Some('b'), Some(n)) => month(lang, n, Month::Short),
            (Some('B'), Some(n)) => month(lang, n, Month::InDate),
            (Some('O'), Some(n)) => month(lang, n, Month::Alone),
            _ => "",
        };
        text.push_str(name);
        rest = &after[end + CLOSE.len_utf8()..];
    }
    // No markers left — or a broken one: drop the marker characters.
    text.extend(rest.chars().filter(|&c| c != OPEN && c != CLOSE));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `date` prints for a prepared format on Thursday 29 October 2026,
    /// 14:19 (only the directives the tests use).
    fn fake_date(format: &str) -> String {
        format
            .replace("%u", "4")
            .replace("%m", "10")
            .replace("%-m", "10")
            .replace("%-d", "29")
            .replace("%H", "14")
            .replace("%M", "19")
            .replace("%%", "%")
    }

    fn run(format: &str, lang: Language) -> String {
        fill(&fake_date(&prepare(format)), lang)
    }

    #[test]
    fn clock_in_every_language() {
        let clock = "%a %-d %b  %H:%M";
        assert_eq!(run(clock, Language::Ru), "Чт 29 окт  14:19");
        assert_eq!(run(clock, Language::En), "Thu 29 Oct  14:19");
        assert_eq!(run(clock, Language::Zh), "周四 29 10月  14:19");
        assert_eq!(run(clock, Language::ZhTw), "週四 29 10月  14:19");
    }

    #[test]
    fn today_in_words() {
        let today = |lang| run(today_format(lang), lang);
        assert_eq!(today(Language::Ru), "Четверг, 29 октября");
        assert_eq!(today(Language::En), "Thursday, October 29");
        assert_eq!(today(Language::Zh), "10月29日 星期四");
        assert_eq!(today(Language::ZhTw), "10月29日 星期四");
        assert_eq!(run("%OB", Language::Ru), "Октябрь");
        assert_eq!(run("%h", Language::En), "Oct");
    }

    #[test]
    fn other_directives_pass_through() {
        assert_eq!(
            prepare("%H:%M %%a %^a %10B %Om %"),
            "%H:%M %%a %^a %10B %Om %"
        );
        assert_eq!(run("100%% %a", Language::En), "100% Thu");
        // Plain text and a broken marker.
        assert_eq!(fill("14:19", Language::Ru), "14:19");
        assert_eq!(fill("x\u{1}a4", Language::Ru), "xa4");
        assert_eq!(fill("\u{1}z9\u{2}!", Language::Ru), "!");
    }

    #[test]
    fn names_by_number() {
        assert_eq!(weekday(Language::Ru, 1, true), "Понедельник");
        assert_eq!(weekday(Language::En, 7, false), "Sun");
        assert_eq!(month(Language::Ru, 5, Month::Short), "мая");
        assert_eq!(month(Language::Zh, 12, Month::Alone), "十二月");
        // Out of range is clamped, never a panic.
        assert_eq!(weekday(Language::En, 0, false), "Mon");
        assert_eq!(month(Language::En, 13, Month::Short), "Dec");
    }
}
