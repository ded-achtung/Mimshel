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
    /// F2 — переименовать (на рабочем столе, как в Windows).
    F2,
    /// Мультимедийные клавиши громкости (их обрабатывает экран блокировки).
    VolumeUp,
    VolumeDown,
    VolumeMute,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Клавиша Super (с логотипом Windows) — на Mac её место занимает Cmd.
    pub logo: bool,
}

impl KeyPress {
    /// Нажатие без Ctrl, Alt и Shift (для проверок).
    pub fn plain(key: Key) -> KeyPress {
        KeyPress {
            key,
            ctrl: false,
            shift: false,
            alt: false,
            logo: false,
        }
    }
}

/// Буква сочетания по месту на клавиатуре: в русской раскладке Ctrl+Ф —
/// это Ctrl+A, Ctrl+С — Ctrl+C и так далее (ЙЦУКЕН → QWERTY).
pub fn latin(c: char) -> char {
    const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбю";
    const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.";
    let c = c.to_lowercase().next().unwrap_or(c);
    RU.chars()
        .position(|r| r == c)
        .and_then(|i| EN.chars().nth(i))
        .unwrap_or(c)
}
