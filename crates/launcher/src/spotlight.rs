//! Состояние Spotlight и реакция на клавиши и мышь.

use shell_apps::App;
use shell_core::input::{Key, KeyPress};
use shell_core::search::{Query, best};

use crate::draw::{BAR_H, LIST_PAD, MAX_ROWS, ROW_H, WIDTH};

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

pub struct Spotlight {
    apps: Vec<App>,
    pub(crate) query: String,
    /// Позиция курсора в строке, в символах (не байтах).
    pub(crate) cursor: usize,
    /// Номера подходящих программ в `apps`, лучшие первыми.
    results: Vec<usize>,
    pub(crate) selected: usize,
    /// Номер первой видимой строки списка.
    pub(crate) scroll: usize,
}

impl Spotlight {
    pub fn new(apps: Vec<App>) -> Spotlight {
        Spotlight {
            apps,
            query: String::new(),
            cursor: 0,
            results: Vec::new(),
            selected: 0,
            scroll: 0,
        }
    }

    /// Новый список программ (после установки или удаления).
    pub fn set_apps(&mut self, apps: Vec<App>) {
        self.apps = apps;
        self.search();
    }

    pub fn apps(&self) -> &[App] {
        &self.apps
    }

    /// Пустое поле — так окно открывается каждый раз.
    pub fn reset(&mut self) {
        self.query.clear();
        self.cursor = 0;
        self.search();
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Подходящие программы, лучшие первыми.
    pub fn results(&self) -> impl Iterator<Item = &App> {
        self.results.iter().map(|&i| &self.apps[i])
    }

    pub(crate) fn result_count(&self) -> usize {
        self.results.len()
    }

    /// Сколько строк списка видно (0 — списка нет).
    pub(crate) fn visible_rows(&self) -> usize {
        if self.query.is_empty() {
            0
        } else {
            // Ничего не нашлось — одна строка с надписью об этом.
            self.results.len().clamp(1, MAX_ROWS)
        }
    }

    /// Размер окна в пикселях.
    pub fn size(&self) -> (u32, u32) {
        let rows = self.visible_rows() as u32;
        let h = if rows == 0 {
            BAR_H
        } else {
            BAR_H + 1 + LIST_PAD * 2 + rows * ROW_H
        };
        (WIDTH, h)
    }

    pub fn key(&mut self, k: KeyPress) -> Action {
        match k.key {
            Key::Escape if !self.query.is_empty() => {
                self.reset();
                Action::Redraw
            }
            Key::Escape => Action::Close,
            // Повторное Super+Пробел закрывает, как Cmd+Пробел на Mac: пока окно
            // открыто, клавиатура у него, и до Xfce это нажатие не доходит.
            Key::Char(' ') if k.logo => Action::Close,
            Key::Return => match self.results.get(self.selected) {
                Some(&i) => Action::Launch(Box::new(self.apps[i].clone())),
                None => Action::Nothing,
            },
            Key::Down => self.select(self.selected as isize + 1),
            Key::Up => self.select(self.selected as isize - 1),
            Key::Tab if k.shift => self.select(self.selected as isize - 1),
            Key::Tab => self.select(self.selected as isize + 1),
            Key::PageDown => self.select(self.selected as isize + MAX_ROWS as isize),
            Key::PageUp => self.select(self.selected as isize - MAX_ROWS as isize),
            Key::Char('u') if k.ctrl => {
                self.query.clear();
                self.cursor = 0;
                self.edited()
            }
            Key::Char('w') if k.ctrl => self.delete_word(),
            Key::BackSpace if k.ctrl || k.alt => self.delete_word(),
            Key::BackSpace if self.cursor > 0 => {
                self.cursor -= 1;
                self.remove_char(self.cursor);
                self.edited()
            }
            Key::Delete if self.cursor < self.query.chars().count() => {
                self.remove_char(self.cursor);
                self.edited()
            }
            Key::Left if self.cursor > 0 => {
                self.cursor -= 1;
                Action::Redraw
            }
            Key::Right if self.cursor < self.query.chars().count() => {
                self.cursor += 1;
                Action::Redraw
            }
            Key::Home => {
                self.cursor = 0;
                Action::Redraw
            }
            Key::End => {
                self.cursor = self.query.chars().count();
                Action::Redraw
            }
            Key::Char(c) if !k.ctrl && !k.alt && !k.logo => {
                let at = self.byte_index(self.cursor);
                self.query.insert(at, c);
                self.cursor += 1;
                self.edited()
            }
            _ => Action::Nothing,
        }
    }

    /// Нажатие кнопки мыши; координаты — относительно окна.
    pub fn click(&mut self, x: i32, y: i32, button: u8) -> Action {
        let (w, h) = self.size();
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            return Action::Close;
        }
        match button {
            1 => match self.row_at(x, y) {
                Some(i) => {
                    self.selected = i;
                    self.key(KeyPress::plain(Key::Return))
                }
                None => Action::Nothing,
            },
            // Колёсико.
            4 => self.select(self.selected as isize - 1),
            5 => self.select(self.selected as isize + 1),
            _ => Action::Nothing,
        }
    }

