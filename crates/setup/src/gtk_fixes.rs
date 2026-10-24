//! Поправки к теме GTK 3 — своим кусочком в `~/.config/gtk-3.0/gtk.css`
//! между пометками; остальное в файле — пользователя, его не трогаем.
//!
//! Рамка выделения в Проводнике получает от Colloid лишнее: скругление
//! области файлов (`widget.view`) и тень наведения (`.view:hover` —
//! `box-shadow` с огромным размахом). Проводник перерисовывает только
//! прямоугольник рамки — всё, что рисуется сверх него, остаётся следами
//! (пунктир за углом, шум по краям). Рамка — без скругления, тени и обводки,
//! как в стандартной теме Adwaita.

use std::path::{Path, PathBuf};

const BEGIN: &str = "/* shell: начало — пишет shell apply look, не правьте */";
const END: &str = "/* shell: конец */";
const CSS: &str = "\
.thunar .standard-view.frame widget.view.rubberband,
.thunar .standard-view.frame widget.view .rubberband,
.thunar .standard-view.frame widget.view rubberband {
  border-radius: 0;
  box-shadow: none;
  outline: none;
  background-image: none;
}";

fn file() -> PathBuf {
    crate::sys::home().join(".config/gtk-3.0/gtk.css")
}

/// Вписать свой кусок (или обновить его). Запущенные программы увидят
/// его только после перезапуска — `true`, если файл изменился.
pub fn apply() -> bool {
    apply_in(&file())
}

/// Убрать свой кусок; файл, где больше ничего нет, — удалить.
pub fn cleanup() -> std::io::Result<()> {
    let path = file();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let rest = without_block(&text);
    if rest == text {
        return Ok(());
    }
    if rest.trim().is_empty() {
        std::fs::remove_file(&path)
    } else {
        std::fs::write(&path, rest)
    }
}

/// Есть ли в файле наш кусок (для отката с `--dry-run`).
pub fn present() -> bool {
    std::fs::read_to_string(file()).is_ok_and(|t| t.contains(BEGIN))
}

fn apply_in(path: &Path) -> bool {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let new = with_block(&text);
    if new == text {
        return false;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(path, new) {
        Ok(()) => true,
        Err(e) => {
            println!("  ⚠ не записан {}: {e}", path.display());
            false
        }
    }
}

fn with_block(text: &str) -> String {
    let rest = without_block(text);
    let sep = if rest.is_empty() || rest.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    format!("{rest}{sep}{BEGIN}\n{CSS}\n{END}\n")
}

fn without_block(text: &str) -> String {
    let (Some(start), Some(end)) = (text.find(BEGIN), text.find(END)) else {
        return text.to_string();
    };
    if end < start {
        return text.to_string();
    }
    let after = &text[end + END.len()..];
    let after = after.strip_prefix('\n').unwrap_or(after);
    format!("{}{after}", &text[..start])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_added_once_and_user_rules_kept() {
        let user = "window { color: red; }";
        let once = with_block(user);
        assert!(once.starts_with("window { color: red; }\n/* shell"));
        assert_eq!(with_block(&once), once, "второй раз — без изменений");
        assert_eq!(without_block(&once), "window { color: red; }\n");
        assert_eq!(without_block(&with_block("")), "");
    }

    #[test]
    fn file_written() {
        let dir = std::env::temp_dir().join(format!("shell-gtk-fixes-{}", std::process::id()));
        let path = dir.join("gtk-3.0/gtk.css");
        assert!(apply_in(&path));
        assert!(!apply_in(&path), "второй раз — без изменений");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("border-radius: 0") && text.contains("box-shadow: none"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
