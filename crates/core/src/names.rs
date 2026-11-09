//! Название программы там, где его видит система: команда, папки настроек и
//! состояния, сокет службы, ярлыки, класс окон, переменные для проверок.
//! Проект называется **Mimshel**; прежнее рабочее название — `shell`.
//! Прежние имена ещё узнаём: папки переносятся сами при первом запуске,
//! переменные `SHELL_…` читаются, если нет `MIMSHEL_…`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Имя команды, папок (`~/.config/mimshel`…) и ярлыков (`mimshel-…`).
pub const APP: &str = "mimshel";
/// Прежнее имя — его узнаём и меняем на новое.
pub const OLD: &str = "shell";
/// Класс окон (вторая часть `WM_CLASS`).
pub const CLASS: &str = "Mimshel";
pub const OLD_CLASS: &str = "Shell";
/// Название для людей (уведомления, заголовки).
pub const TITLE: &str = "Mimshel";

/// The program's folder in `base` (`~/.config`, `~/.local/state`, …):
/// `base/mimshel`. When it is not there yet and the former `base/shell` is,
/// that one is renamed (once; with all settings, copies and icon places) —
/// but only when it is really ours, see [`was_ours`].
pub fn dir_in(base: &Path) -> PathBuf {
    let new = base.join(APP);
    let old = base.join(OLD);
    if !new.exists() && old.is_dir() && !old.is_symlink() && was_ours(&old) {
        let _ = std::fs::rename(&old, &new);
    }
    new
}

/// Whether a folder called `shell` is the old project's and not somebody's
/// own: `~/.config/shell` is a common home for a login profile and aliases,
/// and taking it over broke that person's shell setup. Ours holds at least
/// one of the files the old program kept in its config, data, state and
/// cache folders; a `config.toml` must also look like our settings.
fn was_ours(old: &Path) -> bool {
    const MARKS: [&str; 8] = [
        "apple.menu",
        "backups",
        "wallpapers",
        "daemon.log",
        "desktop-icons",
        "config.good.toml",
        "crashes",
        "lock",
    ];
    const SECTIONS: [&str; 6] = [
        "[parts]",
        "[look]",
        "[dock]",
        "[menubar]",
        "[apps]",
        "[lock]",
    ];
    if MARKS.iter().any(|m| old.join(m).exists()) {
        return true;
    }
    std::fs::read_to_string(old.join("config.toml"))
        .is_ok_and(|text| text.lines().any(|l| SECTIONS.contains(&l.trim())))
}

/// Переменная для проверок и отладки: `MIMSHEL_ИМЯ`, а если её нет —
/// прежняя `SHELL_ИМЯ` (`env("TIMING")`).
pub fn env(name: &str) -> Option<OsString> {
    std::env::var_os(format!("MIMSHEL_{name}"))
        .or_else(|| std::env::var_os(format!("SHELL_{name}")))
}

/// Задана ли переменная для проверок (см. [`env`]).
pub fn flag(name: &str) -> bool {
    env(name).is_some()
}

/// Ярлык или другой файл программы: `mimshel-ИМЯ` (прежний — `shell-ИМЯ`).
pub fn file(name: &str) -> String {
    format!("{APP}-{name}")
}

pub fn old_file(name: &str) -> String {
    format!("{OLD}-{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_folder_moves_once() {
        let base = std::env::temp_dir().join(format!("mimshel-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("shell")).unwrap();
        // Somebody's own `shell` folder (a login profile, aliases, even a
        // config.toml of another program) is not ours and stays.
        std::fs::write(base.join("shell/profile"), "export A=1").unwrap();
        std::fs::write(base.join("shell/config.toml"), "x").unwrap();
        assert_eq!(dir_in(&base), base.join("mimshel"));
        assert!(base.join("shell/profile").exists() && !base.join("mimshel").exists());
        // With our settings in it, it is the old project's folder.
        std::fs::write(base.join("shell/config.toml"), "[look]\nscale = 1\n").unwrap();
        let dir = dir_in(&base);
        assert_eq!(dir, base.join("mimshel"));
        assert_eq!(
            std::fs::read_to_string(dir.join("config.toml")).unwrap(),
            "[look]\nscale = 1\n"
        );
        assert!(!base.join("shell").exists());
        // Новая уже есть — прежняя (появилась снова) не трогается.
        std::fs::create_dir_all(base.join("shell")).unwrap();
        assert_eq!(dir_in(&base), base.join("mimshel"));
        assert!(base.join("shell").exists());
        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(file("folder"), "mimshel-folder");
    }
}