    /// Движение мыши: строка под указателем становится выбранной.
    pub fn motion(&mut self, x: i32, y: i32) -> Action {
        match self.row_at(x, y) {
            Some(i) if i != self.selected => {
                self.selected = i;
                Action::Redraw
            }
            _ => Action::Nothing,
        }
    }

    /// Номер результата под точкой окна.
    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        let top = (BAR_H + 1 + LIST_PAD) as i32;
        let pad = LIST_PAD as i32;
        if x < pad || x >= WIDTH as i32 - pad || y < top {
            return None;
        }
        let row = ((y - top) / ROW_H as i32) as usize;
        let i = self.scroll + row;
        (row < self.visible_rows() && i < self.results.len()).then_some(i)
    }

    fn select(&mut self, i: isize) -> Action {
        if self.results.is_empty() {
            return Action::Nothing;
        }
        let i = i.clamp(0, self.results.len() as isize - 1) as usize;
        if i == self.selected {
            return Action::Nothing;
        }
        self.selected = i;
        // Прокрутка: выбранная строка всегда видна.
        if i < self.scroll {
            self.scroll = i;
        } else if i >= self.scroll + MAX_ROWS {
            self.scroll = i + 1 - MAX_ROWS;
        }
        Action::Redraw
    }

    fn edited(&mut self) -> Action {
        self.search();
        Action::Redraw
    }

    fn search(&mut self) {
        let query = Query::new(&self.query);
        let indices: Vec<usize> = (0..self.apps.len()).collect();
        let apps = &self.apps;
        self.results = best(&query, &indices, |&i| crate::fields(&apps[i]))
            .into_iter()
            .copied()
            .collect();
        self.selected = 0;
        self.scroll = 0;
    }

    fn delete_word(&mut self) -> Action {
        let chars: Vec<char> = self.query.chars().collect();
        let mut start = self.cursor;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start == self.cursor {
            return Action::Nothing;
        }
        let (a, b) = (self.byte_index(start), self.byte_index(self.cursor));
        self.query.replace_range(a..b, "");
        self.cursor = start;
        self.edited()
    }

    fn remove_char(&mut self, at: usize) {
        let i = self.byte_index(at);
        self.query.remove(i);
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.query
            .char_indices()
            .nth(chars)
            .map_or(self.query.len(), |(i, _)| i)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    pub fn app(name: &str) -> App {
        App {
            id: format!("{name}.desktop"),
            path: PathBuf::from(format!("/x/{name}.desktop")),
            name: name.into(),
            name_untranslated: name.into(),
            generic_name: None,
            comment: None,
            keywords: vec![],
            categories: vec![],
            icon: None,
            command: vec![name.to_lowercase()],
            working_dir: None,
            terminal: false,
            startup_wm_class: None,
        }
    }

    pub fn spotlight() -> Spotlight {
        let names = [
            "Терминал",
            "Текстовый редактор",
            "Firefox",
            "Files",
            "Файлы",
            "Калькулятор",
            "Тест 1",
            "Тест 2",
            "Тест 3",
            "Тест 4",
            "Тест 5",
            "Тест 6",
            "Тест 7",
            "Тест 8",
            "Тест 9",
        ];
        Spotlight::new(names.iter().map(|n| app(n)).collect())
    }

    fn press(s: &mut Spotlight, text: &str) {
        for c in text.chars() {
            s.key(KeyPress::plain(Key::Char(c)));
        }
    }

    fn names(s: &Spotlight) -> Vec<&str> {
        s.results().map(|a| a.name.as_str()).collect()
    }

    #[test]
    fn typing_searches() {
        let mut s = spotlight();
        assert_eq!(s.size(), (WIDTH, BAR_H));
        press(&mut s, "те");
        assert_eq!(names(&s)[..2], ["Терминал", "Текстовый редактор"]);
        press(&mut s, "р");
        assert_eq!(names(&s), ["Терминал"]);
        assert_eq!(s.size().1, BAR_H + 1 + LIST_PAD * 2 + ROW_H);
    }

    #[test]
    fn enter_launches_selected() {
        let mut s = spotlight();
        press(&mut s, "f");
        s.key(KeyPress::plain(Key::Down));
        match s.key(KeyPress::plain(Key::Return)) {
            Action::Launch(app) => assert_eq!(app.name, names(&s)[1]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn escape_clears_then_closes() {
        let mut s = spotlight();
        press(&mut s, "fi");
        assert_eq!(s.key(KeyPress::plain(Key::Escape)), Action::Redraw);
        assert_eq!(s.query(), "");
        assert_eq!(s.key(KeyPress::plain(Key::Escape)), Action::Close);
    }

    #[test]
    fn editing() {
        let mut s = spotlight();
        press(&mut s, "тест ред");
        s.key(KeyPress::plain(Key::Home));
        s.key(KeyPress::plain(Key::Delete));
        press(&mut s, "Т");
        assert_eq!(s.query(), "Тест ред");
        s.key(KeyPress::plain(Key::End));
        let ctrl_w = KeyPress {
            ctrl: true,
            ..KeyPress::plain(Key::Char('w'))
        };
        s.key(ctrl_w);
        assert_eq!(s.query(), "Тест ");
        s.key(KeyPress::plain(Key::BackSpace));
        assert_eq!(s.query(), "Тест");
        let ctrl_u = KeyPress {
            ctrl: true,
            ..KeyPress::plain(Key::Char('u'))
        };
        s.key(ctrl_u);
        assert_eq!(s.query(), "");
    }

    #[test]
    fn scrolling_keeps_selection_visible() {
        let mut s = spotlight();
        press(&mut s, "тест");
        assert_eq!(s.result_count(), 9); // «Тест 1…9»
        for _ in 0..8 {
            s.key(KeyPress::plain(Key::Down));
        }
        assert_eq!((s.selected, s.scroll), (8, 1));
        s.key(KeyPress::plain(Key::PageUp));
        assert_eq!((s.selected, s.scroll), (0, 0));
    }

    #[test]
    fn mouse() {
        let mut s = spotlight();
        press(&mut s, "f");
        let row2 = (BAR_H + 1 + LIST_PAD + ROW_H + ROW_H / 2) as i32;
        assert_eq!(s.motion(100, row2), Action::Redraw);
        assert_eq!(s.selected, 1);
        assert!(matches!(s.click(100, row2, 1), Action::Launch(_)));
        assert_eq!(s.click(-5, 10, 1), Action::Close);
        assert_eq!(s.click(100, 10, 1), Action::Nothing);
    }

    #[test]
    fn super_space_closes() {
        let mut s = spotlight();
        press(&mut s, "fi");
        let super_space = KeyPress {
            logo: true,
            ..KeyPress::plain(Key::Char(' '))
        };
        assert_eq!(s.key(super_space), Action::Close);
        let super_a = KeyPress {
            logo: true,
            ..KeyPress::plain(Key::Char('a'))
        };
        assert_eq!(s.key(super_a), Action::Nothing);
        assert_eq!(s.query(), "fi");
    }

    #[test]
    fn nothing_found_row() {
        let mut s = spotlight();
        press(&mut s, "zzzz");
        assert_eq!(s.result_count(), 0);
        assert_eq!(s.visible_rows(), 1);
        assert_eq!(s.key(KeyPress::plain(Key::Return)), Action::Nothing);
    }
}
