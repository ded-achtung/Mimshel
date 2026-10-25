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

/// Папка программы в `base` (`~/.config`, `~/.local/state`, …): `base/mimshel`.
/// Её ещё нет, а прежняя `base/shell` есть — прежняя переименовывается (один
/// раз; со всеми настройками, копиями и местами значков).
pub fn dir_in(base: &Path) -> PathBuf {
    let new = base.join(APP);
    let old = base.join(OLD);
    if !new.exists() && old.is_dir() && !old.is_symlink() {
        let _ = std::fs::rename(&old, &new);
    }
    new
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
        std::fs::write(base.join("shell/config.toml"), "x").unwrap();
        let dir = dir_in(&base);
        assert_eq!(dir, base.join("mimshel"));
        assert_eq!(
            std::fs::read_to_string(dir.join("config.toml")).unwrap(),
            "x"
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
