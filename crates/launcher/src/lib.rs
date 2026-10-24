//! Окна «Поиск» и «Программы»: поиск и запуск программ.
//!
//! «Поиск» (`search_panel`) — поле поиска и список подходящих программ,
//! «Программы» (`apps_panel`) — все программы сеткой по страницам, с полем
//! поиска сверху. Общее для обоих — `common` (рисование) и `field` (поле ввода).
//!
//! Поведение «Поиска» как на Mac: пока ничего не набрано — только поле; результаты
//! появляются по мере ввода; стрелки выбирают, Enter запускает, Esc сначала
//! стирает набранное, затем закрывает; клик мимо окна закрывает.
//! Здесь нет окон и X11: состояние, реакция на ввод и рисование в картинку.

mod apps_panel;
mod common;
mod field;
mod search_panel;

pub use apps_panel::{AppsPanel, Grid, backdrop};
pub use search_panel::SearchPanel;

use shell_apps::App;
use shell_core::search::{Fields, Prepared, Query, best_prepared};

/// Что сделать после события.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Nothing,
    /// Картинка изменилась — перерисовать (и, возможно, поменять высоту окна).
    Redraw,
    /// Спрятать окно.
    Close,
    /// Спрятать окно и запустить программу.
    Launch(Box<App>),
}

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

/// Поля всех программ, подготовленные для поиска один раз — при загрузке
/// списка, а не на каждое нажатие клавиши.
pub fn prepare_all(apps: &[App]) -> Vec<Prepared> {
    apps.iter().map(|a| Prepared::new(&fields(a))).collect()
}

/// Номера программ, подходящих к тексту, лучшие первыми.
pub(crate) fn rank(prepared: &[Prepared], text: &str) -> Vec<usize> {
    let query = Query::new(text);
    let indices: Vec<usize> = (0..prepared.len()).collect();
    best_prepared(&query, &indices, |&i| &prepared[i])
        .into_iter()
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn found_by_program_name() {
        let mut app = crate::search_panel::tests::app("Блокнот");
        app.command = vec!["/usr/bin/mousepad".into()];
        assert!(Query::new("mousep").score(&fields(&app)).is_some());
    }
}
