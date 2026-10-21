//! Spotlight: поле поиска и список подходящих программ.
//!
//! Поведение как на Mac: пока ничего не набрано — только поле; результаты
//! появляются по мере ввода; стрелки выбирают, Enter запускает, Esc сначала
//! стирает набранное, затем закрывает; клик мимо окна закрывает.
//! Здесь нет окон и X11: состояние, реакция на ввод и рисование в картинку.

mod draw;
mod spotlight;

pub use spotlight::{Action, Spotlight};

use shell_apps::App;
use shell_core::search::Fields;

/// Где искать для программы: названия, ключевые слова, описание и имя
/// команды (переименованный ярлык «Блокнот» находится и по «mousepad»).
pub fn fields(app: &App) -> Fields<'_> {
    let program = app
        .command
        .first()
        .map(|c| c.rsplit('/').next().unwrap_or(c));
    Fields {
        names: vec![app.name.as_str(), app.name_untranslated.as_str()],
        keywords: app.keywords.iter().map(String::as_str).collect(),
        others: app
            .generic_name
            .iter()
            .chain(&app.comment)
            .map(String::as_str)
            .chain(program)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shell_core::search::Query;

    #[test]
    fn found_by_program_name() {
        let mut app = crate::spotlight::tests::app("Блокнот");
        app.command = vec!["/usr/bin/mousepad".into()];
        assert!(Query::new("mousep").score(&fields(&app)).is_some());
    }
}
