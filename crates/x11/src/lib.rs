//! Связь с X-сервером: всплывающее окно с прозрачностью, вывод в него
//! картинки, захват клавиатуры и мыши, перевод нажатий в буквы с учётом
//! раскладки (us/ru) и регистра.

use std::sync::Mutex;
use std::time::Duration;

pub use shell_core::input::{Key, KeyPress};
use tiny_skia::Pixmap;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event as XEvent;
use x11rb::protocol::xkb::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux,
    EventMask, GrabMode, GrabStatus, ImageFormat, PropMode, StackMode, VisualClass, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};
use xkeysym::{Keysym, key};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// События, которые нужны оболочке. Координаты — относительно окна;
/// при захвате мыши клик мимо окна приходит с координатами за его краями.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Key(KeyPress),
    Button {
        x: i16,
        y: i16,
        button: u8,
    },
    Motion {
        x: i16,
        y: i16,
    },
    /// Окно нужно перерисовать.
    Expose,
    Other,
}

pub struct Display {
    conn: RustConnection,
    root: Window,
    screen_size: (u16, u16),
    /// Вид пикселей с прозрачностью (32 бита) и палитра для него.
    visual: u32,
    colormap: u32,
    keymap: Mutex<Keymap>,
}

impl Display {
    /// Подключается к X-серверу из переменной DISPLAY.
    pub fn connect() -> Result<Display> {
        let (conn, screen_num) = RustConnection::connect(None)?;
        // Просим XKB: тогда в событиях клавиш есть номер раскладки (us/ru).
        conn.xkb_use_extension(1, 0)?.reply()?;
        conn.prefetch_maximum_request_bytes();

        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let screen_size = (screen.width_in_pixels, screen.height_in_pixels);
        let visual = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| &d.visuals)
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .map(|v| v.visual_id)
            .ok_or("X-сервер не умеет окна с прозрачностью (нет 32-битного вида)")?;
        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, root, visual)?;
        let keymap = Mutex::new(Keymap::load(&conn)?);

        Ok(Display {
            conn,
            root,
            screen_size,
            visual,
            colormap,
            keymap,
        })
    }

    /// Размер экрана в пикселях.
    pub fn screen_size(&self) -> (u16, u16) {
        self.screen_size
    }

    /// Создаёт скрытое всплывающее окно (без рамки, поверх остальных).
    pub fn create_popup(&self, name: &str) -> Result<Popup> {
        let window = self.conn.generate_id()?;
        let events = EventMask::KEY_PRESS
            | EventMask::BUTTON_PRESS
            | EventMask::POINTER_MOTION
            | EventMask::EXPOSURE;
        self.conn.create_window(
            32,
            window,
            self.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &CreateWindowAux::new()
                .background_pixel(0)
                .border_pixel(0)
                .colormap(self.colormap)
                .override_redirect(1)
                .event_mask(events),
        )?;
        let class = format!("{name}\0Shell\0");
        self.conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            class.as_bytes(),
        )?;
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, window, &CreateGCAux::new())?;
        self.conn.flush()?;
        Ok(Popup { window, gc })
    }

    /// Перечитывает таблицу клавиш. Смену раскладок (setxkbmap, настройки Xfce)
    /// сервер сообщает событиями XKB, а не `MappingNotify`, поэтому проще
    /// перечитывать при каждом открытии окна — это один короткий запрос.
    pub fn refresh_keymap(&self) -> Result<()> {
        let k = Keymap::load(&self.conn)?;
        *self.keymap.lock().unwrap_or_else(|p| p.into_inner()) = k;
        Ok(())
    }

    /// Ждёт следующее событие.
    pub fn next_event(&self) -> Result<Event> {
        let ev = self.conn.wait_for_event()?;
        Ok(self.translate(ev))
    }

    fn translate(&self, ev: XEvent) -> Event {
        match ev {
            XEvent::KeyPress(e) => {
                let state = u16::from(e.state);
                let keymap = self.keymap.lock().unwrap_or_else(|p| p.into_inner());
                let key = keymap.key(e.detail, state);
                if std::env::var_os("SHELL_DEBUG_KEYS").is_some() {
                    let (sym, _) = keymap.keysym(e.detail, state);
                    eprintln!(
                        "клавиша: код {} состояние {state:#06x} раскладка {} символ {sym:#x} → {key:?}",
                        e.detail,
                        (state >> 13) & 0x3
                    );
                }
                Event::Key(KeyPress {
                    key,
                    shift: state & 0x1 != 0,
                    ctrl: state & 0x4 != 0,
                    alt: state & 0x8 != 0,
                })
            }
            XEvent::ButtonPress(e) => Event::Button {
                x: e.event_x,
                y: e.event_y,
                button: e.detail,
            },
            XEvent::MotionNotify(e) => Event::Motion {
                x: e.event_x,
                y: e.event_y,
            },
            XEvent::Expose(e) if e.count == 0 => Event::Expose,
            XEvent::MappingNotify(_) => {
                // Сменилась раскладка клавиатуры — перечитываем.
                if let Ok(k) = Keymap::load(&self.conn) {
                    *self.keymap.lock().unwrap_or_else(|p| p.into_inner()) = k;
                }
                Event::Other
            }
            _ => Event::Other,
        }
    }
}

