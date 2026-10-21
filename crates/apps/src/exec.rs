//! Строка `Exec=` → список аргументов для запуска.
//!
//! По стандарту аргументы разделяются пробелами, в двойных кавычках `\"`, `` \` ``,
//! `\$` и `\\` означают сам символ. Коды `%f %F %u %U` — место для файлов; Spotlight
//! запускает программу без файлов, поэтому они просто убираются. `%i` → `--icon имя`,
//! `%c` → название программы, `%k` → путь к самому .desktop, `%%` → `%`.

use std::fmt;

/// Что подставлять вместо `%i`, `%c`, `%k`.
pub struct Context<'a> {
    pub name: &'a str,
    pub icon: Option<&'a str>,
    pub file: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExecError {
    /// Кавычка открыта и не закрыта.
    UnclosedQuote,
    /// После удаления кодов не осталось ни одного аргумента.
    Empty,
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::UnclosedQuote => write!(f, "в Exec не закрыта кавычка"),
            ExecError::Empty => write!(f, "в Exec нет команды"),
        }
    }
}

impl std::error::Error for ExecError {}

/// Собирает аргументы по одному.
#[derive(Default)]
struct Args {
    done: Vec<String>,
    cur: String,
    /// Начат ли аргумент (нужно, чтобы не потерять пустой `""`).
    started: bool,
    /// Был ли в аргументе код вроде `%f`: если без него аргумент пуст — выбрасываем.
    had_code: bool,
}

impl Args {
    fn push(&mut self, s: &str) {
        self.started = true;
        self.cur.push_str(s);
    }

    fn finish(&mut self) {
        if self.started && !(self.cur.is_empty() && self.had_code) {
            self.done.push(std::mem::take(&mut self.cur));
        }
        self.cur.clear();
        self.started = false;
        self.had_code = false;
    }
}

pub fn parse(exec: &str, ctx: &Context) -> Result<Vec<String>, ExecError> {
    let mut a = Args::default();
    let mut chars = exec.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => a.finish(),
            '"' => {
                a.started = true;
                loop {
                    match chars.next() {
                        None => return Err(ExecError::UnclosedQuote),
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(e @ ('"' | '`' | '$' | '\\')) => a.cur.push(e),
                            Some(other) => {
                                a.cur.push('\\');
                                a.cur.push(other);
                            }
                            None => return Err(ExecError::UnclosedQuote),
                        },
                        Some(other) => a.cur.push(other),
                    }
                }
            }
            '%' => match chars.next() {
                Some('%') => a.push("%"),
                Some('c') => a.push(ctx.name),
                Some('k') => a.push(ctx.file),
                Some('i') => {
                    // По стандарту %i стоит отдельно и превращается в два аргумента.
                    let alone = !a.started && chars.peek().is_none_or(|n| *n == ' ' || *n == '\t');
                    match ctx.icon {
                        Some(icon) if alone => {
                            a.push("--icon");
                            a.finish();
                            a.push(icon);
                        }
                        Some(icon) => a.push(icon),
                        None => {
                            a.started = true;
                            a.had_code = true;
                        }
                    }
                }
                // Файлы, адреса и устаревшие коды — убираем.
                _ => {
                    a.started = true;
                    a.had_code = true;
                }
            },
            _ => {
                a.started = true;
                a.cur.push(c);
            }
        }
    }
    a.finish();

    if a.done.is_empty() {
        return Err(ExecError::Empty);
    }
    Ok(a.done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(exec: &str) -> Result<Vec<String>, ExecError> {
        let ctx = Context {
            name: "Текстовый редактор",
            icon: Some("mousepad"),
            file: "/usr/share/applications/m.desktop",
        };
        parse(exec, &ctx)
    }

    #[test]
    fn plain() {
        assert_eq!(run("mousepad").unwrap(), ["mousepad"]);
        assert_eq!(run("  xterm   -e  top ").unwrap(), ["xterm", "-e", "top"]);
    }

    #[test]
    fn file_codes_removed() {
        assert_eq!(run("mousepad %U").unwrap(), ["mousepad"]);
        assert_eq!(run("gimp %f --new").unwrap(), ["gimp", "--new"]);
        assert_eq!(run("app --file=%f").unwrap(), ["app", "--file="]);
    }

    #[test]
    fn quotes() {
        assert_eq!(
            run(r#"sh -c "echo \"hi\" \$HOME \\ end""#).unwrap(),
            ["sh", "-c", r#"echo "hi" $HOME \ end"#]
        );
        assert_eq!(run(r#"app "" x"#).unwrap(), ["app", "", "x"]);
        assert_eq!(run(r#""/opt/My App/run" %u"#).unwrap(), ["/opt/My App/run"]);
        assert_eq!(run(r#"app "oops"#), Err(ExecError::UnclosedQuote));
    }

    #[test]
    fn substitutions() {
        assert_eq!(run("app %i").unwrap(), ["app", "--icon", "mousepad"]);
        assert_eq!(
            run("app --title %c").unwrap(),
            ["app", "--title", "Текстовый редактор"]
        );
        assert_eq!(
            run("app %k").unwrap(),
            ["app", "/usr/share/applications/m.desktop"]
        );
        assert_eq!(run("app 100%%").unwrap(), ["app", "100%"]);

        let no_icon = Context {
            name: "x",
            icon: None,
            file: "f",
        };
        assert_eq!(parse("app %i", &no_icon).unwrap(), ["app"]);
    }

    #[test]
    fn empty() {
        assert_eq!(run(""), Err(ExecError::Empty));
        assert_eq!(run("%U"), Err(ExecError::Empty));
    }
}
