//! Нажатие → байты для программы в терминале, как у xterm: буквы — в UTF-8,
//! Ctrl+буква — управляющий код (и в русской раскладке — по месту клавиши:
//! Ctrl+С — это Ctrl+C), Alt — приставка ESC, стрелки и прочие — команды
//! `ESC [ …` (с Ctrl/Alt/Shift — с номером сочетания).

use alacritty_terminal::term::TermMode;
use mimshel_core::input::{Key, KeyPress, latin};

/// Байты для программы. `None` — нажатие ей ничего не говорит.
pub fn key_bytes(k: &KeyPress, mode: TermMode) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    // Номер сочетания для команд: 1 + Shift + 2·Alt + 4·Ctrl.
    let m = 1 + u8::from(k.shift) + 2 * u8::from(k.alt) + 4 * u8::from(k.ctrl);
    let esc = |mut b: Vec<u8>| {
        if k.alt {
            b.insert(0, 0x1b);
        }
        b
    };
    // Стрелки, Home, End, F1–F4: `ESC [ X`, в режиме программы (vim) — `ESC O X`,
    // с сочетанием — `ESC [ 1 ; m X`.
    let letter = |x: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{x}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{x}").into_bytes()
        } else {
            format!("\x1b[{x}").into_bytes()
        }
    };
    let ss3 = |x: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{x}").into_bytes()
        } else {
            format!("\x1bO{x}").into_bytes()
        }
    };
    // Insert, Delete, PageUp… и F5–F12: `ESC [ n ~`, с сочетанием — `ESC [ n ; m ~`.
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[{n};{m}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let bytes = match k.key {
        Key::Char(c) if k.ctrl => esc(vec![ctrl_code(c)?]),
        Key::Char(c) => esc(c.to_string().into_bytes()),
        Key::Return => esc(vec![b'\r']),
        Key::BackSpace if k.ctrl => esc(vec![0x08]),
        Key::BackSpace => esc(vec![0x7f]),
        Key::Tab if k.shift => b"\x1b[Z".to_vec(),
        Key::Tab => esc(vec![b'\t']),
        Key::Escape => esc(vec![0x1b]),
        Key::Up => letter('A'),
        Key::Down => letter('B'),
        Key::Right => letter('C'),
        Key::Left => letter('D'),
        Key::Home => letter('H'),
        Key::End => letter('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F(1) => ss3('P'),
        Key::F2 | Key::F(2) => ss3('Q'),
        Key::F(3) => ss3('R'),
        Key::F(4) => ss3('S'),
        Key::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]),
        _ => return None,
    };
    Some(bytes)
}

/// Ctrl+клавиша → управляющий код (по латинскому месту клавиши).
fn ctrl_code(c: char) -> Option<u8> {
    let l = latin(c).to_ascii_lowercase();
    Some(match l {
        'a'..='z' => l as u8 - b'a' + 1,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '/' | '-' => 0x1f,
        '8' => 0x7f,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key) -> KeyPress {
        KeyPress::plain(key)
    }

    fn bytes(k: KeyPress) -> Vec<u8> {
        key_bytes(&k, TermMode::empty()).unwrap()
    }

    #[test]
    fn letters_and_control() {
        assert_eq!(bytes(key(Key::Char('я'))), "я".as_bytes());
        let ctrl = |c| KeyPress {
            ctrl: true,
            ..key(Key::Char(c))
        };
        assert_eq!(bytes(ctrl('c')), [3]);
        assert_eq!(
            bytes(ctrl('с')),
            [3],
            "Ctrl+С в русской раскладке — тоже Ctrl+C"
        );
        assert_eq!(bytes(ctrl('d')), [4]);
        let alt = KeyPress {
            alt: true,
            ..key(Key::Char('b'))
        };
        assert_eq!(bytes(alt), b"\x1bb");
        assert_eq!(bytes(key(Key::BackSpace)), [0x7f]);
        assert_eq!(bytes(key(Key::Return)), b"\r");
    }

    #[test]
    fn arrows_and_function_keys() {
        assert_eq!(bytes(key(Key::Up)), b"\x1b[A");
        assert_eq!(
            key_bytes(&key(Key::Up), TermMode::APP_CURSOR).unwrap(),
            b"\x1bOA",
            "vim и less просят такой вид"
        );
        let ctrl_right = KeyPress {
            ctrl: true,
            ..key(Key::Right)
        };
        assert_eq!(bytes(ctrl_right), b"\x1b[1;5C");
        assert_eq!(bytes(key(Key::Delete)), b"\x1b[3~");
        assert_eq!(bytes(key(Key::F(1))), b"\x1bOP");
        assert_eq!(bytes(key(Key::F(12))), b"\x1b[24~");
        assert!(key_bytes(&key(Key::VolumeUp), TermMode::empty()).is_none());
    }
}
