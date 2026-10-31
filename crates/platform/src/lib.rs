//! What the shell's parts know about the screen, whatever draws it: the
//! events they react to, keys, errors. Today the X11 backend
//! (`mimshel-x11`) produces these; a Wayland backend will produce the same
//! (docs/platform.md, docs/wayland-plan.md phase 1). Nothing here knows
//! about a display protocol.
//!
//! Window ids (`window: u32`) are the backend's names for surfaces: X11
//! window ids today; a Wayland backend numbers its surfaces itself.

pub use mimshel_core::input::{Key, KeyPress};
use xkeysym::key;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// Which property of a window changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    /// The list of program windows.
    ClientList,
    /// The active window.
    ActiveWindow,
    WorkArea,
    /// Minimized, maximized and the like.
    State,
    Title,
    Icon,
    Class,
    Other,
}

/// The events the shell needs. Coordinates are relative to `window`; while
/// the pointer is grabbed, a click outside the window comes with
/// coordinates beyond its edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A new program window drew for the first time
    /// (once the backend watches new windows).
    Drawn {
        window: u32,
    },
    Key(KeyPress),
    Button {
        window: u32,
        x: i16,
        y: i16,
        button: u8,
    },
    /// A mouse button was released.
    Release {
        window: u32,
        x: i16,
        y: i16,
        button: u8,
    },
    Motion {
        window: u32,
        x: i16,
        y: i16,
    },
    /// The pointer entered the window (at `x`, `y`) / left it.
    Enter {
        window: u32,
        x: i16,
        y: i16,
    },
    Leave {
        window: u32,
    },
    /// The window needs redrawing.
    Expose {
        window: u32,
    },
    Property {
        window: u32,
        what: Property,
    },
    /// The window moved or changed size (the root window: the screen size
    /// changed).
    Configure {
        window: u32,
    },
    /// A window was mapped (after the backend was asked to watch stacking).
    Mapped {
        window: u32,
    },
    /// Files from another program are dragged over the window (at `x`, `y`).
    DropHover {
        window: u32,
        x: i16,
        y: i16,
    },
    DropLeave {
        window: u32,
    },
    /// Files were dropped on the window; `moving` comes from the drag
    /// protocol. The backend hands out the paths by `ticket`, then is told
    /// the drop is finished.
    Dropped {
        window: u32,
        x: i16,
        y: i16,
        moving: bool,
        ticket: u64,
    },
    /// The close button of an app window's frame was pressed, or the window
    /// was destroyed from outside.
    Close {
        window: u32,
    },
    /// The keyboard layout with this number (from 0) became active.
    KeyboardGroup(u8),
    /// The set of layouts changed (setxkbmap, for example).
    KeyboardLayouts,
    Other,
}

/// What is under the pointer during a file drag.
/// Wayland: target state from `wl_data_device` / `wl_data_source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DragTarget {
    /// The source surface itself, where icons can be rearranged.
    #[default]
    Own,
    /// A program window that may accept files.
    App { accepts: bool },
    /// A surface that does not accept files.
    Nothing,
}

/// The key for a keysym that is not a character (Return, arrows, F-keys,
/// volume keys…), the same for every backend; `None` for character keys,
/// which each backend turns into `Key::Char` with its own layout handling.
pub fn special_key(sym: u32) -> Option<Key> {
    Some(match sym {
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
        key::F2 => Key::F2,
        key::F1 => Key::F(1),
        key::F3 => Key::F(3),
        key::F4 => Key::F(4),
        key::F5 => Key::F(5),
        key::F6 => Key::F(6),
        key::F7 => Key::F(7),
        key::F8 => Key::F(8),
        key::F9 => Key::F(9),
        key::F10 => Key::F(10),
        key::F11 => Key::F(11),
        key::F12 => Key::F(12),
        key::Insert | key::KP_Insert => Key::Insert,
        key::XF86_AudioRaiseVolume => Key::VolumeUp,
        key::XF86_AudioLowerVolume => Key::VolumeDown,
        key::XF86_AudioMute | key::XF86_AudioMicMute => Key::VolumeMute,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_keys_and_characters() {
        assert_eq!(special_key(key::Return), Some(Key::Return));
        assert_eq!(special_key(key::KP_Enter), Some(Key::Return));
        assert_eq!(special_key(key::ISO_Left_Tab), Some(Key::Tab));
        assert_eq!(special_key(key::F12), Some(Key::F(12)));
        assert_eq!(special_key(key::a), None);
        assert_eq!(special_key(key::Cyrillic_ve), None);
    }
}
