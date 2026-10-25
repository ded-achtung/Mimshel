//! Терминал по умолчанию — kitty (решение пользователя 25.10.2026): Терминал
//! Xfce не умеет OSC 52, и текст, скопированный в программе по SSH (Claude
//! Code, tmux, vim), не попадал в буфер обмена.
//!
//! `mimshel apply look`, если в `[apps] terminal` выбран kitty и он
//! установлен: делает его терминалом Xfce (`helpers.rc`: Ctrl+Alt+T,
//! «Открыть терминал здесь» в Thunar) и кладёт наши настройки kitty, если
//! своих у человека нет. `mimshel restore` возвращает прежний терминал Xfce и
//! убирает только наши, помеченные настройки kitty.

use std::path::PathBuf;

use crate::sys::{home, read, which, write_if_changed};

/// Наши настройки kitty (тёмная тема, Ctrl+Shift+C/V, вкладки, OSC 52).
const KITTY_CONF: &str = include_str!("../data/kitty.conf");
/// Первая строка нашего `kitty.conf` — по ней откат узнаёт свой файл.
const KITTY_MARK: &str = "# mimshel: настройки kitty";
/// Строка в `helpers.rc`, где запомнен прежний терминал Xfce (пусто — не был
/// выбран). Xfce читает файл как `ключ=значение` и строки с `#` пропускает.
const HELPERS_MARK: &str = "# mimshel: прежний TerminalEmulator=";

fn helpers_rc() -> PathBuf {
    home().join(".config/xfce4/helpers.rc")
}

fn kitty_conf() -> PathBuf {
    home().join(".config/kitty/kitty.conf")
}

/// Выбран ли kitty в `[apps] terminal`.
pub fn is_kitty(terminal: &[String]) -> bool {
    terminal
        .first()
        .is_some_and(|p| p.rsplit('/').next() == Some("kitty"))
}

/// Включить kitty (см. описание модуля). Ничего не делает, если выбран другой
/// терминал или kitty не установлен.
pub fn apply(terminal: &[String]) {
    if !is_kitty(terminal) {
        return;
    }
    if which("kitty").is_none() {
        println!("✘ Нет kitty — запустите ./install.sh в папке проекта");
        return;
    }
    println!("▶ Терминал: kitty");
    let path = helpers_rc();
    let new = set_terminal(&read(&path), "kitty");
    if let Err(e) = write_if_changed(&path, &new) {
        println!("✘ Не записать {}: {e}", path.display());
    }
    let conf = kitty_conf();
    if conf.exists() {
        println!("  свои настройки kitty не трогаю: {}", conf.display());
    } else if let Err(e) = write_if_changed(&conf, KITTY_CONF) {
        println!("✘ Не записать {}: {e}", conf.display());
    }
}

/// Что вернёт откат (для `restore --dry-run`): пусто — ничего нашего нет.
pub fn owned() -> Vec<String> {
    let mut out = Vec::new();
    if read(&helpers_rc()).contains(HELPERS_MARK) {
        out.push("вернуть прежний терминал Xfce (helpers.rc)".into());
    }
    if read(&kitty_conf()).starts_with(KITTY_MARK) {
        out.push(format!(
            "убрать свои настройки kitty {}",
            kitty_conf().display()
        ));
    }
    out
}

/// Откат: прежний терминал Xfce и без наших настроек kitty.
pub fn cleanup() -> std::io::Result<()> {
    let path = helpers_rc();
    let text = read(&path);
    if text.contains(HELPERS_MARK) {
        write_if_changed(&path, &restore_terminal(&text))?;
    }
    if read(&kitty_conf()).starts_with(KITTY_MARK) {
        match std::fs::remove_file(kitty_conf()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// `helpers.rc` с `TerminalEmulator=program`. Прежнее значение запоминается
/// пометкой — только в первый раз, чтобы повторный `apply` не затёр его.
fn set_terminal(text: &str, program: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut old = String::new();
    for line in text.lines() {
        match line.strip_prefix("TerminalEmulator=") {
            Some(v) => old = v.to_string(),
            None => lines.push(line.to_string()),
        }
    }
    if !text.contains(HELPERS_MARK) {
        lines.push(format!("{HELPERS_MARK}{old}"));
    }
    lines.push(format!("TerminalEmulator={program}"));
    lines.join("\n") + "\n"
}

/// Обратно к [`set_terminal`]: прежний терминал (или никакого) и без пометки.
fn restore_terminal(text: &str) -> String {
    let old = text
        .lines()
        .find_map(|l| l.strip_prefix(HELPERS_MARK))
        .unwrap_or("")
        .to_string();
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| !l.starts_with(HELPERS_MARK) && !l.starts_with("TerminalEmulator="))
        .map(str::to_string)
        .collect();
    if !old.is_empty() {
        lines.push(format!("TerminalEmulator={old}"));
    }
    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_is_recognized_by_program_name() {
        assert!(is_kitty(&["kitty".into()]));
        assert!(is_kitty(&["/usr/bin/kitty".into(), "--".into()]));
        assert!(!is_kitty(&["xfce4-terminal".into(), "-x".into()]));
        assert!(!is_kitty(&[]));
    }

    #[test]
    fn terminal_is_set_and_restored() {
        let before = "WebBrowser=firefox\nTerminalEmulator=xfce4-terminal\n";
        let set = set_terminal(before, "kitty");
        assert_eq!(
            set,
            "WebBrowser=firefox\n# mimshel: прежний TerminalEmulator=xfce4-terminal\n\
             TerminalEmulator=kitty\n"
        );
        // Повторный apply не затирает запомненный прежний терминал.
        assert_eq!(set_terminal(&set, "kitty"), set);
        assert_eq!(
            restore_terminal(&set),
            "WebBrowser=firefox\nTerminalEmulator=xfce4-terminal\n"
        );
    }

    #[test]
    fn no_previous_terminal_restores_to_nothing() {
        let set = set_terminal("", "kitty");
        assert_eq!(
            set,
            "# mimshel: прежний TerminalEmulator=\nTerminalEmulator=kitty\n"
        );
        assert_eq!(restore_terminal(&set), "");
    }

    #[test]
    fn our_kitty_conf_is_marked() {
        assert!(KITTY_CONF.starts_with(KITTY_MARK));
        assert!(KITTY_CONF.contains("clipboard_control write-clipboard"));
    }
}
