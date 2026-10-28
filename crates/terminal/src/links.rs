//! Web addresses in terminal text: Ctrl+click opens the one under the pointer
//! in the browser (`xdg-open`).

/// Schemes we recognise.
const SCHEMES: [&str; 3] = ["https://", "http://", "file://"];

/// The address in `line` (a screen row, one char per cell) covering column
/// `col`, if any. Trailing punctuation (`.`, `,`, `)` without its pair…) is
/// not part of it, as in other terminals.
pub fn url_at(line: &[char], col: usize) -> Option<String> {
    if col >= line.len() {
        return None;
    }
    let stop = |c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`' | '\0');
    if stop(line[col]) {
        return None;
    }
    // The «word» around the column.
    let mut start = col;
    while start > 0 && !stop(line[start - 1]) {
        start -= 1;
    }
    let mut end = col + 1;
    while end < line.len() && !stop(line[end]) {
        end += 1;
    }
    let word: String = line[start..end].iter().collect();
    // Where the address starts inside the word (it may follow `(` or `=`).
    let (at, _) = SCHEMES
        .iter()
        .filter_map(|s| word.find(s).map(|i| (i, s)))
        .min_by_key(|(i, _)| *i)?;
    let at_col = start + word[..at].chars().count();
    let mut url: Vec<char> = word[at..].chars().collect();
    // Trailing punctuation; a closing bracket stays only if it has a pair.
    while let Some(&last) = url.last() {
        let opens = |o: char| url.iter().filter(|&&c| c == o).count();
        let unpaired = match last {
            ')' => opens('(') < opens(')'),
            ']' => opens('[') < opens(']'),
            '}' => opens('{') < opens('}'),
            '.' | ',' | ';' | ':' | '!' | '?' => true,
            _ => false,
        };
        if !unpaired {
            break;
        }
        url.pop();
    }
    let url_end = at_col + url.len();
    let scheme_only = SCHEMES.iter().any(|s| s.chars().count() == url.len());
    ((at_col..url_end).contains(&col) && !scheme_only).then(|| url.into_iter().collect())
}

/// Whether Ctrl+click may hand `url` to `xdg-open`. Web addresses yes; a
/// `file://` link only when it is not something that runs: on Xfce
/// `xdg-open` goes to `exo-open`, which launches `.desktop` files, and a
/// program printed by a remote host (`file:///home/u/Downloads/x.desktop`)
/// must not start with one click.
pub fn safe_to_open(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("file://") else {
        return true;
    };
    // `file://host/path`: the path starts at the first slash.
    let path = std::path::PathBuf::from(percent_decode(&rest[rest.find('/').unwrap_or(0)..]));
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("desktop"))
    {
        return false;
    }
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(&path) {
        Ok(m) if m.is_file() => m.permissions().mode() & 0o111 == 0,
        _ => true,
    }
}

/// `%2E` -> `.`; a broken escape stays as it is.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, col: usize) -> Option<String> {
        url_at(&text.chars().collect::<Vec<_>>(), col)
    }

    #[test]
    fn runnable_files_are_not_opened() {
        assert!(safe_to_open("https://example.org/x.desktop"));
        assert!(!safe_to_open("file:///home/u/Downloads/x.desktop"));
        assert!(!safe_to_open("file:///home/u/x%2EDesktop"));
        assert!(!safe_to_open("file://localhost/tmp/a.desktop"));
        assert!(!safe_to_open("file:///bin/sh"), "an executable file");
        assert!(safe_to_open("file:///etc/hostname-missing.txt"));
        assert!(safe_to_open("file:///tmp"), "a folder opens");
    }

    #[test]
    fn finds_addresses() {
        let line = "см. https://github.com/ded-achtung/Mimshel. ок";
        assert_eq!(
            at(line, 10).as_deref(),
            Some("https://github.com/ded-achtung/Mimshel"),
            "the full stop after the address is not part of it"
        );
        assert_eq!(at(line, 2), None, "plain text");
        assert_eq!(at(line, 43), None, "after the address");
        assert_eq!(
            at("(see https://en.wikipedia.org/wiki/Rust_(language))", 10).as_deref(),
            Some("https://en.wikipedia.org/wiki/Rust_(language)"),
            "a bracket with its pair stays, the outer one goes"
        );
        assert_eq!(
            at("href=\"http://localhost:8080/a?b=1\"", 9).as_deref(),
            Some("http://localhost:8080/a?b=1")
        );
        assert_eq!(at("https:// nothing", 3), None);
    }
}
