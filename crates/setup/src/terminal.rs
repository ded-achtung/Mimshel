//! The default terminal for Xfce (`helpers.rc`: Ctrl+Alt+T, «Open terminal
//! here» in Thunar), following `[apps] terminal`:
//! - our own `mimshel terminal` (the default since 2026-10-25): an Xfce helper
//!   file describing it, then `TerminalEmulator=mimshel-terminal`;
//! - kitty (the default before; Xfce Terminal lacks OSC 52, so text copied
//!   in programs over SSH never reached the clipboard): `TerminalEmulator=kitty`
//!   and our kitty.conf if the user has none.
//!
//! `mimshel apply look` sets it up; `mimshel restore` brings back the previous
//! Xfce terminal and removes only our marked files.

use std::path::{Path, PathBuf};

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

/// Name of our Xfce helper (the `TerminalEmulator` value) and its file.
const HELPER: &str = "mimshel-terminal";
/// Marks our helper file (Xfce skips `#` lines).
const HELPER_MARK: &str = "# mimshel: terminal helper";

fn helper_file() -> PathBuf {
    home().join(format!(".local/share/xfce4/helpers/{HELPER}.desktop"))
}

/// Xfce helper for our terminal: how to open it and how to run a command in it.
fn helper_text(bin: &Path) -> String {
    let bin = bin.display();
    format!(
        "{HELPER_MARK}\n[Desktop Entry]\nNoDisplay=true\nVersion=1.0\nType=X-XFCE-Helper\n\
         X-XFCE-Category=TerminalEmulator\nName=Терминал Mimshel\nIcon=utilities-terminal\n\
         X-XFCE-Commands={bin} terminal\nX-XFCE-CommandsWithParameter={bin} terminal -e \"%s\"\n"
    )
}

/// Our own terminal is chosen in `[apps] terminal` (`mimshel terminal …`).
pub fn is_mimshel(terminal: &[String]) -> bool {
    terminal
        .first()
        .is_some_and(|p| p.rsplit('/').next() == Some(mimshel_core::names::APP))
        && terminal.get(1).map(String::as_str) == Some("terminal")
}

/// Выбран ли kitty в `[apps] terminal`.
pub fn is_kitty(terminal: &[String]) -> bool {
    terminal
        .first()
        .is_some_and(|p| p.rsplit('/').next() == Some("kitty"))
}

/// Make the chosen terminal Xfce's default (see the module docs). `bin` —
/// our program (`~/.local/bin/mimshel`). Other terminals are left alone.
pub fn apply(terminal: &[String], bin: &Path) {
    if is_mimshel(terminal) {
        println!("▶ Терминал: Mimshel");
        let file = helper_file();
        if let Err(e) = write_if_changed(&file, &helper_text(bin)) {
            println!("✘ Не записать {}: {e}", file.display());
            return;
        }
        let path = helpers_rc();
        if let Err(e) = write_if_changed(&path, &set_terminal(&read(&path), HELPER)) {
            println!("✘ Не записать {}: {e}", path.display());
        }
        return;
    }
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
    if read(&helper_file()).starts_with(HELPER_MARK) {
        out.push(format!("убрать помощник Xfce {}", helper_file().display()));
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
    if read(&helper_file()).starts_with(HELPER_MARK) {
        match std::fs::remove_file(helper_file()) {
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
    fn mimshel_terminal_is_recognized() {
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_mimshel(&t(&["mimshel", "terminal", "-e"])));
        assert!(is_mimshel(&t(&[
            "/home/ded/.local/bin/mimshel",
            "terminal"
        ])));
        assert!(!is_mimshel(&t(&["mimshel", "folder"])));
        assert!(!is_mimshel(&t(&["kitty"])));
        let text = helper_text(Path::new("/home/ded/.local/bin/mimshel"));
        assert!(text.starts_with(HELPER_MARK));
        assert!(text.contains("X-XFCE-Category=TerminalEmulator"));
        assert!(text.contains(
            "X-XFCE-CommandsWithParameter=/home/ded/.local/bin/mimshel terminal -e \"%s\""
        ));
    }

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