/// Всплывающее окно.
pub struct Popup {
    window: Window,
    gc: u32,
}

impl Popup {
    /// Показывает окно в заданном месте и забирает себе клавиатуру и мышь.
    pub fn show(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        d.refresh_keymap()?;
        self.set_geometry(d, x, y, w, h)?;
        d.conn.map_window(self.window)?;
        d.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        d.conn.flush()?;
        // Пока держат горячую клавишу, клавиатура ещё занята тем, кто её поймал,
        // поэтому пробуем несколько раз (до ~1 с).
        for _ in 0..100 {
            let status = d
                .conn
                .grab_keyboard(
                    true,
                    self.window,
                    CURRENT_TIME,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )?
                .reply()?
                .status;
            if status == GrabStatus::SUCCESS {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mask = EventMask::BUTTON_PRESS | EventMask::POINTER_MOTION;
        d.conn
            .grab_pointer(
                true,
                self.window,
                mask,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                NONE,
                NONE,
                CURRENT_TIME,
            )?
            .reply()?;
        Ok(())
    }

    /// Прячет окно и отпускает клавиатуру и мышь.
    pub fn hide(&self, d: &Display) -> Result<()> {
        d.conn.ungrab_keyboard(CURRENT_TIME)?;
        d.conn.ungrab_pointer(CURRENT_TIME)?;
        d.conn.unmap_window(self.window)?;
        d.conn.flush()?;
        Ok(())
    }

    pub fn set_geometry(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        d.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new()
                .x(i32::from(x))
                .y(i32::from(y))
                .width(u32::from(w))
                .height(u32::from(h)),
        )?;
        Ok(())
    }

    /// Выводит картинку в окно (левый верхний угол к углу).
    pub fn present(&self, d: &Display, pixmap: &Pixmap) -> Result<()> {
        let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
        // tiny-skia хранит RGBA, X-сервер ждёт BGRA.
        let mut data = pixmap.data().to_vec();
        for px in data.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        // Большую картинку отправляем полосами — у запроса к X есть предел размера.
        let max = d.conn.maximum_request_bytes().saturating_sub(64);
        let rows = (max / (w * 4)).clamp(1, h.max(1));
        for (i, chunk) in data.chunks(rows * w * 4).enumerate() {
            d.conn.put_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                self.gc,
                w as u16,
                (chunk.len() / (w * 4)) as u16,
                0,
                (i * rows) as i16,
                0,
                32,
                chunk,
            )?;
        }
        d.conn.flush()?;
        Ok(())
    }
}

/// Таблица «код клавиши → символы» из X-сервера.
struct Keymap {
    min_code: u8,
    per_code: usize,
    syms: Vec<u32>,
}

impl Keymap {
    fn load(conn: &RustConnection) -> Result<Keymap> {
        let setup = conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let reply = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        Ok(Keymap {
            min_code: min,
            per_code: usize::from(reply.keysyms_per_keycode),
            syms: reply.keysyms,
        })
    }

