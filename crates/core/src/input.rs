//! Ввод с клавиатуры, уже переведённый в понятный вид (без привязки к X11).

/// Клавиша после перевода.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Return,
    Escape,
    BackSpace,
    Delete,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl KeyPress {
    /// Нажатие без Ctrl, Alt и Shift (для проверок).
    pub fn plain(key: Key) -> KeyPress {
        KeyPress {
            key,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }
}
