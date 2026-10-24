//! Поле ввода: текст, курсор и правка — общее для «Поиска», «Программ» и
//! переименования значков на столе. Может быть выделена часть текста от
//! начала (при переименовании — имя без расширения, как на Mac): набор
//! заменяет выделенное, Backspace/Delete стирают его, стрелки — снимают.
//! Как в полях macOS/GTK: стрелки, Home/End, Backspace/Delete,
//! Ctrl+W и Ctrl/Alt+Backspace — стереть слово, Ctrl+U — стереть всё.

use shell_core::input::{Key, KeyPress};

/// Что произошло с полем после нажатия.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// Текст изменился — пора искать заново.
    Changed,
    /// Сдвинулся только курсор.
    Moved,
    /// Нажатие не для поля.
    Ignored,
}

#[derive(Debug, Default, Clone)]
pub struct TextField {
    text: String,
    /// Позиция курсора в символах (не байтах).
    cursor: usize,
    /// Выделено от начала до этого символа (0 — ничего).
    selected: usize,
}

impl TextField {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.selected = 0;
    }

    /// Поле с текстом; выделены первые `select` символов, курсор — после них.
    pub fn with_text(text: &str, select: usize) -> TextField {
        let select = select.min(text.chars().count());
        TextField {
            text: text.to_string(),
            cursor: select,
            selected: select,
        }
    }

    /// Выделенный текст (от начала).
    pub fn selection(&self) -> &str {
        &self.text[..self.byte_index(self.selected)]
    }

    /// Стереть выделенное (курсор — на его место).
    fn delete_selection(&mut self) {
        let end = self.byte_index(self.selected);
        self.text.replace_range(..end, "");
        self.cursor = 0;
        self.selected = 0;
    }

    /// Текст до курсора — чтобы узнать, где рисовать курсор.
    pub fn before_cursor(&self) -> &str {
        &self.text[..self.byte_index(self.cursor)]
    }

    pub fn key(&mut self, k: KeyPress) -> Edit {
        if self.selected > 0 {
            match k.key {
                Key::BackSpace | Key::Delete => {
                    self.delete_selection();
                    return Edit::Changed;
                }
                Key::Char(_) if !k.ctrl && !k.alt && !k.logo => self.delete_selection(),
                // Как в полях Mac: стрелка влево — в начало выделения,
                // вправо — в его конец; Home/End — как обычно.
                Key::Left | Key::Right => {
                    self.cursor = if k.key == Key::Left { 0 } else { self.selected };
                    self.selected = 0;
                    return Edit::Moved;
                }
                Key::Home | Key::End => self.selected = 0,
                _ => {}
            }
        }
        let len = self.text.chars().count();
        match k.key {
            Key::Char('u') if k.ctrl => {
                if self.text.is_empty() {
                    return Edit::Ignored;
                }
                self.clear();
                Edit::Changed
            }
            Key::Char('w') if k.ctrl => self.delete_word(),
            Key::BackSpace if k.ctrl || k.alt => self.delete_word(),
            Key::BackSpace if self.cursor > 0 => {
                self.cursor -= 1;
                self.remove_char(self.cursor);
                Edit::Changed
            }
            Key::Delete if self.cursor < len => {
                self.remove_char(self.cursor);
                Edit::Changed
            }
            Key::Left if self.cursor > 0 => {
                self.cursor -= 1;
                Edit::Moved
            }
            Key::Right if self.cursor < len => {
                self.cursor += 1;
                Edit::Moved
            }
            Key::Home if self.cursor > 0 => {
                self.cursor = 0;
                Edit::Moved
            }
            Key::End if self.cursor < len => {
                self.cursor = len;
                Edit::Moved
            }
            Key::Char(c) if !k.ctrl && !k.alt && !k.logo => {
                let at = self.byte_index(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
                Edit::Changed
            }
            _ => Edit::Ignored,
        }
    }

    fn delete_word(&mut self) -> Edit {
        let chars: Vec<char> = self.text.chars().collect();
        let mut start = self.cursor;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start == self.cursor {
            return Edit::Ignored;
        }
        let (a, b) = (self.byte_index(start), self.byte_index(self.cursor));
        self.text.replace_range(a..b, "");
        self.cursor = start;
        Edit::Changed
    }

    fn remove_char(&mut self, at: usize) {
        let i = self.byte_index(at);
        self.text.remove(i);
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(f: &mut TextField, text: &str) {
        for c in text.chars() {
            f.key(KeyPress::plain(Key::Char(c)));
        }
    }

    fn with(key: Key, ctrl: bool, alt: bool) -> KeyPress {
        KeyPress {
            ctrl,
            alt,
            ..KeyPress::plain(key)
        }
    }

    #[test]
    fn editing() {
        let mut f = TextField::default();
        press(&mut f, "тест ред");
        assert_eq!(f.key(KeyPress::plain(Key::Home)), Edit::Moved);
        assert_eq!(f.key(KeyPress::plain(Key::Home)), Edit::Ignored);
        assert_eq!(f.key(KeyPress::plain(Key::Delete)), Edit::Changed);
        press(&mut f, "Т");
        assert_eq!(f.text(), "Тест ред");
        assert_eq!(f.before_cursor(), "Т");
        f.key(KeyPress::plain(Key::End));
        f.key(with(Key::Char('w'), true, false));
        assert_eq!(f.text(), "Тест ");
        f.key(KeyPress::plain(Key::BackSpace));
        assert_eq!(f.text(), "Тест");
        f.key(with(Key::BackSpace, false, true));
        assert_eq!(f.text(), "");
        press(&mut f, "abc");
        f.key(with(Key::Char('u'), true, false));
        assert!(f.is_empty());
    }

    #[test]
    fn selection_is_replaced_by_typing() {
        // «Новая папка» выделена целиком — набор заменяет её.
        let mut f = TextField::with_text("Новая папка", 11);
        assert_eq!(f.selection(), "Новая папка");
        press(&mut f, "Фото");
        assert_eq!(f.text(), "Фото");
        // Выделено имя без расширения: Backspace стирает только его.
        let mut f = TextField::with_text("отчёт.pdf", 5);
        f.key(KeyPress::plain(Key::BackSpace));
        assert_eq!((f.text(), f.before_cursor()), (".pdf", ""));
        // Стрелка снимает выделение, текст цел.
        let mut f = TextField::with_text("отчёт.pdf", 5);
        f.key(KeyPress::plain(Key::Right));
        assert_eq!(f.selection(), "");
        press(&mut f, "2");
        assert_eq!(f.text(), "отчёт2.pdf");
    }

    #[test]
    fn shortcuts_are_not_typed() {
        let mut f = TextField::default();
        let super_a = KeyPress {
            logo: true,
            ..KeyPress::plain(Key::Char('a'))
        };
        assert_eq!(f.key(super_a), Edit::Ignored);
        assert_eq!(f.key(with(Key::Char('x'), true, false)), Edit::Ignored);
        assert!(f.is_empty());
    }
}