    /// Символ клавиши с учётом раскладки, Shift и Caps Lock.
    fn keysym(&self, code: u8, state: u16) -> (u32, bool) {
        let Some(start) = usize::from(code)
            .checked_sub(usize::from(self.min_code))
            .map(|i| i * self.per_code)
        else {
            return (0, false);
        };
        let syms = self.syms.get(start..start + self.per_code).unwrap_or(&[]);
        let at = |i: usize| syms.get(i).copied().unwrap_or(0);
        // Раскладка — в битах 13–14; в общей таблице X на каждую из первых
        // двух приходится по две колонки: без Shift и с Shift.
        let group = usize::from((state >> 13) & 0x3).min(1);
        let shift = state & 0x1 != 0;
        let (base, shifted) = match (at(group * 2), at(group * 2 + 1)) {
            (0, 0) => (at(0), at(1)),
            pair => pair,
        };
        if shift && shifted != 0 {
            (shifted, false)
        } else {
            // Нет отдельного символа для Shift — значит, нужна заглавная буква.
            (base, shift)
        }
    }

    fn key(&self, code: u8, state: u16) -> Key {
        let (sym, want_upper) = self.keysym(code, state);
        let caps = state & 0x2 != 0;
        match sym {
            key::Return | key::KP_Enter => Key::Return,
            key::Escape => Key::Escape,
            key::BackSpace => Key::BackSpace,
            key::Delete | key::KP_Delete => Key::Delete,
            key::Tab | key::ISO_Left_Tab => Key::Tab,
            key::Up | key::KP_Up => Key::Up,
            key::Down | key::KP_Down => Key::Down,
            key::Left | key::KP_Left => Key::Left,
            key::Right | key::KP_Right => Key::Right,
            key::Home | key::KP_Home => Key::Home,
            key::End | key::KP_End => Key::End,
            key::Page_Up | key::KP_Page_Up => Key::PageUp,
            key::Page_Down | key::KP_Page_Down => Key::PageDown,
            _ => match Keysym::new(sym).key_char() {
                Some(c) if !c.is_control() => {
                    // Caps Lock меняет регистр букв (и наоборот при зажатом Shift).
                    if c.is_alphabetic() && (want_upper != caps) {
                        Key::Char(swap_case(c))
                    } else {
                        Key::Char(c)
                    }
                }
                _ => Key::Other,
            },
        }
    }
}

fn swap_case(c: char) -> char {
    if c.is_lowercase() {
        c.to_uppercase().next().unwrap_or(c)
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Две клавиши: «q/Q | й/Й» и «1/! | 1/!», плюс Enter.
    fn keymap() -> Keymap {
        Keymap {
            min_code: 8,
            per_code: 4,
            syms: vec![
                key::q,
                key::Q,
                key::Cyrillic_shorti,
                key::Cyrillic_SHORTI,
                key::_1,
                key::exclam,
                key::_1,
                key::exclam,
                key::Return,
                0,
                key::Return,
                0,
            ],
        }
    }

    const SHIFT: u16 = 0x1;
    const CAPS: u16 = 0x2;
    const RU: u16 = 1 << 13;

    #[test]
    fn layouts_and_case() {
        let k = keymap();
        assert_eq!(k.key(8, 0), Key::Char('q'));
        assert_eq!(k.key(8, SHIFT), Key::Char('Q'));
        assert_eq!(k.key(8, RU), Key::Char('й'));
        assert_eq!(k.key(8, RU | SHIFT), Key::Char('Й'));
        assert_eq!(k.key(8, RU | CAPS), Key::Char('Й'));
        assert_eq!(k.key(8, RU | CAPS | SHIFT), Key::Char('й'));
        // Caps Lock не трогает цифры.
        assert_eq!(k.key(9, CAPS), Key::Char('1'));
        assert_eq!(k.key(9, SHIFT), Key::Char('!'));
        assert_eq!(k.key(10, RU), Key::Return);
        assert_eq!(k.key(200, 0), Key::Other);
    }
}
